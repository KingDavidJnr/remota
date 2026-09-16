// ── Remota Desktop Endpoint ───────────────────────────────────────────────────
// Windows native application — runs as a visible console process so the
// participant can see status messages and confirm remote control is active.
//
// Invocation modes:
//
//   1. Deep-link (browser launches the app automatically):
//        remota-desktop.exe remota://session/<token>
//        REMOTA_WS_URL must be set in the environment.
//
//   2. Manual (CLI):
//        remota-desktop.exe <BACKEND_WS_URL> <ROOM_TOKEN>
//
//   3. Environment only:
//        REMOTA_WS_URL=wss://... REMOTA_TOKEN=<token> remota-desktop.exe
//
// Optional environment variables:
//   REMOTA_TURN_URL     turn:turn.remota.quickdesk.tech:3478
//   REMOTA_TURN_USER    <coturn username>
//   REMOTA_TURN_PASS    <coturn credential>
//
// On first run the app registers the remota:// URI scheme in HKCU so that
// subsequent deep-link clicks open this executable automatically.

mod capture;
mod input;
mod protocol;
mod register;
mod signaling;
mod webrtc_session;

use std::{
    env,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tracing::{error, info};
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

#[tokio::main]
async fn main() -> Result<()> {
    // ── Logging ───────────────────────────────────────────────────────────────
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("remota_desktop=debug".parse().unwrap()),
        )
        .init();

    // ── Register remota:// protocol handler (no-op if already registered) ────
    if let Err(e) = register::register_protocol_handler() {
        // Non-fatal — app still works without the deep-link handler
        tracing::warn!("[main] protocol handler registration failed: {e}");
    }

    // ── Config ────────────────────────────────────────────────────────────────
    // Arg 1 may be either:
    //   a)  A deep-link URI:  remota://session/<token>
    //   b)  The WS base URL:  wss://api.remota.quickdesk.tech  (legacy / manual)
    let args: Vec<String> = env::args().collect();
    let arg1 = args.get(1).cloned();

    // Detect deep-link invocation from the browser
    let (ws_base, token) = if let Some(ref uri) = arg1 {
        if let Some(token) = register::parse_deep_link(uri) {
            // Launched via remota://session/<token> — WS URL comes from env
            let ws = env::var("REMOTA_WS_URL")
                .context("REMOTA_WS_URL must be set when launching via deep-link")?;
            (ws, token)
        } else {
            // Manual invocation: remota-desktop.exe <WS_URL> <TOKEN>
            let ws = arg1.unwrap();
            let tok = args
                .get(2)
                .cloned()
                .or_else(|| env::var("REMOTA_TOKEN").ok())
                .context("Pass room token as second argument or set REMOTA_TOKEN")?;
            (ws, tok)
        }
    } else {
        // No args — fall back entirely to env vars
        let ws = env::var("REMOTA_WS_URL")
            .context("Pass WS URL as first argument or set REMOTA_WS_URL")?;
        let tok = env::var("REMOTA_TOKEN")
            .context("Pass room token as second argument or set REMOTA_TOKEN")?;
        (ws, tok)
    };

    let ws_url = format!("{ws_base}/ws");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  Remota Desktop — connecting");
    info!("  Server : {ws_base}");
    info!("  Token  : {token}");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    run(&ws_url, &token).await
}

