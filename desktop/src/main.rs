// ── Remota Desktop ───────────────────────────────────────────────────────────
// Native Windows application.
// Opens a GUI launcher where the user chooses Controller or Participant mode.

#![windows_subsystem = "windows"]

mod capture;
mod config;
mod controller;
mod input;
mod protocol;
mod register;
mod signaling;
mod webrtc_session;

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use anyhow::{Context, Result};
use eframe::egui;
use tokio::sync::mpsc;
use tracing::{error, info};
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

fn main() {
    // Logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("remota_desktop=info".parse().unwrap()),
        )
        .init();

    // Register remota:// deep-link handler
    if let Err(e) = register::register_protocol_handler() {
        tracing::warn!("[main] protocol handler registration failed: {e}");
    }

    // Check for deep-link argument — participant mode via URL scheme
    let args: Vec<String> = std::env::args().collect();
    if let Some(arg) = args.get(1) {
        if let Some(token) = register::parse_deep_link(arg) {
            // Launched via remota:// URI — go straight to participant mode
            run_participant(token);
            return;
        }
    }

    // Show the launcher UI
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Remota")
            .with_inner_size([420.0, 280.0])
            .with_resizable(false),
        ..Default::default()
    };

    eframe::run_native(
        "Remota",
        options,
        Box::new(|_cc| Ok(Box::new(RemotaLauncher::default()))),
    ).unwrap();
}

// ── Launcher UI ───────────────────────────────────────────────────────────────

#[derive(Default)]
enum LauncherState {
    #[default]
    Home,
    Participant { token_input: String, error: Option<String> },
}

struct RemotaLauncher {
    state: LauncherState,
}

impl Default for RemotaLauncher {
    fn default() -> Self {
        Self { state: LauncherState::Home }
    }
}

impl eframe::App for RemotaLauncher {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(24.0);

            ui.vertical_centered(|ui| {
                ui.heading(egui::RichText::new("Remota").size(28.0).strong());
                ui.label(egui::RichText::new("Remote Desktop").size(14.0).color(egui::Color32::GRAY));
            });

            ui.add_space(32.0);

            match &mut self.state {
                LauncherState::Home => {
                    ui.vertical_centered(|ui| {
                        ui.label("What would you like to do?");
                        ui.add_space(16.0);

                        let btn_size = egui::vec2(240.0, 48.0);

                        if ui.add_sized(btn_size, egui::Button::new(
                            egui::RichText::new("🖥  Control a remote computer").size(15.0)
                        )).clicked() {
                            // Launch controller on background thread
                            std::thread::spawn(|| {
                                let rt = tokio::runtime::Runtime::new().unwrap();
                                if let Err(e) = rt.block_on(controller::run_controller()) {
                                    error!("[main] controller error: {e}");
                                }
                            });
                            // Close the launcher
                            std::process::exit(0);
                        }

                        ui.add_space(12.0);

                        if ui.add_sized(btn_size, egui::Button::new(
                            egui::RichText::new("🔗  Join a remote session").size(15.0)
                        )).clicked() {
                            self.state = LauncherState::Participant {
                                token_input: String::new(),
                                error: None,
                            };
                        }
                    });
                }

                LauncherState::Participant { token_input, error } => {
                    ui.vertical_centered(|ui| {
                        ui.label("Paste the session token from the link:");
                        ui.add_space(8.0);

                        let response = ui.add_sized(
                            egui::vec2(340.0, 32.0),
                            egui::TextEdit::singleline(token_input)
                                .hint_text("Session token…")
                        );

                        ui.add_space(8.0);

                        if let Some(err) = error.as_ref() {
                            ui.label(
                                egui::RichText::new(err).color(egui::Color32::RED).size(12.0)
                            );
                            ui.add_space(4.0);
                        }

                        let connect_clicked = ui.add_sized(
                            egui::vec2(240.0, 40.0),
                            egui::Button::new(egui::RichText::new("Connect").size(15.0))
                        ).clicked();

                        // Also allow Enter key
                        let enter_pressed = response.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter));

                        if connect_clicked || enter_pressed {
                            let token = token_input.trim().to_owned();
                            if token.is_empty() {
                                *error = Some("Please enter a session token.".to_owned());
                            } else {
                                let t = token.clone();
                                std::thread::spawn(move || run_participant(t));
                                std::process::exit(0);
                            }
                        }

                        ui.add_space(8.0);
                        if ui.small_button("← Back").clicked() {
                            self.state = LauncherState::Home;
                        }
                    });
                }
            }
        });
    }
}

// ── Participant mode ──────────────────────────────────────────────────────────

fn run_participant(token: String) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    if let Err(e) = rt.block_on(run_participant_async(token)) {
        error!("[participant] error: {e}");
    }
}

async fn run_participant_async(token: String) -> Result<()> {
    let ws_url = format!("{}/ws", config::WS_URL);

    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  Remota Desktop — connecting as participant");
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

    let force_keyframe = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (connected_tx, connected_rx) = tokio::sync::watch::channel(false);

    {
        let track = Arc::clone(&session.video_track);
        let kf = Arc::clone(&force_keyframe);
        tokio::spawn(async move {
            webrtc_session::run_encoding_loop(track, frame_rx, kf, connected_rx).await;
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

    // Show Windows notification that remote access is active
    show_active_notification();

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
                    signaling::SignalEvent::Terminate => {
                        info!("[main] session terminated by server");
                        break;
                    }
                    signaling::SignalEvent::Disconnected => {
                        info!("[main] signaling disconnected — exiting in 12s");
                        tokio::time::sleep(tokio::time::Duration::from_secs(12)).await;
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
                        force_keyframe.store(true, std::sync::atomic::Ordering::Relaxed);
                        let _ = connected_tx.send(true);
                        info!("[main] Connected — encoding started, keyframe requested");
                    }
                    RTCPeerConnectionState::Failed => {
                        info!("[main] WebRTC failed — requesting termination");
                        let _ = signal_tx.send(r#"{"type":"terminate"}"#.to_owned()).await;
                    }
                    _ => {}
                }
            }
        }
    }

    info!("[main] cleaning up…");
    input.release_all_modifiers();
    stop_capture.store(true, Ordering::Relaxed);
    session.close().await;
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
    (1920, 1080)
}

fn show_active_notification() {
    #[cfg(target_os = "windows")]
    std::thread::spawn(|| {
        use windows::Win32::UI::WindowsAndMessaging::{
            MessageBoxW, MB_OK, MB_ICONINFORMATION, MB_TOPMOST,
        };
        use windows::core::PCWSTR;

        let title: Vec<u16> = "Remota — Remote Access Active\0"
            .encode_utf16().collect();
        let msg: Vec<u16> =
            "Remote access is now active.\nThe other person can see your screen.\n\nClose Remota to end the session.\0"
            .encode_utf16().collect();

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
