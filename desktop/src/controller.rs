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

use std::sync::{Arc, Mutex};
use std::num::NonZeroU32;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use webrtc::{
    api::{
        interceptor_registry::register_default_interceptors,
        media_engine::MediaEngine,
        APIBuilder,
    },
    ice_transport::ice_server::RTCIceServer,
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
    dpi::PhysicalSize,
    event::{ElementState, Event, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    keyboard::{Key, NamedKey},
    window::WindowBuilder,
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
// Accumulates RTP payload fragments into a complete VP8 bitstream frame.

struct Vp8Depacketizer {
    buf: Vec<u8>,
}

impl Vp8Depacketizer {
    fn new() -> Self {
        Self { buf: Vec::with_capacity(1 << 17) }
    }

    /// Push an RTP payload fragment. Returns the complete VP8 frame bytes
    /// when the marker bit signals the last packet of the frame.
    fn push(&mut self, payload: &[u8], marker: bool) -> Option<Vec<u8>> {
        if payload.is_empty() {
            return None;
        }

        // Parse VP8 payload descriptor (RFC 7741 §4.2)
        let mut offset = 0usize;

        // First byte: X|R|N|S|R|PID[2:0]
        let first = payload[offset];
        offset += 1;
        let x_bit = (first & 0x80) != 0;

        if x_bit && offset < payload.len() {
            // Extension byte: I|L|T|K|RSV[3:0]
            let ext = payload[offset];
            offset += 1;
            let i_bit = (ext & 0x80) != 0;
            let l_bit = (ext & 0x40) != 0;
            let t_bit = (ext & 0x20) != 0;
            let k_bit = (ext & 0x10) != 0;

            if i_bit && offset < payload.len() {
                // PictureID: 1 or 2 bytes
                let m = (payload[offset] & 0x80) != 0;
                offset += 1;
                if m && offset < payload.len() {
                    offset += 1; // second byte of 15-bit PictureID
                }
            }
            if l_bit && offset < payload.len() { offset += 1; } // TL0PICIDX
            if (t_bit || k_bit) && offset < payload.len() { offset += 1; } // TID/KEYIDX
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

use env_libvpx_sys::*;

struct Vp8Decoder {
    ctx: vpx_codec_ctx_t,
}

unsafe impl Send for Vp8Decoder {}

impl Vp8Decoder {
    fn new() -> Result<Self> {
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

    /// Decode one VP8 frame. Returns I420 (YUV planar) image dimensions and data.
    fn decode(&mut self, data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
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

        // Convert I420 → BGRA
        let bgra = i420_to_bgra(img, w, h);
        Some((w, h, bgra))
    }
}

impl Drop for Vp8Decoder {
    fn drop(&mut self) {
        unsafe { vpx_codec_destroy(&mut self.ctx); }
    }
}

/// Convert a libvpx I420 image to packed BGRA.
unsafe fn i420_to_bgra(img: *const vpx_image_t, w: u32, h: u32) -> Vec<u8> {
    let w = w as usize;
    let h = h as usize;
    let mut out = vec![0u8; w * h * 4];

    let y_plane = (*img).planes[VPX_PLANE_Y as usize];
    let u_plane = (*img).planes[VPX_PLANE_U as usize];
    let v_plane = (*img).planes[VPX_PLANE_V as usize];
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
    // 1. Create room
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

    // 2. Connect to signaling as controller
    let (mut signal_rx, signal_tx) = sig_client::connect_as_controller(&ws_url, &token).await?;
    info!("[controller] signaling connected, waiting for participant…");

    // Shared frame buffer between decode task and winit window
    let frame_buf: Arc<Mutex<FrameBuffer>> = Arc::new(Mutex::new(FrameBuffer::default()));
    let frame_buf_render = Arc::clone(&frame_buf);

    // Channel for control messages from the window to the DataChannel sender
    let (input_tx, mut input_rx) = mpsc::channel::<String>(64);

    // Channel for state updates
    let (state_tx, mut state_rx) = mpsc::channel::<RTCPeerConnectionState>(8);
    let (ice_tx, mut ice_rx) = mpsc::channel::<String>(32);

    // 3. Build WebRTC API
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

    // DataChannel for sending control messages
    let dc = pc.create_data_channel("control", None).await?;
    let dc_arc = Arc::clone(&dc);

    // Forward input events to DataChannel once it's open
    tokio::spawn(async move {
        while let Some(msg) = input_rx.recv().await {
            if dc_arc.ready_state() == webrtc::data_channel::data_channel_state::RTCDataChannelState::Open {
                if let Err(e) = dc_arc.send_text(msg).await {
                    warn!("[controller] DataChannel send error: {e}");
                }
            }
        }
    });

    // Video transceiver — recvonly (we receive the participant's screen)
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

    // Forward ICE candidates to signaling
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

    // Winit proxy — we need to request redraws from the async task
    // We use a simple atomic flag + a separate thread for the window
    let redraw_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let redraw_flag_decode = Arc::clone(&redraw_flag);

    // Spawn the winit window on its own thread (winit must run on main thread
    // or a dedicated thread; we use a dedicated thread here so tokio keeps main)
    let input_tx_clone = input_tx.clone();
    let frame_buf_window = Arc::clone(&frame_buf_render);
    let redraw_flag_window = Arc::clone(&redraw_flag);

    let window_thread = std::thread::spawn(move || {
        run_window(frame_buf_window, input_tx_clone, redraw_flag_window)
    });

    // 4. Main signaling event loop — runs until window closes or session ends
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

                        // Wait for ICE gathering
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
                        info!("[controller] connected — screen should appear shortly");
                    }
                    RTCPeerConnectionState::Failed => {
                        error!("[controller] WebRTC failed");
                        let _ = signal_tx.send(r#"{"type":"terminate"}"#.to_owned()).await;
                    }
                    _ => {}
                }
            }
        }

        // Check if the window thread has exited (user closed window)
        if window_thread.is_finished() {
            info!("[controller] window closed — terminating session");
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

// ── Winit window ──────────────────────────────────────────────────────────────

fn run_window(
    frame_buf: Arc<Mutex<FrameBuffer>>,
    input_tx: mpsc::Sender<String>,
    redraw_flag: Arc<std::sync::atomic::AtomicBool>,
) {
    let event_loop = EventLoop::new().expect("create event loop");
    let window = Arc::new(
        WindowBuilder::new()
            .with_title("Remota — Remote Desktop")
            .with_inner_size(PhysicalSize::new(1280u32, 720u32))
            .build(&event_loop)
            .expect("create window"),
    );

    let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
    let mut surface = softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");
    let mut current_w = 1280u32;
    let mut current_h = 720u32;

    // Track window size for normalizing mouse coordinates
    let mut win_w = 1280.0f64;
    let mut win_h = 720.0f64;

    event_loop.run(move |event, elwt| {
        elwt.set_control_flow(ControlFlow::Poll);

        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    elwt.exit();
                }

                WindowEvent::Resized(size) => {
                    win_w = size.width as f64;
                    win_h = size.height as f64;
                }

                WindowEvent::CursorMoved { position, .. } => {
                    let x = (position.x / win_w).clamp(0.0, 1.0);
                    let y = (position.y / win_h).clamp(0.0, 1.0);
                    let msg = serde_json::json!({ "type": "mouse_move", "x": x, "y": y });
                    let _ = input_tx.try_send(msg.to_string());
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
                        "type": "mouse_button",
                        "action": action,
                        "button": btn
                    });
                    let _ = input_tx.try_send(msg.to_string());
                }

                WindowEvent::MouseWheel { delta, .. } => {
                    let (dx, dy) = match delta {
                        MouseScrollDelta::LineDelta(x, y) => (x as f64 * 40.0, y as f64 * 40.0),
                        MouseScrollDelta::PixelDelta(p)   => (p.x, p.y),
                    };
                    let msg = serde_json::json!({ "type": "scroll", "deltaX": dx, "deltaY": -dy });
                    let _ = input_tx.try_send(msg.to_string());
                }

                WindowEvent::KeyboardInput { event: key_event, .. } => {
                    let action = if key_event.state == ElementState::Pressed { "down" } else { "up" };
                    let key_str = match &key_event.logical_key {
                        Key::Named(n) => named_key_to_str(n),
                        Key::Character(c) => c.as_str().to_owned(),
                        _ => return,
                    };
                    let msg = serde_json::json!({
                        "type": "keyboard",
                        "action": action,
                        "key": key_str
                    });
                    let _ = input_tx.try_send(msg.to_string());
                }

                WindowEvent::RedrawRequested => {
                    let fb = frame_buf.lock().unwrap();
                    if fb.width == 0 || fb.height == 0 || fb.data.is_empty() {
                        return;
                    }

                    let w = fb.width;
                    let h = fb.height;

                    if w != current_w || h != current_h {
                        surface.resize(
                            NonZeroU32::new(w).unwrap(),
                            NonZeroU32::new(h).unwrap(),
                        ).unwrap();
                        current_w = w;
                        current_h = h;
                    }

                    let mut buf = surface.buffer_mut().unwrap();
                    // softbuffer on Windows uses 0x00RRGGBB
                    // our frame is BGRA packed
                    for (i, pixel) in buf.iter_mut().enumerate() {
                        let base = i * 4;
                        if base + 3 < fb.data.len() {
                            let b = fb.data[base]     as u32;
                            let g = fb.data[base + 1] as u32;
                            let r = fb.data[base + 2] as u32;
                            *pixel = (r << 16) | (g << 8) | b;
                        }
                    }
                    buf.present().unwrap();
                }

                _ => {}
            }

            Event::AboutToWait => {
                // Check if a new frame is available
                let dirty = {
                    let mut fb = frame_buf.lock().unwrap();
                    let d = fb.dirty;
                    fb.dirty = false;
                    d
                };
                if dirty {
                    window.request_redraw();
                }
            }

            _ => {}
        }
    }).expect("event loop error");
}

