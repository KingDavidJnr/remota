// ── Desktop Controller (async task) ──────────────────────────────────────────
// Called from the main eframe app. Handles WebRTC offerer role and
// VP8 decode. Sends frames back to the UI via shared FrameBuffer.

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
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

    let resp = ureq::post(&format!("{base}/rooms"))
        .set("Content-Type", "application/json")
        .send_string("{}")
        .context("POST /rooms failed")?;

    let body: serde_json::Value = resp.into_json().context("parse /rooms response")?;
    let token = body["token"].as_str().context("no token")?.to_owned();

    let join_url = config::WEB_URL
        .map(|u| format!("{u}/join/{token}"))
        .unwrap_or_else(|| format!("https://remota.quickdesk.tech/join/{token}"));

    Ok((token, join_url))
}

// ── Shared frame buffer ───────────────────────────────────────────────────────

#[derive(Default)]
pub struct FrameBuffer {
    pub data: Vec<u8>,  // BGRA
    pub width: u32,
    pub height: u32,
    pub dirty: bool,
}

// ── VP8 RTP depacketizer (RFC 7741) ──────────────────────────────────────────

struct Vp8Depacketizer { buf: Vec<u8> }

impl Vp8Depacketizer {
    fn new() -> Self { Self { buf: Vec::with_capacity(1 << 17) } }

    fn push(&mut self, payload: &[u8], marker: bool) -> Option<Vec<u8>> {
        if payload.is_empty() { return None; }
        let mut off = 0usize;
        let first = payload[off]; off += 1;
        if (first & 0x80) != 0 && off < payload.len() {
            let ext = payload[off]; off += 1;
            if (ext & 0x80) != 0 && off < payload.len() {
                let m = (payload[off] & 0x80) != 0; off += 1;
                if m && off < payload.len() { off += 1; }
            }
            if (ext & 0x40) != 0 && off < payload.len() { off += 1; }
            if ((ext & 0x20) != 0 || (ext & 0x10) != 0) && off < payload.len() { off += 1; }
        }
        if off >= payload.len() { return None; }
        self.buf.extend_from_slice(&payload[off..]);
        if marker { Some(std::mem::replace(&mut self.buf, Vec::with_capacity(1 << 17))) }
        else { None }
    }
}

// ── VP8 decoder ───────────────────────────────────────────────────────────────

struct Vp8Decoder { ctx: vpx_sys::vpx_codec_ctx_t }
unsafe impl Send for Vp8Decoder {}

impl Vp8Decoder {
    fn new() -> Result<Self> {
        use vpx_sys::*;
        let mut ctx: vpx_codec_ctx_t = unsafe { std::mem::zeroed() };
        let rc = unsafe {
            vpx_codec_dec_init_ver(&mut ctx, vpx_codec_vp8_dx(), std::ptr::null(), 0,
                VPX_DECODER_ABI_VERSION as i32)
        };
        anyhow::ensure!(rc == vpx_codec_err_t::VPX_CODEC_OK,
            "vpx_codec_dec_init failed: {}", rc as i32);
        Ok(Self { ctx })
    }

    fn decode(&mut self, data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
        use vpx_sys::*;
        let rc = unsafe {
            vpx_codec_decode(&mut self.ctx, data.as_ptr(), data.len() as u32,
                std::ptr::null_mut(), 0)
        };
        if rc != vpx_codec_err_t::VPX_CODEC_OK {
            warn!("[controller] decode: {}", rc as i32); return None;
        }
        let mut iter: vpx_codec_iter_t = std::ptr::null();
        let img = unsafe { vpx_codec_get_frame(&mut self.ctx, &mut iter) };
        if img.is_null() { return None; }
        let (w, h) = unsafe { ((*img).d_w, (*img).d_h) };
        Some((w, h, unsafe { i420_to_bgra(img, w, h) }))
    }
}

impl Drop for Vp8Decoder {
    fn drop(&mut self) { unsafe { vpx_sys::vpx_codec_destroy(&mut self.ctx); } }
}

