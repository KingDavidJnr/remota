// ── Desktop Controller ────────────────────────────────────────────────────────
// Native Windows controller mode.
//
// Invocation:  remota-desktop.exe --controller
//
// Flow:
//   1. Call POST /rooms on the backend to create a room.
//   2. Print the join URL for the controller to share.
//   3. Connect to signaling as "controller".
//   4. Wait for participant_joined → send WebRTC offer.
//   5. Receive VP8 video stream → decode → render in a window.
//   6. Capture mouse/keyboard from the window → send over DataChannel.
//   7. End on window close or server terminate.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use webrtc::{
    api::{
        interceptor_registry::register_default_interceptors,
        media_engine::MediaEngine,
        APIBuilder,
    },
    interceptor::registry::Registry,
    peer_connection::{
        configuration::RTCConfiguration,
        peer_connection_state::RTCPeerConnectionState,
        sdp::session_description::RTCSessionDescription,
    },
    rtp_transceiver::rtp_codec::RTPCodecType,
    rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection,
    rtp_transceiver::RTCRtpTransceiverInit,
};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, NamedKey},
    window::{Window, WindowId},
};

use crate::config;
use crate::signaling as sig_client;

// ── Room creation ─────────────────────────────────────────────────────────────

/// Call POST /rooms and return (token, join_url).
pub fn create_room() -> Result<(String, String)> {
    let base = config::WS_URL
        .replace("wss://", "https://")
        .replace("ws://", "http://");

    let url = format!("{base}/rooms");
    let resp = ureq::post(&url)
        .set("Content-Type", "application/json")
        .send_string("{}")
        .context("POST /rooms failed")?;

    let body: serde_json::Value = resp.into_json().context("parse /rooms response")?;
    let token = body["token"]
        .as_str()
        .context("no token in /rooms response")?
        .to_owned();

    let join_url = crate::config::WEB_URL
        .map(|u| format!("{u}/join/{token}"))
        .unwrap_or_else(|| format!("https://remota.quickdesk.tech/join/{token}"));

    Ok((token, join_url))
}

// ── Shared frame buffer ───────────────────────────────────────────────────────

#[derive(Default)]
pub struct FrameBuffer {
    /// BGRA packed pixels, width × height × 4 bytes.
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub dirty: bool,
}

// ── VP8 RTP depacketizer ──────────────────────────────────────────────────────
// Implements RFC 7741 VP8 RTP payload format.

struct Vp8Depacketizer {
    buf: Vec<u8>,
}

impl Vp8Depacketizer {
    fn new() -> Self {
        Self { buf: Vec::with_capacity(1 << 17) }
    }

    fn push(&mut self, payload: &[u8], marker: bool) -> Option<Vec<u8>> {
        if payload.is_empty() {
            return None;
        }

        let mut offset = 0usize;

        // First byte: X|R|N|S|R|PID[2:0]
        let first = payload[offset];
        offset += 1;
        let x_bit = (first & 0x80) != 0;

        if x_bit && offset < payload.len() {
            let ext = payload[offset];
            offset += 1;
            let i_bit = (ext & 0x80) != 0;
            let l_bit = (ext & 0x40) != 0;
            let t_bit = (ext & 0x20) != 0;
            let k_bit = (ext & 0x10) != 0;

            if i_bit && offset < payload.len() {
                let m = (payload[offset] & 0x80) != 0;
                offset += 1;
                if m && offset < payload.len() {
                    offset += 1;
                }
            }
            if l_bit && offset < payload.len() { offset += 1; }
            if (t_bit || k_bit) && offset < payload.len() { offset += 1; }
        }

        if offset >= payload.len() {
            return None;
        }

        self.buf.extend_from_slice(&payload[offset..]);

        if marker {
            let frame = std::mem::replace(&mut self.buf, Vec::with_capacity(1 << 17));
            Some(frame)
        } else {
            None
        }
    }
}

// ── VP8 decoder ───────────────────────────────────────────────────────────────

struct Vp8Decoder {
    ctx: vpx_sys::vpx_codec_ctx_t,
}

unsafe impl Send for Vp8Decoder {}

