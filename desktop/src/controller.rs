// ── Desktop Controller ────────────────────────────────────────────────────────
// Controller mode — creates a room, waits for participant, displays their
// screen in a native egui window and forwards mouse/keyboard input.

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use eframe::egui;
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

use crate::config;
use crate::signaling as sig_client;

// ── Room creation ─────────────────────────────────────────────────────────────

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

    let join_url = config::WEB_URL
        .map(|u| format!("{u}/join/{token}"))
        .unwrap_or_else(|| format!("https://remota.quickdesk.tech/join/{token}"));

    Ok((token, join_url))
}

// ── Shared frame buffer ───────────────────────────────────────────────────────

#[derive(Default)]
pub struct FrameBuffer {
    pub data: Vec<u8>,  // BGRA packed
    pub width: u32,
    pub height: u32,
    pub dirty: bool,
}

// ── VP8 RTP depacketizer (RFC 7741) ──────────────────────────────────────────

struct Vp8Depacketizer {
    buf: Vec<u8>,
}

impl Vp8Depacketizer {
    fn new() -> Self { Self { buf: Vec::with_capacity(1 << 17) } }

    fn push(&mut self, payload: &[u8], marker: bool) -> Option<Vec<u8>> {
        if payload.is_empty() { return None; }

        let mut offset = 0usize;
        let first = payload[offset]; offset += 1;
        let x_bit = (first & 0x80) != 0;

        if x_bit && offset < payload.len() {
            let ext = payload[offset]; offset += 1;
            let i_bit = (ext & 0x80) != 0;
            let l_bit = (ext & 0x40) != 0;
            let t_bit = (ext & 0x20) != 0;
            let k_bit = (ext & 0x10) != 0;
            if i_bit && offset < payload.len() {
                let m = (payload[offset] & 0x80) != 0; offset += 1;
                if m && offset < payload.len() { offset += 1; }
            }
            if l_bit && offset < payload.len() { offset += 1; }
            if (t_bit || k_bit) && offset < payload.len() { offset += 1; }
        }

        if offset >= payload.len() { return None; }
        self.buf.extend_from_slice(&payload[offset..]);

        if marker {
            Some(std::mem::replace(&mut self.buf, Vec::with_capacity(1 << 17)))
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
            vpx_codec_dec_init_ver(&mut ctx, iface, std::ptr::null(), 0,
                VPX_DECODER_ABI_VERSION as i32)
        };
        if rc != vpx_codec_err_t::VPX_CODEC_OK {
            anyhow::bail!("vpx_codec_dec_init failed: {}", rc as i32);
        }
        Ok(Self { ctx })
    }

    fn decode(&mut self, data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
        use vpx_sys::*;
        let rc = unsafe {
            vpx_codec_decode(&mut self.ctx, data.as_ptr(), data.len() as u32,
                std::ptr::null_mut(), 0)
        };
        if rc != vpx_codec_err_t::VPX_CODEC_OK {
            warn!("[controller] decode error: {}", rc as i32);
            return None;
        }
        let mut iter: vpx_codec_iter_t = std::ptr::null();
        let img = unsafe { vpx_codec_get_frame(&mut self.ctx, &mut iter) };
        if img.is_null() { return None; }
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
    let (w, h) = (w as usize, h as usize);
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
            out[i] = b; out[i+1] = g; out[i+2] = r; out[i+3] = 255;
        }
    }
    out
}

// ── Controller entry point ────────────────────────────────────────────────────