unsafe fn i420_to_bgra(img: *const vpx_sys::vpx_image_t, w: u32, h: u32) -> Vec<u8> {
    use vpx_sys::*;
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * h * 4];
    let yp = (*img).planes[VPX_PLANE_Y as usize];
    let up = (*img).planes[VPX_PLANE_U as usize];
    let vp = (*img).planes[VPX_PLANE_V as usize];
    let ys = (*img).stride[VPX_PLANE_Y as usize] as usize;
    let us = (*img).stride[VPX_PLANE_U as usize] as usize;
    let vs = (*img).stride[VPX_PLANE_V as usize] as usize;
    for row in 0..h { for col in 0..w {
        let y = *yp.add(row * ys + col) as f32;
        let u = *up.add((row/2) * us + col/2) as f32;
        let v = *vp.add((row/2) * vs + col/2) as f32;
        let r = (y + 1.402*(v-128.0)).clamp(0.0,255.0) as u8;
        let g = (y - 0.344*(u-128.0) - 0.714*(v-128.0)).clamp(0.0,255.0) as u8;
        let b = (y + 1.772*(u-128.0)).clamp(0.0,255.0) as u8;
        let i = (row*w+col)*4;
        out[i]=b; out[i+1]=g; out[i+2]=r; out[i+3]=255;
    }}
    out
}

// ── Controller async entry point ──────────────────────────────────────────────

