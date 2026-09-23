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
use vpx_encode::{Config, Encoder, Error as VpxError, VideoCodecId};

use crate::capture::CapturedFrame;
use crate::config;
use crate::protocol::ControlMessage;

// ── ICE server list ───────────────────────────────────────────────────────────

/// Build ICE server list from compile-time config constants.
/// STUN always included. TURN added only when all three vars were set at build time.
pub fn ice_servers() -> Vec<RTCIceServer> {
    let mut servers = vec![RTCIceServer {
        urls: vec!["stun:stun.l.google.com:19302".to_owned()],
        ..Default::default()
    }];

    if let (Some(url), Some(user), Some(pass)) =
        (config::TURN_URL, config::TURN_USER, config::TURN_PASS)
    {
        servers.push(RTCIceServer {
            urls: vec![url.to_owned()],
            username: user.to_owned(),
            credential: pass.to_owned(),
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

        // Track is added in handle_offer() after set_remote_description()
        // so it correctly maps to the controller's recvonly transceiver slot.

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

        info!("[webrtc] offer SDP:\n{}", sdp);

        self.pc.set_remote_description(offer).await?;

        self.pc
            .add_track(Arc::clone(&self.video_track) as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .context("add video track in handle_offer")?;

        let answer = self.pc.create_answer(None).await?;
        let mut gather_complete = self.pc.gathering_complete_promise().await;
        self.pc.set_local_description(answer).await?;
        let _ = gather_complete.recv().await;

        let local = self.pc
            .local_description()
            .await
            .context("no local description after gathering")?;

        info!("[webrtc] answer SDP:\n{}", local.sdp);

        Ok(local)
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
const TARGET_BITRATE_KBPS: u32 = 2000;

/// Spawns a dedicated OS thread for VP8 encoding (encoder is !Send so it cannot
/// live in a tokio task across await points). The thread encodes frames and sends
/// the compressed bytes to an async task via a channel which calls write_sample.
///
/// Encoding does not begin until `connected_rx` is true. This ensures the RTP
/// sender is bound before any write_sample call, avoiding pre-connection errors
/// that would otherwise accumulate and kill the write task.
pub async fn run_encoding_loop(
    track: Arc<TrackLocalStaticSample>,
    mut frame_rx: mpsc::Receiver<CapturedFrame>,
    force_keyframe: Arc<std::sync::atomic::AtomicBool>,
    mut connected_rx: tokio::sync::watch::Receiver<bool>,
) {
    info!("[encode] encoding loop started ({TARGET_FPS} fps, {TARGET_BITRATE_KBPS} kbps)");

    // Wait until the peer connection reaches Connected before consuming any frames.
    // This prevents write_sample from being called before the RTP sender is bound.
    if !*connected_rx.borrow() {
        info!("[encode] waiting for WebRTC Connected before encoding…");
        if connected_rx.changed().await.is_err() {
            info!("[encode] connected_rx closed before Connected — exiting");
            return;
        }
    }
    // Drain any frames that accumulated while we were waiting — they are stale
    // and would produce delta packets that cannot be decoded without a keyframe.
    while frame_rx.try_recv().is_ok() {}
    info!("[encode] WebRTC Connected — starting encode/write pipeline");

    let frame_duration = Duration::from_millis(1000 / TARGET_FPS as u64);

    // Channel: encoding thread → async write task
    let (encoded_tx, mut encoded_rx) = mpsc::channel::<Bytes>(8);

    // Capture the tokio runtime handle before entering the std thread
    let handle = tokio::runtime::Handle::current();

    // Encoding thread
    std::thread::spawn(move || {
        let mut encoder: Option<Encoder> = None;
        let mut last_w: u32 = 0;
        let mut last_h: u32 = 0;
        let mut frame_idx: i64 = 0;
        let mut frames_received: u64 = 0;
        let mut packets_sent: u64 = 0;
        let mut frames_skipped: u64 = 0;

        while let Some(frame) = handle.block_on(frame_rx.recv()) {
            frames_received += 1;
            let log_this = frames_received == 1 || frames_received % 150 == 0;

            let capture_w = frame.width;
            let capture_h = frame.height;
            let capture_data_len = frame.data.len();

            let w = capture_w & !1;
            let h = capture_h & !1;
            if w == 0 || h == 0 {
                frames_skipped += 1;
                continue;
            }

            // DIAGNOSTIC: verify frame data size matches encoder dimensions
            let expected_bgra = (w as usize) * (h as usize) * 4;
            if frame.data.len() < expected_bgra {
                frames_skipped += 1;
                if log_this {
                    warn!("[encode] SKIP frame {frames_received}: data={capture_data_len}B expected={expected_bgra}B (capture={capture_w}x{capture_h} encoder={w}x{h})");
                }
                continue;
            }

            if log_this {
                info!("[encode] frame {frames_received}: capture={capture_w}x{capture_h} encoder={w}x{h} data={capture_data_len}B expected={expected_bgra}B packets_sent_so_far={packets_sent}");
            }

            let i420 = bgra_to_i420(&frame.data, w, h);

            // If a keyframe was requested (e.g. on WebRTC connect), reset the encoder.
            // A fresh encoder always produces a keyframe on its first encode call.
            if force_keyframe.swap(false, std::sync::atomic::Ordering::Relaxed) {
                info!("[encode] forced keyframe — resetting encoder");
                last_w = 0; // trigger encoder rebuild on next iteration check
                frame_idx = 0;
            }

            // Rebuild encoder if needed (dimensions changed or forced keyframe reset)
            if encoder.is_none() || w != last_w || h != last_h {
                match build_encoder(w, h) {
                    Ok(e) => {
                        info!("[encode] encoder (re)built ({w}x{h})");
                        encoder = Some(e);
                        last_w = w;
                        last_h = h;
                    }
                    Err(e) => {
                        error!("[encode] encoder build failed: {e}");
                        continue;
                    }
                }
            }

            let enc = encoder.as_mut().unwrap();
            let pts_us = frame_idx * (1_000_000 / TARGET_FPS as i64);

            match enc.encode(pts_us, i420.as_slice()) {
                Ok(packets) => {
                    let mut pkt_count = 0;
                    for pkt in packets {
                        let data = Bytes::copy_from_slice(pkt.data);
                        if !data.is_empty() {
                            pkt_count += 1;
                            packets_sent += 1;
                            // DIAGNOSTIC: log first few packet sizes to confirm data
                            if packets_sent <= 3 {
                                info!("[encode] PACKET {packets_sent}: {} bytes (key={})", data.len(), pkt.key);
                            }
                            if encoded_tx.blocking_send(data).is_err() {
                                info!("[encode] encoded_tx channel closed — receiver dropped");
                                return;
                            }
                        }
                    }
                    if pkt_count == 0 && log_this {
                        info!("[encode] frame {frames_received} → 0 packets (encoder buffering)");
                    }
                }
                Err(e) => {
                    if frame_idx < 5 || log_this {
                        error!("[encode] frame {frames_received} encode error: {e:?}");
                    }
                }
            }

            frame_idx += 1;
        }
        info!("[encode] encoding thread exited — frames_received={frames_received} packets_sent={packets_sent} frames_skipped={frames_skipped}");
    });

    // Async write task — receives encoded bytes and writes to the WebRTC track.
    // Errors from write_sample are logged but never cause an early exit: before
    // the peer connection reaches Connected the RTP sender is not yet bound and
    // write_sample returns ErrRTPSenderNotReady (or similar). Those errors are
    // expected and harmless — the encoding thread will reset the encoder on
    // force_keyframe anyway, so the browser always gets a clean keyframe once
    // the connection is actually up.
    let mut write_count: u64 = 0;
    let mut write_error_count: u64 = 0;
    while let Some(data) = encoded_rx.recv().await {
        write_count += 1;
        let pkt_size = data.len();
        let sample = Sample {
            data,
            duration: frame_duration,
            ..Default::default()
        };
        if write_count <= 5 || write_count % 150 == 0 {
            info!("[encode] write_sample #{write_count}: {pkt_size}B → track");
        }
        if let Err(e) = track.write_sample(&sample).await {
            write_error_count += 1;
            if write_error_count <= 3 || write_count % 150 == 0 {
                error!("[encode] write_sample #{write_count} FAILED: {e}");
            }
        }
    }

    info!("[encode] encoding loop exited — write_count={write_count} write_error_count={write_error_count}");
}

fn build_encoder(width: u32, height: u32) -> Result<Encoder, VpxError> {
    let cfg = Config {
        width,
        height,
        // Use microsecond timebase (1/1000000) — this is the libvpx default
        // and is known to work. timebase [1, 30] causes VPX_CODEC_INVALID_PARAM
        // with libvpx 1.14 on Windows.
        timebase: [1, 1_000_000],
        bitrate: TARGET_BITRATE_KBPS,
        codec: VideoCodecId::VP8,
    };
    Encoder::new(cfg)
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