pub async fn run_controller() -> Result<()> {
    let (token, join_url) = create_room().context("create room")?;
    info!("[controller] room created, token={token}");

    let ws_url = format!("{}/ws", config::WS_URL);
    let (mut signal_rx, signal_tx) = sig_client::connect_as_controller(&ws_url, &token).await?;
    info!("[controller] signaling connected");

    let frame_buf: Arc<Mutex<FrameBuffer>> = Arc::new(Mutex::new(FrameBuffer::default()));
    let (input_tx, mut input_rx) = mpsc::channel::<String>(64);
    let (state_tx, mut state_rx) = mpsc::channel::<RTCPeerConnectionState>(8);
    let (ice_tx, mut ice_rx) = mpsc::channel::<String>(32);

    // Build WebRTC
    let mut media = MediaEngine::default();
    media.register_default_codecs()?;
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut media)?;
    let api = APIBuilder::new()
        .with_media_engine(media)
        .with_interceptor_registry(registry)
        .build();

    let pc = Arc::new(api.new_peer_connection(RTCConfiguration {
        ice_servers: crate::webrtc_session::ice_servers(),
        ..Default::default()
    }).await?);

    // ICE
    { let ice = ice_tx.clone();
      pc.on_ice_candidate(Box::new(move |c| { let ice = ice.clone();
        Box::pin(async move {
            if let Some(c) = c {
                if let Ok(init) = c.to_json() {
                    if let Ok(s) = serde_json::to_string(&init) { let _ = ice.send(s).await; }
                }
            }
        })
    })); }

    // State
    { let st = state_tx.clone();
      pc.on_peer_connection_state_change(Box::new(move |s| { let st = st.clone();
        Box::pin(async move { info!("[controller] state → {s:?}"); let _ = st.send(s).await; })
    })); }

    // DataChannel
    let dc = pc.create_data_channel("control", None).await?;
    let dc_arc = Arc::clone(&dc);
    tokio::spawn(async move {
        while let Some(msg) = input_rx.recv().await {
            if dc_arc.ready_state() == webrtc::data_channel::data_channel_state::RTCDataChannelState::Open {
                if let Err(e) = dc_arc.send_text(msg).await {
                    warn!("[controller] DC send: {e}");
                }
            }
        }
    });

    // Video transceiver — recvonly
    pc.add_transceiver_from_kind(RTPCodecType::Video, Some(RTCRtpTransceiverInit {
        direction: RTCRtpTransceiverDirection::Recvonly,
        send_encodings: vec![],
    })).await?;

    // ontrack
    { let fb = Arc::clone(&frame_buf);
      pc.on_track(Box::new(move |track, _, _| { let fb = Arc::clone(&fb);
        Box::pin(async move {
            if track.kind() == RTPCodecType::Video {
                info!("[controller] video track received");
                tokio::spawn(async move { run_decode_loop(track, fb).await; });
            }
        })
    })); }

    // ICE forwarding
    { let sig = signal_tx.clone();
      tokio::spawn(async move {
        while let Some(cj) = ice_rx.recv().await {
            match sig_client::make_controller_ice_msg(&cj) {
                Ok(msg) => { let _ = sig.send(msg).await; }
                Err(e) => error!("[controller] ICE: {e}"),
            }
        }
    }); }

    // Spawn egui window on a dedicated thread
    let fb_win = Arc::clone(&frame_buf);
    let itx_win = input_tx.clone();
    let join_url_win = join_url.clone();
    let token_win = token.clone();
    let window_thread = std::thread::spawn(move || {
        run_controller_window(fb_win, itx_win, join_url_win, token_win);
    });

    // Main loop
    let mut started_offer = false;
    loop {
        tokio::select! {
            Some(event) = signal_rx.recv() => match event {
                ControllerSignalEvent::ParticipantJoined => {
                    if started_offer { continue; }
                    started_offer = true;
                    info!("[controller] participant joined — offering");

                    let offer = pc.create_offer(None).await?;
                    pc.set_local_description(offer).await?;
                    let mut g = pc.gathering_complete_promise().await;
                    let _ = g.recv().await;
                    let local = pc.local_description().await.context("no local desc")?;
                    let _ = signal_tx.send(sig_client::make_controller_offer_msg(&local)?).await;
                }
                ControllerSignalEvent::Answer(sdp) => {
                    pc.set_remote_description(RTCSessionDescription::answer(sdp)?).await?;
                    info!("[controller] answer applied");
                }
                ControllerSignalEvent::IceCandidate(json) => {
                    let init: webrtc::ice_transport::ice_candidate::RTCIceCandidateInit =
                        serde_json::from_str(&json)?;
                    let _ = pc.add_ice_candidate(init).await;
                }
                ControllerSignalEvent::Terminate => {
                    info!("[controller] terminated by server"); break;
                }
                ControllerSignalEvent::Disconnected => {
                    info!("[controller] disconnected"); break;
                }
            },

            Some(state) = state_rx.recv() => match state {
                RTCPeerConnectionState::Failed => {
                    error!("[controller] WebRTC failed");
                    let _ = signal_tx.send(r#"{"type":"terminate"}"#.to_owned()).await;
                }
                _ => {}
            },
        }

        if window_thread.is_finished() {
            let _ = signal_tx.send(r#"{"type":"terminate"}"#.to_owned()).await;
            break;
        }
    }

    pc.close().await?;
    Ok(())
}

// ── Decode loop ───────────────────────────────────────────────────────────────

async fn run_decode_loop(
    track: Arc<webrtc::track::track_remote::TrackRemote>,
    frame_buf: Arc<Mutex<FrameBuffer>>,
) {
    let mut depkt = Vp8Depacketizer::new();
    let mut decoder = match Vp8Decoder::new() {
        Ok(d) => d,
        Err(e) => { error!("[controller] decoder init: {e}"); return; }
    };
    loop {
        match track.read_rtp().await {
            Ok((pkt, _)) => {
                if let Some(frame) = depkt.push(&pkt.payload, pkt.header.marker) {
                    if let Some((w, h, bgra)) = decoder.decode(&frame) {
                        let mut fb = frame_buf.lock().unwrap();
                        fb.data = bgra; fb.width = w; fb.height = h; fb.dirty = true;
                    }
                }
            }
            Err(e) => { warn!("[controller] read_rtp: {e}"); break; }
        }
    }
}

// ── Controller window (egui) ──────────────────────────────────────────────────

fn run_controller_window(
    frame_buf: Arc<Mutex<FrameBuffer>>,
    input_tx: mpsc::Sender<String>,
    join_url: String,
    token: String,
) {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Remota — Remote Desktop")
            .with_inner_size([1280.0, 760.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Remota — Remote Desktop",
        options,
        Box::new(move |_cc| Ok(Box::new(ControllerWindow {
            frame_buf,
            input_tx,
            join_url,
            token,
            texture: None,
            connected: false,
        }))),
    ).unwrap();
}

struct ControllerWindow {
    frame_buf: Arc<Mutex<FrameBuffer>>,
    input_tx: mpsc::Sender<String>,
    join_url: String,
    token: String,
    texture: Option<egui::TextureHandle>,
    connected: bool,
}

impl eframe::App for ControllerWindow {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Upload new frame to GPU texture if dirty
        {
            let mut fb = self.frame_buf.lock().unwrap();
            if fb.dirty && fb.width > 0 && fb.height > 0 {
                fb.dirty = false;
                self.connected = true;

                // Convert BGRA → RGBA for egui
                let mut rgba = vec![0u8; fb.data.len()];
                for i in (0..fb.data.len()).step_by(4) {
                    rgba[i]   = fb.data[i+2]; // R
                    rgba[i+1] = fb.data[i+1]; // G
                    rgba[i+2] = fb.data[i];   // B
                    rgba[i+3] = 255;
                }

                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [fb.width as usize, fb.height as usize],
                    &rgba,
                );

                match &mut self.texture {
                    Some(t) => t.set(image, egui::TextureOptions::LINEAR),
                    None => {
                        self.texture = Some(ctx.load_texture(
                            "remote-screen", image, egui::TextureOptions::LINEAR,
                        ));
                    }
                }
            }
        }

        ctx.request_repaint(); // continuous repaint for live video

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                if !self.connected {
                    ui.vertical_centered(|ui| {
                        ui.add_space(40.0);
                        ui.label(egui::RichText::new("Waiting for participant…")
                            .size(20.0).color(egui::Color32::WHITE));
                        ui.add_space(28.0);

                        // Session token — paste this into the app
                        ui.label(egui::RichText::new("Session token (paste in the Remota app):")
                            .color(egui::Color32::GRAY).size(13.0));
                        ui.add_space(4.0);
                        ui.add(egui::TextEdit::singleline(&mut self.token.clone())
                            .desired_width(440.0)
                            .font(egui::TextStyle::Monospace));
                        ui.add_space(4.0);
                        if ui.button("📋  Copy token").clicked() {
                            ctx.copy_text(self.token.clone());
                        }

                        ui.add_space(20.0);
                        ui.separator();
                        ui.add_space(12.0);

                        // Full invite link — open in browser
                        ui.label(egui::RichText::new("Or share this link:")
                            .color(egui::Color32::GRAY).size(13.0));
                        ui.add_space(4.0);
                        ui.add(egui::TextEdit::singleline(&mut self.join_url.clone())
                            .desired_width(440.0)
                            .font(egui::TextStyle::Monospace));
                        ui.add_space(4.0);
                        if ui.button("📋  Copy link").clicked() {
                            ctx.copy_text(self.join_url.clone());
                        }
                    });
                    return;
                }

                if let Some(texture) = &self.texture {
                    let available = ui.available_size();
                    let tex_size = texture.size_vec2();
                    // Scale to fit while maintaining aspect ratio
                    let scale = (available.x / tex_size.x).min(available.y / tex_size.y);
                    let display_size = tex_size * scale;

                    let response = ui.add(
                        egui::Image::new(texture)
                            .fit_to_exact_size(display_size)
                            .sense(egui::Sense::click_and_drag()),
                    );

                    // Normalize pointer position relative to the image rect
                    if let Some(pos) = response.hover_pos() {
                        let rect = response.rect;
                        let x = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64;
                        let y = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0) as f64;

                        // Mouse move
                        if response.hovered() {
                            let msg = serde_json::json!({"type":"mouse_move","x":x,"y":y});
                            let _ = self.input_tx.try_send(msg.to_string());
                        }

                        // Mouse buttons
                        if response.clicked() {
                            send_click(&self.input_tx, "left", x, y);
                        }
                        if response.secondary_clicked() {
                            send_click(&self.input_tx, "right", x, y);
                        }
                        if response.middle_clicked() {
                            send_click(&self.input_tx, "middle", x, y);
                        }
                    }

                    // Scroll
                    let scroll = ui.input(|i| i.smooth_scroll_delta);
                    if scroll != egui::Vec2::ZERO {
                        let msg = serde_json::json!({
                            "type": "scroll",
                            "deltaX": -scroll.x as f64,
                            "deltaY": -scroll.y as f64
                        });
                        let _ = self.input_tx.try_send(msg.to_string());
                    }
                }

                // Keyboard input
                ctx.input(|i| {
                    for event in &i.events {
                        if let egui::Event::Key { key, pressed, modifiers, .. } = event {
                            let action = if *pressed { "down" } else { "up" };

                            // Send modifiers
                            if modifiers.shift {
                                send_key(&self.input_tx, action, "Shift");
                            }
                            if modifiers.ctrl {
                                send_key(&self.input_tx, action, "Control");
                            }
                            if modifiers.alt {
                                send_key(&self.input_tx, action, "Alt");
                            }

                            let key_str = egui_key_to_str(key);
                            if !key_str.is_empty() {
                                send_key(&self.input_tx, action, key_str);
                            }
                        }
                        if let egui::Event::Text(text) = event {
                            for ch in text.chars() {
                                let s = ch.to_string();
                                send_key(&self.input_tx, "down", &s);
                                send_key(&self.input_tx, "up", &s);
                            }
                        }
                    }
                });
            });
    }
}