fn named_key_to_str(key: &NamedKey) -> String {
    match key {
        NamedKey::Enter       => "Enter",
        NamedKey::Backspace   => "Backspace",
        NamedKey::Delete      => "Delete",
        NamedKey::Escape      => "Escape",
        NamedKey::Tab         => "Tab",
        NamedKey::Space       => " ",
        NamedKey::ArrowUp     => "ArrowUp",
        NamedKey::ArrowDown   => "ArrowDown",
        NamedKey::ArrowLeft   => "ArrowLeft",
        NamedKey::ArrowRight  => "ArrowRight",
        NamedKey::Home        => "Home",
        NamedKey::End         => "End",
        NamedKey::PageUp      => "PageUp",
        NamedKey::PageDown    => "PageDown",
        NamedKey::F1          => "F1",
        NamedKey::F2          => "F2",
        NamedKey::F3          => "F3",
        NamedKey::F4          => "F4",
        NamedKey::F5          => "F5",
        NamedKey::F6          => "F6",
        NamedKey::F7          => "F7",
        NamedKey::F8          => "F8",
        NamedKey::F9          => "F9",
        NamedKey::F10         => "F10",
        NamedKey::F11         => "F11",
        NamedKey::F12         => "F12",
        NamedKey::Shift       => "Shift",
        NamedKey::Control     => "Control",
        NamedKey::Alt         => "Alt",
        NamedKey::Meta        => "Meta",
        NamedKey::CapsLock    => "CapsLock",
        _                     => return String::new(),
    }.to_owned()
}

// ── Controller signaling ──────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ControllerSignalEvent {
    ParticipantJoined,
    Answer(String),
    IceCandidate(String),
    Terminate,
    Disconnected,
}
