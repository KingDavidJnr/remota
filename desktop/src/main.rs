// ── Remota Desktop Endpoint ───────────────────────────────────────────────────
// Windows native application.
// Runs without a console window — status shown via Windows notifications.

#![windows_subsystem = "windows"]
//
// The user does not need to configure anything. The server URL and TURN
// credentials are baked into the binary at build time.
//
// Invocation modes (in order of priority):
//
//   1. Deep-link — browser clicks "Launch Remota Desktop":
//        remota-desktop.exe remota://session/<token>
//
//   2. Manual token entry — user runs the exe and is prompted:
//        remota-desktop.exe <token>
//
// On first run the app registers the remota:// URI scheme in HKCU so that
// subsequent deep-link clicks open this executable automatically.

mod capture;
mod config;
mod input;
mod protocol;
mod register;
mod signaling;
mod webrtc_session;

use std::{
    io::{self, BufRead, Write},
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

    // ── Register remota:// protocol handler ───────────────────────────────────
    if let Err(e) = register::register_protocol_handler() {
        tracing::warn!("[main] protocol handler registration failed: {e}");
    }

    // ── Resolve token ─────────────────────────────────────────────────────────
    // The WS URL is always baked in. The token comes from:
    //   a) the deep-link URI passed as arg 1
    //   b) a plain token passed as arg 1
    //   c) prompted interactively if no args given
    let args: Vec<String> = std::env::args().collect();
    let token = match args.get(1) {
        Some(arg) => {
            // Try to parse as a deep-link URI first
            if let Some(t) = register::parse_deep_link(arg) {
                t
            } else {
                // Treat bare arg as a token directly
                arg.clone()
            }
        }
        None => {
            // No argument — prompt the user to paste the token
            print!("Enter session token: ");
            io::stdout().flush().ok();
            let mut line = String::new();
            io::stdin()
                .lock()
                .read_line(&mut line)
                .context("Failed to read token from stdin")?;
            let t = line.trim().to_owned();
            if t.is_empty() {
                anyhow::bail!("No token provided");
            }
            t
        }
    };

    let ws_url = format!("{}/ws", config::WS_URL);

    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  Remota Desktop — connecting");
    info!("  Server : {}", config::WS_URL);
    info!("  Token  : {token}");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    run(&ws_url, &token).await
}

async fn run(ws_url: &str, token: &str) -> Result<()> {
    let (screen_w, screen_h) = get_primary_screen_dimensions();
    info!("[main] screen {screen_w}×{screen_h}");

    let (frame_tx, frame_rx) = mpsc::channel::<capture::CapturedFrame>(4);
    let (control_tx, mut control_rx) = mpsc::channel::<protocol::ControlMessage>(64);
    let (ice_event_tx, mut ice_event_rx) = mpsc::channel::<String>(32);
    let (state_tx, mut state_rx) = mpsc::channel::<RTCPeerConnectionState>(8);

    let (mut signal_rx, signal_tx) = signaling::connect(ws_url, token).await?;
    info!("[main] signaling connected, waiting for offer…");

    let api = webrtc_session::build_api()?;
    let session = Arc::new(
        webrtc_session::Session::new(&api, control_tx, ice_event_tx, state_tx).await?,
    );

    let stop_capture = Arc::new(AtomicBool::new(false));
    capture::start(frame_tx, Arc::clone(&stop_capture))?;

    {
        let track = Arc::clone(&session.video_track);
        tokio::spawn(async move {
            webrtc_session::run_encoding_loop(track, frame_rx).await;
        });
    }

    let mut input = input::InputController::new(screen_w, screen_h)?;

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
                        let _ = signal_tx.send(r#"{"type":"active"}"#.to_owned()).await;
                        // Show a Windows notification balloon so the participant
                        // knows remote control is active even without a console
                        show_active_notification();
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

    info!("[main] cleaning up — releasing all input...");
    input.release_all_modifiers();
    stop_capture.store(true, Ordering::Relaxed);
    session.close().await;
    let _ = signal_tx.send(signaling::make_terminate_msg()).await;

    // Force process exit — with windows_subsystem = "windows" there is no
    // console or window to close, so we must exit explicitly.
    std::process::exit(0);
}

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

    #[cfg(target_os = "macos")]
    {
        use screencapturekit::prelude::SCShareableContent;
        if let Ok(content) = SCShareableContent::get() {
            if let Some(display) = content.displays().into_iter().next() {
                return (display.width() as u32, display.height() as u32);
            }
        }
    }

    (1920, 1080)
}

/// Shows a Windows MessageBox informing the participant remote control is active.
/// Non-blocking — spawned on a background thread so it doesn't block the event loop.
fn show_active_notification() {
    #[cfg(target_os = "windows")]
    std::thread::spawn(|| {
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK, MB_ICONINFORMATION, MB_TOPMOST};
        use windows::core::PCWSTR;

        let title: Vec<u16> = "Remota — Remote Access Active\0".encode_utf16().collect();
        let msg: Vec<u16> = "Remote access is now active.\nThe other person can see your screen.\n\nClose the Remota Desktop app to end the session.\0"
            .encode_utf16()
            .collect();

        unsafe {
            MessageBoxW(
                None,
                PCWSTR(msg.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OK | MB_ICONINFORMATION | MB_TOPMOST,
            );
        }
    });
}