impl Vp8Decoder {
    fn new() -> Result<Self> {
        use vpx_sys::*;
        let mut ctx: vpx_codec_ctx_t = unsafe { std::mem::zeroed() };
        let iface = unsafe { vpx_codec_vp8_dx() };
        let rc = unsafe {
            vpx_codec_dec_init_ver(
                &mut ctx,
                iface,
                std::ptr::null(),
                0,
                VPX_DECODER_ABI_VERSION as i32,
            )
        };
        if rc != vpx_codec_err_t_VPX_CODEC_OK {
            anyhow::bail!("vpx_codec_dec_init failed: {rc}");
        }
        Ok(Self { ctx })
    }

    fn decode(&mut self, data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
        use vpx_sys::*;
        let rc = unsafe {
            vpx_codec_decode(
                &mut self.ctx,
                data.as_ptr(),
                data.len() as u32,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != vpx_codec_err_t_VPX_CODEC_OK {
            warn!("[controller] vpx_codec_decode error: {rc}");
            return None;
        }

        let mut iter: vpx_codec_iter_t = std::ptr::null();
        let img = unsafe { vpx_codec_get_frame(&mut self.ctx, &mut iter) };
        if img.is_null() {
            return None;
        }

        let w = unsafe { (*img).d_w };
        let h = unsafe { (*img).d_h };
        let bgra = unsafe { i420_to_bgra(img, w, h) };
        Some((w, h, bgra))
    }
}

impl Drop for Vp8Decoder {
    fn drop(&mut self) {
        unsafe { vpx_sys::vpx_codec_destroy(&mut self.ctx); }
    }
}

unsafe fn i420_to_bgra(img: *const vpx_sys::vpx_image_t, w: u32, h: u32) -> Vec<u8> {
    use vpx_sys::*;
    let w = w as usize;
    let h = h as usize;
    let mut out = vec![0u8; w * h * 4];

    let y_plane  = (*img).planes[VPX_PLANE_Y as usize];
    let u_plane  = (*img).planes[VPX_PLANE_U as usize];
    let v_plane  = (*img).planes[VPX_PLANE_V as usize];
    let y_stride = (*img).stride[VPX_PLANE_Y as usize] as usize;
    let u_stride = (*img).stride[VPX_PLANE_U as usize] as usize;
    let v_stride = (*img).stride[VPX_PLANE_V as usize] as usize;

    for row in 0..h {
        for col in 0..w {
            let y = *y_plane.add(row * y_stride + col) as f32;
            let u = *u_plane.add((row / 2) * u_stride + col / 2) as f32;
            let v = *v_plane.add((row / 2) * v_stride + col / 2) as f32;

            let r = (y + 1.402 * (v - 128.0)).clamp(0.0, 255.0) as u8;
            let g = (y - 0.344 * (u - 128.0) - 0.714 * (v - 128.0)).clamp(0.0, 255.0) as u8;
            let b = (y + 1.772 * (u - 128.0)).clamp(0.0, 255.0) as u8;

            let i = (row * w + col) * 4;
            out[i]     = b;
            out[i + 1] = g;
            out[i + 2] = r;
            out[i + 3] = 255;
        }
    }
    out
}

// ── Controller entry point ────────────────────────────────────────────────────

pub async fn run_controller() -> Result<()> {
    let (token, join_url) = create_room().context("create room")?;

    println!();
    println!("  ┌─────────────────────────────────────────────────────┐");
    println!("  │  Remota — Controller Mode                           │");
    println!("  │                                                     │");
    println!("  │  Share this link with the participant:              │");
    println!("  │                                                     │");
    println!("  │  {:<51} │", join_url);
    println!("  │                                                     │");
    println!("  │  Waiting for participant to connect…                │");
    println!("  └─────────────────────────────────────────────────────┘");
    println!();

    info!("[controller] token={token}");

    let ws_url = format!("{}/ws", config::WS_URL);

    let (mut signal_rx, signal_tx) = sig_client::connect_as_controller(&ws_url, &token).await?;
    info!("[controller] signaling connected, waiting for participant…");

    let frame_buf: Arc<Mutex<FrameBuffer>> = Arc::new(Mutex::new(FrameBuffer::default()));
    let (input_tx, mut input_rx) = mpsc::channel::<String>(64);
    let (state_tx, mut state_rx) = mpsc::channel::<RTCPeerConnectionState>(8);
    let (ice_tx, mut ice_rx) = mpsc::channel::<String>(32);

    // Build WebRTC API
    let mut media = MediaEngine::default();
    media.register_default_codecs().context("register codecs")?;
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut media)?;
    let api = APIBuilder::new()
        .with_media_engine(media)
        .with_interceptor_registry(registry)
        .build();

    let ice_servers = crate::webrtc_session::ice_servers();
    let config = RTCConfiguration { ice_servers, ..Default::default() };
    let pc = Arc::new(api.new_peer_connection(config).await?);

    // ICE candidates
    {
        let ice = ice_tx.clone();
        pc.on_ice_candidate(Box::new(move |c| {
            let ice = ice.clone();
            Box::pin(async move {
                if let Some(c) = c {
                    if let Ok(init) = c.to_json() {
                        if let Ok(s) = serde_json::to_string(&init) {
                            let _ = ice.send(s).await;
                        }
                    }
                }
            })
        }));
    }

    // Connection state
    {
        let st = state_tx.clone();
        pc.on_peer_connection_state_change(Box::new(move |s| {
            let st = st.clone();
            Box::pin(async move {
                info!("[controller] WebRTC state → {s:?}");
                let _ = st.send(s).await;
            })
        }));
    }

    // DataChannel for control messages
    let dc = pc.create_data_channel("control", None).await?;
    let dc_arc = Arc::clone(&dc);

    tokio::spawn(async move {
        while let Some(msg) = input_rx.recv().await {
            if dc_arc.ready_state() == webrtc::data_channel::data_channel_state::RTCDataChannelState::Open {
                if let Err(e) = dc_arc.send_text(msg).await {
                    warn!("[controller] DataChannel send error: {e}");
                }
            }
        }
    });

    // Video transceiver — recvonly
    pc.add_transceiver_from_kind(
        RTPCodecType::Video,
        Some(RTCRtpTransceiverInit {
            direction: RTCRtpTransceiverDirection::Recvonly,
            send_encodings: vec![],
        }),
    ).await?;

    // ontrack — spawn decode loop
    {
        let fb = Arc::clone(&frame_buf);
        pc.on_track(Box::new(move |track, _, _| {
            let fb = Arc::clone(&fb);
            Box::pin(async move {
                if track.kind() == RTPCodecType::Video {
                    info!("[controller] video track received, starting decode loop");
                    tokio::spawn(async move {
                        run_decode_loop(track, fb).await;
                    });
                }
            })
        }));
    }

    // Forward ICE candidates
    {
        let sig = signal_tx.clone();
        tokio::spawn(async move {
            while let Some(candidate_json) = ice_rx.recv().await {
                match sig_client::make_controller_ice_msg(&candidate_json) {
                    Ok(msg) => { let _ = sig.send(msg).await; }
                    Err(e) => error!("[controller] ICE msg: {e}"),
                }
            }
        });
    }

    // Spawn winit window on a dedicated thread
    let input_tx_win = input_tx.clone();
    let frame_buf_win = Arc::clone(&frame_buf);
    let window_thread = std::thread::spawn(move || {
        run_window(frame_buf_win, input_tx_win);
    });

    // Main event loop
    let mut started_offer = false;

    loop {
        tokio::select! {
            Some(event) = signal_rx.recv() => {
                match event {
                    ControllerSignalEvent::ParticipantJoined => {
                        if started_offer { continue; }
                        started_offer = true;
                        info!("[controller] participant joined — creating offer");

                        let offer = pc.create_offer(None).await?;
                        pc.set_local_description(offer).await?;

                        let mut gather = pc.gathering_complete_promise().await;
                        let _ = gather.recv().await;

                        let local = pc.local_description().await
                            .context("no local description")?;
                        let msg = sig_client::make_controller_offer_msg(&local)?;
                        let _ = signal_tx.send(msg).await;
                        info!("[controller] offer sent");
                    }
                    ControllerSignalEvent::Answer(sdp) => {
                        let answer = RTCSessionDescription::answer(sdp)?;
                        pc.set_remote_description(answer).await?;
                        info!("[controller] answer applied");
                    }
                    ControllerSignalEvent::IceCandidate(json) => {
                        let init: webrtc::ice_transport::ice_candidate::RTCIceCandidateInit =
                            serde_json::from_str(&json)?;
                        if let Err(e) = pc.add_ice_candidate(init).await {
                            warn!("[controller] add_ice_candidate: {e}");
                        }
                    }
                    ControllerSignalEvent::Terminate => {
                        info!("[controller] session terminated by server");
                        break;
                    }
                    ControllerSignalEvent::Disconnected => {
                        info!("[controller] signaling disconnected");
                        break;
                    }
                }
            }

            Some(state) = state_rx.recv() => {
                match state {
                    RTCPeerConnectionState::Connected => {
                        info!("[controller] WebRTC connected — screen should appear");
                    }
                    RTCPeerConnectionState::Failed => {
                        error!("[controller] WebRTC failed");
                        let _ = signal_tx.send(r#"{"type":"terminate"}"#.to_owned()).await;
                    }
                    _ => {}
                }
            }
        }

        if window_thread.is_finished() {
            info!("[controller] window closed — terminating");
            let _ = signal_tx.send(r#"{"type":"terminate"}"#.to_owned()).await;
            break;
        }
    }

    pc.close().await?;
    Ok(())
}

// ── VP8 decode loop ───────────────────────────────────────────────────────────

async fn run_decode_loop(
    track: Arc<webrtc::track::track_remote::TrackRemote>,
    frame_buf: Arc<Mutex<FrameBuffer>>,
) {
    let mut depacketizer = Vp8Depacketizer::new();
    let mut decoder = match Vp8Decoder::new() {
        Ok(d) => d,
        Err(e) => { error!("[controller] VP8 decoder init failed: {e}"); return; }
    };

    loop {
        match track.read_rtp().await {
            Ok((rtp_packet, _)) => {
                let marker = rtp_packet.header.marker;
                if let Some(frame_data) = depacketizer.push(&rtp_packet.payload, marker) {
                    if let Some((w, h, bgra)) = decoder.decode(&frame_data) {
                        let mut fb = frame_buf.lock().unwrap();
                        fb.data = bgra;
                        fb.width = w;
                        fb.height = h;
                        fb.dirty = true;
                    }
                }
            }
            Err(e) => {
                warn!("[controller] track.read_rtp error: {e}");
                break;
            }
        }
    }
}

// ── Winit window (ApplicationHandler pattern) ─────────────────────────────────

struct RemotaApp {
    frame_buf: Arc<Mutex<FrameBuffer>>,
    input_tx: mpsc::Sender<String>,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    current_w: u32,
    current_h: u32,
    win_w: f64,
    win_h: f64,
}

impl RemotaApp {
    fn new(frame_buf: Arc<Mutex<FrameBuffer>>, input_tx: mpsc::Sender<String>) -> Self {
        Self {
            frame_buf,
            input_tx,
            window: None,
            surface: None,
            current_w: 1280,
            current_h: 720,
            win_w: 1280.0,
            win_h: 720.0,
        }
    }
}

impl ApplicationHandler for RemotaApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes()
            .with_title("Remota — Remote Desktop")
            .with_inner_size(PhysicalSize::new(1280u32, 720u32));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        let context = softbuffer::Context::new(Arc::clone(&window))
            .expect("softbuffer context");
        let surface = softbuffer::Surface::new(&context, Arc::clone(&window))
            .expect("softbuffer surface");
        self.window = Some(window);
        self.surface = Some(surface);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            WindowEvent::Resized(size) => {
                self.win_w = size.width as f64;
                self.win_h = size.height as f64;
            }

            WindowEvent::CursorMoved { position, .. } => {
                let x = (position.x / self.win_w).clamp(0.0, 1.0);
                let y = (position.y / self.win_h).clamp(0.0, 1.0);
                let msg = serde_json::json!({ "type": "mouse_move", "x": x, "y": y });
                let _ = self.input_tx.try_send(msg.to_string());
            }

            WindowEvent::MouseInput { button, state, .. } => {
                let btn = match button {
                    MouseButton::Left   => "left",
                    MouseButton::Right  => "right",
                    MouseButton::Middle => "middle",
                    _ => return,
                };
                let action = if state == ElementState::Pressed { "down" } else { "up" };
                let msg = serde_json::json!({
                    "type": "mouse_button", "action": action, "button": btn
                });
                let _ = self.input_tx.try_send(msg.to_string());
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x as f64 * 40.0, y as f64 * 40.0),
                    MouseScrollDelta::PixelDelta(p)   => (p.x, p.y),
                };
                let msg = serde_json::json!({ "type": "scroll", "deltaX": dx, "deltaY": -dy });
                let _ = self.input_tx.try_send(msg.to_string());
            }