pub async fn run_controller_async(
    token: String,
    app_tx: std::sync::mpsc::SyncSender<crate::AppMsg>,
    term_rx: std::sync::mpsc::Receiver<()>,
    ctx: eframe::egui::Context,
) -> Result<()> {
    use tokio::sync::mpsc;

    let ws_url = format!("{}/ws", config::WS_URL);
    let (mut signal_rx, signal_tx) =
        sig_client::connect_as_controller(&ws_url, &token).await?;
    info!("[controller] signaling connected");

    let frame_buf: Arc<Mutex<FrameBuffer>> = Arc::new(Mutex::new(FrameBuffer::default()));
    let (input_tx, mut input_rx) = mpsc::channel::<String>(64);
    let (state_tx, mut state_rx) = mpsc::channel::<RTCPeerConnectionState>(8);
    let (ice_tx, mut ice_rx) = mpsc::channel::<String>(32);

    // WebRTC
    let mut media = MediaEngine::default();
    media.register_default_codecs()?;
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut media)?;
    let api = APIBuilder::new()
        .with_media_engine(media).with_interceptor_registry(registry).build();

    let pc = Arc::new(api.new_peer_connection(RTCConfiguration {
        ice_servers: crate::webrtc_session::ice_servers(), ..Default::default()
    }).await?);

    { let ice = ice_tx.clone();
      pc.on_ice_candidate(Box::new(move |c| { let ice = ice.clone();
        Box::pin(async move {
            if let Some(c) = c {
                if let Ok(init) = c.to_json() {
                    if let Ok(s) = serde_json::to_string(&init) {
                        let _ = ice.send(s).await;
                    }
                }
            }
        })
    })); }

    { let st = state_tx.clone();
      pc.on_peer_connection_state_change(Box::new(move |s| { let st = st.clone();
        Box::pin(async move { let _ = st.send(s).await; })
    })); }

    let dc = pc.create_data_channel("control", None).await?;
    // Clone for the send task. Both dc and dc_send must be kept alive for the
    // duration of the session — dropping dc would remove the strong reference
    // that the peer connection needs to keep the DataChannel open.
    let dc_send = Arc::clone(&dc);
    tokio::spawn(async move {
        while let Some(msg) = input_rx.recv().await {
            if dc_send.ready_state() == webrtc::data_channel::data_channel_state::RTCDataChannelState::Open {
                let _ = dc_send.send_text(msg).await;
            }
        }
    });

    pc.add_transceiver_from_kind(RTPCodecType::Video, Some(RTCRtpTransceiverInit {
        direction: RTCRtpTransceiverDirection::Recvonly, send_encodings: vec![],
    })).await?;

    { let fb = Arc::clone(&frame_buf); let ctx2 = ctx.clone();
      pc.on_track(Box::new(move |track, receiver, _| {
        let fb = Arc::clone(&fb); let ctx3 = ctx2.clone();
        Box::pin(async move {
            if track.kind() == RTPCodecType::Video {
                // Drain RTCP feedback (PLI, RR, etc.) from the receiver so the
                // interceptor pipeline does not back-pressure and stall RTP.
                tokio::spawn(async move {
                    let mut rtcp_buf = vec![0u8; 1500];
                    while receiver.read(&mut rtcp_buf).await.is_ok() {}
                });
                tokio::spawn(async move { run_decode_loop(track, fb, ctx3).await; });
            }
        })
    })); }

    { let sig = signal_tx.clone();
      tokio::spawn(async move {
        while let Some(cj) = ice_rx.recv().await {
            if let Ok(msg) = sig_client::make_controller_ice_msg(&cj) {
                let _ = sig.send(msg).await;
            }
        }
    }); }

    let mut started_offer = false;

    loop {
        // Check for UI-requested termination
        if term_rx.try_recv().is_ok() {
            let _ = signal_tx.send(r#"{"type":"terminate"}"#.to_owned()).await;
            break;
        }

        tokio::select! {
            Some(event) = signal_rx.recv() => match event {
                ControllerSignalEvent::ParticipantJoined => {
                    if started_offer { continue; }
                    started_offer = true;
                    // Spawn into a separate task so the main loop can continue
                    // processing trickle-ICE candidates from the participant
                    // while we wait for our own ICE gathering to complete.
                    let pc2 = Arc::clone(&pc);
                    let sig2 = signal_tx.clone();
                    tokio::spawn(async move {
                        let offer = match pc2.create_offer(None).await {
                            Ok(o) => o,
                            Err(e) => { error!("[controller] create_offer failed: {e}"); return; }
                        };
                        // In webrtc-rs 0.13, gathering_complete_promise() MUST be
                        // obtained BEFORE set_local_description. The internal
                        // mpsc sender fires on ICE completion — subscribe first or
                        // recv() will block forever having missed the signal.
                        let mut gather = pc2.gathering_complete_promise().await;
                        if let Err(e) = pc2.set_local_description(offer).await {
                            error!("[controller] set_local_description failed: {e}"); return;
                        }
                        let _ = gather.recv().await;
                        let local = match pc2.local_description().await {
                            Some(d) => d,
                            None => { error!("[controller] no local desc after gathering"); return; }
                        };
                        match sig_client::make_controller_offer_msg(&local) {
                            Ok(msg) => { let _ = sig2.send(msg).await; info!("[controller] offer sent"); }
                            Err(e) => error!("[controller] make_controller_offer_msg: {e}"),
                        }
                    });
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
                ControllerSignalEvent::Terminate | ControllerSignalEvent::Disconnected => break,
            },

            Some(state) = state_rx.recv() => match state {
                RTCPeerConnectionState::Connected => {
                    info!("[controller] WebRTC connected");
                    // Notify UI to transition to connected view
                    let _ = app_tx.try_send(crate::AppMsg::ControllerConnected {
                        frame_buf: Arc::clone(&frame_buf),
                        input_tx: input_tx.clone(),
                    });
                    ctx.request_repaint();
                }
                RTCPeerConnectionState::Failed => {
                    error!("[controller] WebRTC failed");
                    let _ = signal_tx.send(r#"{"type":"terminate"}"#.to_owned()).await;
                }
                _ => {}
            },
        }
    }

    pc.close().await?;
    Ok(())
}

// ── Decode loop ───────────────────────────────────────────────────────────────

async fn run_decode_loop(
    track: Arc<webrtc::track::track_remote::TrackRemote>,
    frame_buf: Arc<Mutex<FrameBuffer>>,
    ctx: eframe::egui::Context,
) {
    let mut depkt = Vp8Depacketizer::new();
    let mut decoder = match Vp8Decoder::new() {
        Ok(d) => d,
        Err(e) => { error!("[controller] decoder: {e}"); return; }
    };
    loop {
        match track.read_rtp().await {
            Ok((pkt, _)) => {
                if let Some(frame) = depkt.push(&pkt.payload, pkt.header.marker) {
                    if let Some((w, h, bgra)) = decoder.decode(&frame) {
                        let mut fb = frame_buf.lock().unwrap();
                        fb.data = bgra; fb.width = w; fb.height = h; fb.dirty = true;
                        ctx.request_repaint();
                    }
                }
            }
            Err(e) => { warn!("[controller] read_rtp: {e}"); break; }
        }
    }
}

// ── Signal event type ─────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ControllerSignalEvent {
    ParticipantJoined,
    Answer(String),
    IceCandidate(String),
    Terminate,
    Disconnected,
}