async fn run(ws_url: &str, token: &str) -> Result<()> {
    // ── Screen dimensions ─────────────────────────────────────────────────────
    let (screen_w, screen_h) = get_primary_screen_dimensions();
    info!("[main] screen {screen_w}×{screen_h}");

    // ── Channels ──────────────────────────────────────────────────────────────
    let (frame_tx, frame_rx) = mpsc::channel::<capture::CapturedFrame>(4);
    let (control_tx, mut control_rx) = mpsc::channel::<protocol::ControlMessage>(64);
    let (ice_event_tx, mut ice_event_rx) = mpsc::channel::<String>(32);
    let (state_tx, mut state_rx) = mpsc::channel::<RTCPeerConnectionState>(8);

    // ── Signaling ─────────────────────────────────────────────────────────────
    let (mut signal_rx, signal_tx) = signaling::connect(ws_url, token).await?;
    info!("[main] signaling connected, waiting for offer…");

    // ── WebRTC ────────────────────────────────────────────────────────────────
    let api = webrtc_session::build_api()?;
    let session = Arc::new(
        webrtc_session::Session::new(&api, control_tx, ice_event_tx, state_tx).await?,
    );

    // ── Screen capture ────────────────────────────────────────────────────────
    let stop_capture = Arc::new(AtomicBool::new(false));
    capture::start(frame_tx, Arc::clone(&stop_capture))?;

    // ── Encoding loop ─────────────────────────────────────────────────────────
    {
        let track = Arc::clone(&session.video_track);
        tokio::spawn(async move {
            webrtc_session::run_encoding_loop(track, frame_rx).await;
        });
    }

    // ── Input controller ──────────────────────────────────────────────────────
    let mut input = input::InputController::new(screen_w, screen_h)?;

    // ── Forward local ICE candidates to signaling ─────────────────────────────
    {
        let sig = signal_tx.clone();
        tokio::spawn(async move {
            while let Some(candidate_json) = ice_event_rx.recv().await {
                match signaling::make_ice_msg(&candidate_json) {
                    Ok(msg) => { let _ = sig.send(msg).await; }
                    Err(e) => error!("[main] ICE msg error: {e}"),
                }
            }
        });
    }

    // ── Main event loop ───────────────────────────────────────────────────────
    loop {
        tokio::select! {
            Some(event) = signal_rx.recv() => {
                match event {
                    signaling::SignalEvent::Offer(sdp) => {
                        info!("[main] offer received — creating answer");
                        match session.handle_offer(&sdp).await {
                            Ok(answer) => {
                                match signaling::make_answer_msg(&answer) {
                                    Ok(msg) => { let _ = signal_tx.send(msg).await; }
                                    Err(e) => error!("[main] answer msg: {e}"),
                                }
                            }
                            Err(e) => error!("[main] handle_offer: {e}"),
                        }
                    }

                    signaling::SignalEvent::IceCandidate(candidate_json) => {
                        if let Err(e) = session.add_ice_candidate(&candidate_json).await {
                            error!("[main] add_ice_candidate: {e}");
                        }
                    }

                    signaling::SignalEvent::Terminate | signaling::SignalEvent::Disconnected => {
                        info!("[main] session terminated by signal");
                        break;
                    }
                }
            }

            Some(msg) = control_rx.recv() => {
                input.dispatch(msg);
            }

            Some(state) = state_rx.recv() => {
                match state {
                    RTCPeerConnectionState::Connected => {
                        info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                        info!("  ● REMOTE CONTROL IS NOW ACTIVE");
                        info!("  The other person can see and control your screen.");
                        info!("  Close this window or press Ctrl+C to end the session.");
                        info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                        // Notify server so it can set room ACTIVE and forward to controller
                        let _ = signal_tx.send(r#"{"type":"active"}"#.to_owned()).await;
                    }
                    RTCPeerConnectionState::Failed
                    | RTCPeerConnectionState::Disconnected
                    | RTCPeerConnectionState::Closed => {
                        info!("[main] WebRTC connection ended ({state:?})");
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    // ── Cleanup (fail-closed) ─────────────────────────────────────────────────
    info!("[main] cleaning up — releasing all input…");
    input.release_all_modifiers();
    stop_capture.store(true, Ordering::Relaxed);
    session.close().await;
    let _ = signal_tx.send(signaling::make_terminate_msg()).await;

    info!("[main] session ended. You may close this window.");
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn get_primary_screen_dimensions() -> (u32, u32) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
        };
        let w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        if w > 0 && h > 0 {
            return (w as u32, h as u32);
        }
    }
    (1920, 1080)
}
