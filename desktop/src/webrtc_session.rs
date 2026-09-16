// ── WebRTC peer connection ────────────────────────────────────────────────────
// Sets up the WebRTC answer side (participant / desktop endpoint):
//   • Adds a VP8 video track and feeds frames from the screen-capture pipeline
//   • Receives control events over the DataChannel
//   • Handles ICE trickle and connection-state changes

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use bytes::Bytes;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use webrtc::{
    api::{
        interceptor_registry::register_default_interceptors,
        media_engine::{MediaEngine, MIME_TYPE_VP8},
        APIBuilder,
    },
    ice_transport::{
        ice_candidate::RTCIceCandidateInit,
        ice_server::RTCIceServer,
    },
    interceptor::registry::Registry,
    media::Sample,
    peer_connection::{
        configuration::RTCConfiguration,
        peer_connection_state::RTCPeerConnectionState,
        sdp::session_description::RTCSessionDescription,
        RTCPeerConnection,
    },
    rtp_transceiver::rtp_codec::RTCRtpCodecCapability,
    track::track_local::{
        track_local_static_sample::TrackLocalStaticSample, TrackLocal,
    },
};
use vpx::{
    codec::VpxCodec,
    encoder::{Encoder, EncoderConfig, EncoderFlags},
};

use crate::capture::CapturedFrame;
use crate::protocol::ControlMessage;

// ── ICE server list ───────────────────────────────────────────────────────────

/// Build ICE server list from environment variables.
///
/// Required in env (or inherited from calling shell):
///   REMOTA_STUN_URL   (optional, defaults to Google STUN)
///   REMOTA_TURN_URL   e.g. turn:turn.remota.quickdesk.tech:3478
///   REMOTA_TURN_USER
///   REMOTA_TURN_PASS
pub fn ice_servers() -> Vec<RTCIceServer> {
    let stun_url = std::env::var("REMOTA_STUN_URL")
        .unwrap_or_else(|_| "stun:stun.l.google.com:19302".to_owned());

    let mut servers = vec![RTCIceServer {
        urls: vec![stun_url],
        ..Default::default()
    }];

    if let (Ok(url), Ok(user), Ok(pass)) = (
        std::env::var("REMOTA_TURN_URL"),
        std::env::var("REMOTA_TURN_USER"),
        std::env::var("REMOTA_TURN_PASS"),
    ) {
        servers.push(RTCIceServer {
            urls: vec![url],
            username: user,
            credential: pass,
            ..Default::default()
        });
    }

    servers
}

// ── Build API ─────────────────────────────────────────────────────────────────

pub fn build_api() -> Result<webrtc::api::API> {
    let mut media = MediaEngine::default();
    media
        .register_default_codecs()
        .context("register default codecs")?;

    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut media)
        .context("register interceptors")?;

    let api = APIBuilder::new()
        .with_media_engine(media)
        .with_interceptor_registry(registry)
        .build();

    Ok(api)
}

// ── Session ───────────────────────────────────────────────────────────────────

pub struct Session {
    pub pc: Arc<RTCPeerConnection>,
    pub video_track: Arc<TrackLocalStaticSample>,
}

impl Session {
    pub async fn new(
        api: &webrtc::api::API,
        control_tx: mpsc::Sender<ControlMessage>,
        ice_tx: mpsc::Sender<String>,
        state_tx: mpsc::Sender<RTCPeerConnectionState>,
    ) -> Result<Self> {
        let config = RTCConfiguration {
            ice_servers: ice_servers(),
            ..Default::default()
        };

        let pc = Arc::new(api.new_peer_connection(config).await?);

        // ── Video track (VP8) ─────────────────────────────────────────────────
        let video_track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_VP8.to_owned(),
                ..Default::default()
            },
            "video".to_owned(),
            "remota-screen".to_owned(),
        ));

        pc.add_track(Arc::clone(&video_track) as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .context("add video track")?;

        // ── DataChannel (inbound control) ─────────────────────────────────────
        {
            let ctrl = control_tx.clone();
            pc.on_data_channel(Box::new(move |dc| {
                let ctrl = ctrl.clone();
                Box::pin(async move {
                    info!("[webrtc] DataChannel open: {}", dc.label());
                    dc.on_message(Box::new(move |msg| {
                        let ctrl = ctrl.clone();
                        let data = msg.data.clone();
                        Box::pin(async move {
                            match serde_json::from_slice::<ControlMessage>(&data) {
                                Ok(m) => { let _ = ctrl.send(m).await; }
                                Err(e) => warn!("[webrtc] bad control msg: {e}"),
                            }
                        })
                    }));
                })
            }));
        }

        // ── ICE candidates ────────────────────────────────────────────────────
        {
            let ice = ice_tx.clone();
            pc.on_ice_candidate(Box::new(move |c| {
                let ice = ice.clone();
                Box::pin(async move {
                    if let Some(candidate) = c {
                        match candidate.to_json() {
                            Ok(init) => match serde_json::to_string(&init) {
                                Ok(s) => { let _ = ice.send(s).await; }
                                Err(e) => error!("[webrtc] ICE serialise: {e}"),
                            },
                            Err(e) => error!("[webrtc] ICE to_json: {e}"),
                        }
                    }
                })
            }));
        }

        // ── Connection state ──────────────────────────────────────────────────
        {
            let st = state_tx.clone();
            pc.on_peer_connection_state_change(Box::new(move |s| {
                let st = st.clone();
                Box::pin(async move {
                    info!("[webrtc] connection state → {s:?}");
                    let _ = st.send(s).await;
                })
            }));
        }

        Ok(Self { pc, video_track })
    }

    pub async fn handle_offer(&self, sdp: &str) -> Result<RTCSessionDescription> {
        let offer = RTCSessionDescription::offer(sdp.to_owned())
            .context("parse offer SDP")?;
        self.pc.set_remote_description(offer).await?;

        let answer = self.pc.create_answer(None).await?;
        let mut gather_complete = self.pc.gathering_complete_promise().await;
        self.pc.set_local_description(answer).await?;
        let _ = gather_complete.recv().await;

        self.pc
            .local_description()
            .await
            .context("no local description after gathering")
    }

    pub async fn add_ice_candidate(&self, candidate_json: &str) -> Result<()> {
        let init: RTCIceCandidateInit = serde_json::from_str(candidate_json)
            .context("parse ICE candidate JSON")?;
        self.pc.add_ice_candidate(init).await?;
        Ok(())
    }

    pub async fn close(&self) {
        if let Err(e) = self.pc.close().await {
            warn!("[webrtc] close error: {e}");
        }
    }
}