fn send_click(tx: &mpsc::Sender<String>, button: &str, x: f64, y: f64) {
    let down = serde_json::json!({"type":"mouse_button","action":"down","button":button,"x":x,"y":y});
    let up   = serde_json::json!({"type":"mouse_button","action":"up",  "button":button,"x":x,"y":y});
    let _ = tx.try_send(down.to_string());
    let _ = tx.try_send(up.to_string());
}

fn send_key(tx: &mpsc::Sender<String>, action: &str, key: &str) {
    let msg = serde_json::json!({"type":"keyboard","action":action,"key":key});
    let _ = tx.try_send(msg.to_string());
}

fn egui_key_to_str(key: &egui::Key) -> &'static str {
    match key {
        egui::Key::Enter      => "Enter",
        egui::Key::Backspace  => "Backspace",
        egui::Key::Delete     => "Delete",
        egui::Key::Escape     => "Escape",
        egui::Key::Tab        => "Tab",
        egui::Key::Space      => " ",
        egui::Key::ArrowUp    => "ArrowUp",
        egui::Key::ArrowDown  => "ArrowDown",
        egui::Key::ArrowLeft  => "ArrowLeft",
        egui::Key::ArrowRight => "ArrowRight",
        egui::Key::Home       => "Home",
        egui::Key::End        => "End",
        egui::Key::PageUp     => "PageUp",
        egui::Key::PageDown   => "PageDown",
        egui::Key::F1         => "F1",
        egui::Key::F2         => "F2",
        egui::Key::F3         => "F3",
        egui::Key::F4         => "F4",
        egui::Key::F5         => "F5",
        egui::Key::F6         => "F6",
        egui::Key::F7         => "F7",
        egui::Key::F8         => "F8",
        egui::Key::F9         => "F9",
        egui::Key::F10        => "F10",
        egui::Key::F11        => "F11",
        egui::Key::F12        => "F12",
        _                     => "",
    }
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