            WindowEvent::KeyboardInput { event: key_event, .. } => {
                let action = if key_event.state == ElementState::Pressed { "down" } else { "up" };
                let key_str = match &key_event.logical_key {
                    Key::Named(n) => named_key_to_str(n),
                    Key::Character(c) => c.as_str().to_owned(),
                    _ => return,
                };
                if key_str.is_empty() { return; }
                let msg = serde_json::json!({
                    "type": "keyboard", "action": action, "key": key_str
                });
                let _ = self.input_tx.try_send(msg.to_string());
            }

            WindowEvent::RedrawRequested => {
                let surface = match self.surface.as_mut() { Some(s) => s, None => return };
                let fb = self.frame_buf.lock().unwrap();
                if fb.width == 0 || fb.height == 0 || fb.data.is_empty() { return; }

                let w = fb.width;
                let h = fb.height;

                if w != self.current_w || h != self.current_h {
                    if let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) {
                        let _ = surface.resize(nw, nh);
                        self.current_w = w;
                        self.current_h = h;
                    }
                }

                if let Ok(mut buf) = surface.buffer_mut() {
                    for (i, pixel) in buf.iter_mut().enumerate() {
                        let base = i * 4;
                        if base + 2 < fb.data.len() {
                            let b = fb.data[base]     as u32;
                            let g = fb.data[base + 1] as u32;
                            let r = fb.data[base + 2] as u32;
                            // softbuffer on Windows: 0x00RRGGBB
                            *pixel = (r << 16) | (g << 8) | b;
                        }
                    }
                    let _ = buf.present();
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        let dirty = {
            let mut fb = self.frame_buf.lock().unwrap();
            let d = fb.dirty;
            fb.dirty = false;
            d
        };
        if dirty {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }
}

fn run_window(frame_buf: Arc<Mutex<FrameBuffer>>, input_tx: mpsc::Sender<String>) {
    let event_loop = EventLoop::new().expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = RemotaApp::new(frame_buf, input_tx);
    event_loop.run_app(&mut app).expect("event loop error");
}

fn named_key_to_str(key: &NamedKey) -> String {
    match key {
        NamedKey::Enter      => "Enter",
        NamedKey::Backspace  => "Backspace",
        NamedKey::Delete     => "Delete",
        NamedKey::Escape     => "Escape",
        NamedKey::Tab        => "Tab",
        NamedKey::Space      => " ",
        NamedKey::ArrowUp    => "ArrowUp",
        NamedKey::ArrowDown  => "ArrowDown",
        NamedKey::ArrowLeft  => "ArrowLeft",
        NamedKey::ArrowRight => "ArrowRight",
        NamedKey::Home       => "Home",
        NamedKey::End        => "End",
        NamedKey::PageUp     => "PageUp",
        NamedKey::PageDown   => "PageDown",
        NamedKey::F1         => "F1",
        NamedKey::F2         => "F2",
        NamedKey::F3         => "F3",
        NamedKey::F4         => "F4",
        NamedKey::F5         => "F5",
        NamedKey::F6         => "F6",
        NamedKey::F7         => "F7",
        NamedKey::F8         => "F8",
        NamedKey::F9         => "F9",
        NamedKey::F10        => "F10",
        NamedKey::F11        => "F11",
        NamedKey::F12        => "F12",
        NamedKey::Shift      => "Shift",
        NamedKey::Control    => "Control",
        NamedKey::Alt        => "Alt",
        NamedKey::Meta       => "Meta",
        NamedKey::CapsLock   => "CapsLock",
        _                    => return String::new(),
    }.to_owned()
}

// ── Controller signaling event type ──────────────────────────────────────────

#[derive(Debug)]
pub enum ControllerSignalEvent {
    ParticipantJoined,
    Answer(String),
    IceCandidate(String),
    Terminate,
    Disconnected,
}