// ── VP8 encoding loop ─────────────────────────────────────────────────────────
// BGRA → I420 (YUV planar) → VP8 bitstream → WebRTC Sample

const TARGET_FPS: u32 = 30;
const TARGET_BITRATE_KBPS: u32 = 2000; // 2 Mbps — adjust as needed

pub async fn run_encoding_loop(
    track: Arc<TrackLocalStaticSample>,
    mut frame_rx: mpsc::Receiver<CapturedFrame>,
) {
    info!("[encode] encoding loop started ({TARGET_FPS} fps, {TARGET_BITRATE_KBPS} kbps)");

    // Encoder is initialised on the first frame so we know the dimensions.
    let mut encoder: Option<Encoder> = None;
    let mut frame_idx: u64 = 0;
    let frame_duration = Duration::from_millis(1000 / TARGET_FPS as u64);

    while let Some(frame) = frame_rx.recv().await {
        let w = frame.width;
        let h = frame.height;

        // (Re-)initialise encoder if first frame or dimensions changed
        let enc = match encoder {
            Some(ref mut e) if e.config().width == w && e.config().height == h => e,
            _ => {
                match build_encoder(w, h) {
                    Ok(e) => {
                        info!("[encode] VP8 encoder initialised ({w}×{h})");
                        encoder = Some(e);
                        encoder.as_mut().unwrap()
                    }
                    Err(e) => {
                        error!("[encode] failed to build encoder: {e}");
                        continue;
                    }
                }
            }
        };

        // Convert BGRA → I420
        let i420 = bgra_to_i420(&frame.data, w, h);

        // Encode
        let pts = frame_idx as i64;
        let flags = if frame_idx % (TARGET_FPS as u64 * 2) == 0 {
            EncoderFlags::FORCE_KF // keyframe every 2 seconds
        } else {
            EncoderFlags::empty()
        };

        match enc.encode(pts, &i420, w, h, frame_duration, flags) {
            Ok(packets) => {
                for pkt in packets {
                    let data = Bytes::copy_from_slice(&pkt.data);
                    if data.is_empty() {
                        continue;
                    }
                    let sample = Sample {
                        data,
                        duration: frame_duration,
                        ..Default::default()
                    };
                    if let Err(e) = track.write_sample(&sample).await {
                        info!("[encode] write_sample ended: {e}");
                        return;
                    }
                }
            }
            Err(e) => {
                warn!("[encode] encode error: {e}");
            }
        }

        frame_idx += 1;
    }

    info!("[encode] encoding loop exited");
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn build_encoder(width: u32, height: u32) -> Result<Encoder> {
    let mut cfg = EncoderConfig::new(width, height)?;
    cfg.set_bitrate(TARGET_BITRATE_KBPS)?;
    cfg.set_timebase(1, TARGET_FPS)?;
    cfg.set_threads(num_cpus_available() as u32)?;
    Encoder::new(VpxCodec::VP8, cfg, EncoderFlags::empty())
        .context("create VP8 encoder")
}

fn num_cpus_available() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().min(4))
        .unwrap_or(2)
}

/// Convert packed BGRA to planar I420 (YUV 4:2:0).
/// Input: width × height × 4 bytes (B G R A order).
/// Output: width × height × 3/2 bytes (Y plane, then U, then V half-res).
fn bgra_to_i420(bgra: &[u8], width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let y_size = w * h;
    let uv_size = (w / 2) * (h / 2);
    let mut out = vec![0u8; y_size + 2 * uv_size];

    let (y_plane, uv) = out.split_at_mut(y_size);
    let (u_plane, v_plane) = uv.split_at_mut(uv_size);

    for row in 0..h {
        for col in 0..w {
            let i = (row * w + col) * 4;
            let b = bgra[i] as f32;
            let g = bgra[i + 1] as f32;
            let r = bgra[i + 2] as f32;

            // BT.601 coefficients
            let y = (16.0 + 65.481 * r / 255.0 + 128.553 * g / 255.0 + 24.966 * b / 255.0)
                .round()
                .clamp(0.0, 255.0) as u8;
            y_plane[row * w + col] = y;

            if row % 2 == 0 && col % 2 == 0 {
                let uv_idx = (row / 2) * (w / 2) + col / 2;
                let u = (128.0 - 37.797 * r / 255.0 - 74.203 * g / 255.0 + 112.0 * b / 255.0)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                let v = (128.0 + 112.0 * r / 255.0 - 93.786 * g / 255.0 - 18.214 * b / 255.0)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                u_plane[uv_idx] = u;
                v_plane[uv_idx] = v;
            }
        }
    }

    out
}
