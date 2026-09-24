// ── Remota Desktop ───────────────────────────────────────────────────────────
// Single-window native application.
// All modes (home, controller, participant) run in one eframe window.

#![windows_subsystem = "windows"]

mod capture;
mod config;
mod controller;
mod input;
mod protocol;
mod register;
mod signaling;
mod webrtc_session;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use eframe::egui;
use tracing::error;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("remota_desktop=info".parse().unwrap()),
        )
        .init();

    if let Err(e) = register::register_protocol_handler() {
        tracing::warn!("[main] protocol handler registration failed: {e}");
    }

    // Deep-link launch — go straight to participant mode with token
    let args: Vec<String> = std::env::args().collect();
    let deep_link_token = args.get(1).and_then(|a| register::parse_deep_link(a));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Remota")
            .with_inner_size([520.0, 340.0])
            .with_resizable(true),
        ..Default::default()
    };

    eframe::run_native(
        "Remota",
        options,
        Box::new(move |_cc| Ok(Box::new(RemotaApp::new(deep_link_token)))),
    ).unwrap();
}

// ── App state ─────────────────────────────────────────────────────────────────

enum AppState {
    Home,

    // Controller waiting for participant
    ControllerWaiting {
        token: String,
        join_url: String,
        /// Channel to tell the background task to terminate
        terminate_tx: std::sync::mpsc::SyncSender<()>,
    },

    // Controller — remote screen active
    ControllerConnected {
        frame_buf: Arc<Mutex<controller::FrameBuffer>>,
        input_tx: tokio::sync::mpsc::Sender<String>,
        terminate_tx: std::sync::mpsc::SyncSender<()>,
        texture: Option<egui::TextureHandle>,
    },

    // Participant — enter token
    ParticipantInput {
        token_input: String,
        error: Option<String>,
    },

    // Participant — session active (screen being shared)
    ParticipantActive {
        /// Channel to tell the background task to terminate
        terminate_tx: std::sync::mpsc::SyncSender<()>,
    },

    // Session ended
    Ended { message: String },
}

// ── Channels from background tasks to UI ─────────────────────────────────────

enum AppMsg {
    /// Controller: participant joined, WebRTC connected, start showing screen
    ControllerConnected {
        frame_buf: Arc<Mutex<controller::FrameBuffer>>,
        input_tx: tokio::sync::mpsc::Sender<String>,
    },
    /// Session ended (either side)
    SessionEnded,
}

// ── App ───────────────────────────────────────────────────────────────────────

struct RemotaApp {
    state: AppState,
    msg_rx: std::sync::mpsc::Receiver<AppMsg>,
    msg_tx: std::sync::mpsc::SyncSender<AppMsg>,
}

impl RemotaApp {
    fn new(deep_link_token: Option<String>) -> Self {
        let (msg_tx, msg_rx) = std::sync::mpsc::sync_channel(8);
        let state = match deep_link_token {
            Some(token) => AppState::ParticipantInput {
                token_input: token,
                error: None,
            },
            None => AppState::Home,
        };
        Self { state, msg_rx, msg_tx }
    }
}

impl eframe::App for RemotaApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll messages from background tasks
        while let Ok(msg) = self.msg_rx.try_recv() {
            match msg {
                AppMsg::ControllerConnected { frame_buf, input_tx } => {
                    // Grab the terminate_tx from the current waiting state
                    let terminate_tx = match &self.state {
                        AppState::ControllerWaiting { terminate_tx, .. } => terminate_tx.clone(),
                        _ => continue,
                    };
                    // Resize window for remote viewing
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                        egui::vec2(1280.0, 760.0)
                    ));
                    self.state = AppState::ControllerConnected {
                        frame_buf,
                        input_tx,
                        terminate_tx,
                        texture: None,
                    };
                }
                AppMsg::SessionEnded => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                        egui::vec2(520.0, 340.0)
                    ));
                    self.state = AppState::Ended {
                        message: "Session ended.".to_owned(),
                    };
                }
            }
        }

        ctx.request_repaint_after(std::time::Duration::from_millis(16));

        egui::CentralPanel::default().show(ctx, |ui| {
            match &mut self.state {
                // ── Home ──────────────────────────────────────────────────────
                AppState::Home => {
                    ui.vertical_centered(|ui| {
                        ui.add_space(36.0);
                        ui.heading(egui::RichText::new("Remota").size(30.0).strong());
                        ui.label(egui::RichText::new("Remote Desktop")
                            .size(14.0).color(egui::Color32::GRAY));
                        ui.add_space(36.0);
                        ui.label("What would you like to do?");
                        ui.add_space(16.0);

                        let btn = egui::vec2(260.0, 50.0);

                        let ctrl_clicked = ui.add_sized(btn, egui::Button::new(
                            egui::RichText::new("🖥  Control a remote computer").size(15.0)
                        )).clicked();

                        ui.add_space(12.0);

                        let join_clicked = ui.add_sized(btn, egui::Button::new(
                            egui::RichText::new("🔗  Join a remote session").size(15.0)
                        )).clicked();

                        if ctrl_clicked {
                            self.start_controller(ctx);
                        }
                        if join_clicked {
                            self.state = AppState::ParticipantInput {
                                token_input: String::new(),
                                error: None,
                            };
                        }
                    });
                }

                // ── Controller waiting ────────────────────────────────────────
                AppState::ControllerWaiting { token, join_url, terminate_tx } => {
                    let terminate_tx = terminate_tx.clone();
                    let token = token.clone();
                    let join_url = join_url.clone();

                    ui.vertical_centered(|ui| {
                        ui.add_space(24.0);
                        ui.heading(egui::RichText::new("Waiting for participant…").size(20.0));
                        ui.add_space(24.0);

                        ui.label(egui::RichText::new("Session token — paste this in the Remota app:")
                            .color(egui::Color32::GRAY).size(13.0));
                        ui.add_space(4.0);

                        let mut t = token.clone();
                        ui.add(egui::TextEdit::singleline(&mut t)
                            .desired_width(420.0)
                            .font(egui::TextStyle::Monospace));
                        ui.add_space(4.0);
                        if ui.button("📋  Copy token").clicked() {
                            ctx.copy_text(token.clone());
                        }

                        ui.add_space(16.0);
                        ui.separator();
                        ui.add_space(12.0);

                        ui.label(egui::RichText::new("Or share this invite link:")
                            .color(egui::Color32::GRAY).size(13.0));
                        ui.add_space(4.0);

                        let mut u = join_url.clone();
                        ui.add(egui::TextEdit::singleline(&mut u)
                            .desired_width(420.0)
                            .font(egui::TextStyle::Monospace));
                        ui.add_space(4.0);
                        if ui.button("📋  Copy link").clicked() {
                            ctx.copy_text(join_url.clone());
                        }

                        ui.add_space(24.0);
                        if ui.add_sized(egui::vec2(160.0, 36.0),
                            egui::Button::new(egui::RichText::new("End").size(14.0))
                                .fill(egui::Color32::from_rgb(180, 40, 40))
                        ).clicked() {
                            let _ = terminate_tx.try_send(());
                            self.state = AppState::Home;
                        }
                    });
                }

                // ── Controller connected — show remote screen ─────────────────
                AppState::ControllerConnected { frame_buf, input_tx, terminate_tx, texture } => {
                    // Upload new decoded frame
                    {
                        let mut fb = frame_buf.lock().unwrap();
                        if fb.dirty && fb.width > 0 && fb.height > 0 {
                            fb.dirty = false;
                            // BGRA → RGBA
                            let mut rgba = vec![0u8; fb.data.len()];
                            for i in (0..fb.data.len()).step_by(4) {
                                if i + 3 < fb.data.len() {
                                    rgba[i]   = fb.data[i+2];
                                    rgba[i+1] = fb.data[i+1];
                                    rgba[i+2] = fb.data[i];
                                    rgba[i+3] = 255;
                                }
                            }
                            let image = egui::ColorImage::from_rgba_unmultiplied(
                                [fb.width as usize, fb.height as usize], &rgba,
                            );
                            match texture {
                                Some(t) => t.set(image, egui::TextureOptions::LINEAR),
                                None => {
                                    *texture = Some(ctx.load_texture(
                                        "remote-screen", image, egui::TextureOptions::LINEAR,
                                    ));
                                }
                            }
                        }
                    }

                    // Top bar
                    egui::TopBottomPanel::top("ctrl_bar").show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("● Connected")
                                .color(egui::Color32::from_rgb(80, 200, 80))
                                .size(13.0));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.add(egui::Button::new(
                                    egui::RichText::new("End Connection").size(13.0))
                                    .fill(egui::Color32::from_rgb(180, 40, 40))
                                ).clicked() {
                                    let _ = terminate_tx.try_send(());
                                    self.state = AppState::Home;
                                }
                            });
                        });
                    });

                    egui::CentralPanel::default()
                        .frame(egui::Frame::none().fill(egui::Color32::BLACK))
                        .show(ctx, |ui| {
                            if let Some(tex) = texture {
                                let avail = ui.available_size();
                                let ts = tex.size_vec2();
                                let scale = (avail.x / ts.x).min(avail.y / ts.y);
                                let disp = ts * scale;

                                let resp = ui.add(
                                    egui::Image::new(tex)
                                        .fit_to_exact_size(disp)
                                        .sense(egui::Sense::click_and_drag()),
                                );

                                if let Some(pos) = resp.hover_pos() {
                                    let rect = resp.rect;
                                    let x = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64;
                                    let y = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0) as f64;

                                    if resp.hovered() {
                                        let msg = serde_json::json!({"type":"mouse_move","x":x,"y":y});
                                        let _ = input_tx.try_send(msg.to_string());
                                    }
                                    if resp.clicked() {
                                        send_click(input_tx, "left", x, y);
                                    }
                                    if resp.secondary_clicked() {
                                        send_click(input_tx, "right", x, y);
                                    }
                                    if resp.middle_clicked() {
                                        send_click(input_tx, "middle", x, y);
                                    }
                                }

                                let scroll = ui.input(|i| i.smooth_scroll_delta);
                                if scroll != egui::Vec2::ZERO {
                                    let msg = serde_json::json!({
                                        "type":"scroll",
                                        "deltaX": -scroll.x as f64,
                                        "deltaY": -scroll.y as f64
                                    });
                                    let _ = input_tx.try_send(msg.to_string());
                                }
                            }

                            ctx.input(|i| {
                                for event in &i.events {
                                    match event {
                                        egui::Event::Key { key, pressed, modifiers, .. } => {
                                            let action = if *pressed { "down" } else { "up" };
                                            if modifiers.shift { send_key(input_tx, action, "Shift"); }
                                            if modifiers.ctrl  { send_key(input_tx, action, "Control"); }
                                            if modifiers.alt   { send_key(input_tx, action, "Alt"); }
                                            let ks = egui_key_to_str(key);
                                            if !ks.is_empty() { send_key(input_tx, action, ks); }
                                        }
                                        egui::Event::Text(text) => {
                                            for ch in text.chars() {
                                                let s = ch.to_string();
                                                send_key(input_tx, "down", &s);
                                                send_key(input_tx, "up", &s);
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            });
                        });
                }

                // ── Participant — enter token ──────────────────────────────────
                AppState::ParticipantInput { token_input, error } => {
                    let mut connect = false;

                    ui.vertical_centered(|ui| {
                        ui.add_space(36.0);
                        ui.heading(egui::RichText::new("Remota").size(30.0).strong());
                        ui.label(egui::RichText::new("Remote Desktop")
                            .size(14.0).color(egui::Color32::GRAY));
                        ui.add_space(32.0);
                        ui.label("Paste the session token sent by the controller:");
                        ui.add_space(8.0);

                        let resp = ui.add_sized(
                            egui::vec2(380.0, 34.0),
                            egui::TextEdit::singleline(token_input)
                                .hint_text("Session token…")
                        );

                        ui.add_space(8.0);
                        if let Some(err) = error.as_ref() {
                            ui.label(egui::RichText::new(err)
                                .color(egui::Color32::RED).size(12.0));
                            ui.add_space(4.0);
                        }

                        let clicked = ui.add_sized(
                            egui::vec2(180.0, 40.0),
                            egui::Button::new(egui::RichText::new("Connect").size(15.0))
                        ).clicked();

                        let enter = resp.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter));

                        if clicked || enter { connect = true; }

                        ui.add_space(8.0);
                        if ui.small_button("← Back").clicked() {
                            self.state = AppState::Home;
                        }
                    });

                    if connect {
                        let token = token_input.trim().to_owned();
                        if token.is_empty() {
                            *error = Some("Please enter a session token.".to_owned());
                        } else {
                            self.start_participant(token);
                        }
                    }
                }

                // ── Participant active ────────────────────────────────────────
                AppState::ParticipantActive { terminate_tx } => {
                    let terminate_tx = terminate_tx.clone();
                    ui.vertical_centered(|ui| {
                        ui.add_space(60.0);
                        ui.label(egui::RichText::new("● Remote access is active")
                            .size(18.0).color(egui::Color32::from_rgb(80, 200, 80)));
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("The controller can see and control your screen.")
                            .color(egui::Color32::GRAY));
                        ui.add_space(32.0);
                        if ui.add_sized(egui::vec2(180.0, 44.0),
                            egui::Button::new(egui::RichText::new("End Session").size(15.0))
                                .fill(egui::Color32::from_rgb(180, 40, 40))
                        ).clicked() {
                            let _ = terminate_tx.try_send(());
                            self.state = AppState::Home;
                        }
                    });
                }

                // ── Ended ────────────────────────────────────────────────────
                AppState::Ended { message } => {
                    let msg = message.clone();
                    ui.vertical_centered(|ui| {
                        ui.add_space(80.0);
                        ui.label(egui::RichText::new(&msg).size(18.0));
                        ui.add_space(24.0);
                        if ui.button("Back to Home").clicked() {
                            self.state = AppState::Home;
                        }
                    });
                }
            }
        });
    }
}

// ── Launch helpers ────────────────────────────────────────────────────────────

impl RemotaApp {
    fn start_controller(&mut self, ctx: &egui::Context) {
        let msg_tx = self.msg_tx.clone();
        let ctx2 = ctx.clone();

        // Create room synchronously on a blocking thread, then hand off to async
        let (term_tx, term_rx) = std::sync::mpsc::sync_channel::<()>(1);

        match controller::create_room() {
            Err(e) => {
                error!("[main] create_room failed: {e}");
                self.state = AppState::Ended { message: format!("Failed to create room: {e}") };
                return;
            }
            Ok((token, join_url)) => {
                let token2 = token.clone();
                let _join_url2 = join_url.clone();

                std::thread::spawn(move || {
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    if let Err(e) = rt.block_on(
                        controller::run_controller_async(token2, msg_tx.clone(), term_rx, ctx2)
                    ) {
                        error!("[controller] error: {e}");
                    }
                    let _ = msg_tx.try_send(AppMsg::SessionEnded);
                });

                self.state = AppState::ControllerWaiting {
                    token,
                    join_url,
                    terminate_tx: term_tx,
                };
            }
        }
    }

    fn start_participant(&mut self, token: String) {
        let msg_tx = self.msg_tx.clone();
        let (term_tx, term_rx) = std::sync::mpsc::sync_channel::<()>(1);

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            if let Err(e) = rt.block_on(run_participant_async(token, term_rx)) {
                error!("[participant] error: {e}");
            }
            let _ = msg_tx.try_send(AppMsg::SessionEnded);
        });

        self.state = AppState::ParticipantActive { terminate_tx: term_tx };
    }
}

// ── Participant async task ────────────────────────────────────────────────────

async fn run_participant_async(
    token: String,
    term_rx: std::sync::mpsc::Receiver<()>,
) -> Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::mpsc;
    use tracing::info;
    use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

    let ws_url = format!("{}/ws", config::WS_URL);
    info!("[participant] connecting token={token}");

    let (frame_tx, frame_rx) = mpsc::channel::<capture::CapturedFrame>(4);
    let (control_tx, mut control_rx) = mpsc::channel::<protocol::ControlMessage>(64);
    let (ice_event_tx, mut ice_event_rx) = mpsc::channel::<String>(32);
    let (state_tx, mut state_rx) = mpsc::channel::<RTCPeerConnectionState>(8);

    let (mut signal_rx, signal_tx) = signaling::connect(&ws_url, &token).await?;

    let api = webrtc_session::build_api()?;
    let session = std::sync::Arc::new(
        webrtc_session::Session::new(&api, control_tx, ice_event_tx, state_tx).await?,
    );

    let stop_capture = std::sync::Arc::new(AtomicBool::new(false));
    capture::start(frame_tx, std::sync::Arc::clone(&stop_capture))?;

    let force_keyframe = std::sync::Arc::new(AtomicBool::new(false));
    let (connected_tx, connected_rx) = tokio::sync::watch::channel(false);

    {
        let track = std::sync::Arc::clone(&session.video_track);
        let kf = std::sync::Arc::clone(&force_keyframe);
        tokio::spawn(async move {
            webrtc_session::run_encoding_loop(track, frame_rx, kf, connected_rx).await;
        });
    }

    let mut input_ctrl = input::InputController::new(
        get_primary_screen_dimensions().0,
        get_primary_screen_dimensions().1,
    )?;

    {
        let sig = signal_tx.clone();
        tokio::spawn(async move {
            while let Some(cj) = ice_event_rx.recv().await {
                match signaling::make_ice_msg(&cj) {
                    Ok(msg) => { let _ = sig.send(msg).await; }
                    Err(e) => error!("[participant] ICE msg: {e}"),
                }
            }
        });
    }

    show_active_notification();

    loop {
        // Check if UI requested termination
        if term_rx.try_recv().is_ok() {
            let _ = signal_tx.send(r#"{"type":"terminate"}"#.to_owned()).await;
            break;
        }

        tokio::select! {
            Some(event) = signal_rx.recv() => match event {
                signaling::SignalEvent::Offer(sdp) => {
                    match session.handle_offer(&sdp).await {
                        Ok(answer) => {
                            match signaling::make_answer_msg(&answer) {
                                Ok(msg) => { let _ = signal_tx.send(msg).await; }
                                Err(e) => error!("[participant] answer msg: {e}"),
                            }
                        }
                        Err(e) => error!("[participant] handle_offer: {e}"),
                    }
                }
                signaling::SignalEvent::IceCandidate(cj) => {
                    let _ = session.add_ice_candidate(&cj).await;
                }
                signaling::SignalEvent::Terminate => break,
                signaling::SignalEvent::Disconnected => {
                    tokio::time::sleep(tokio::time::Duration::from_secs(12)).await;
                    break;
                }
            },
            Some(msg) = control_rx.recv() => {
                input_ctrl.dispatch(msg);
            }
            Some(state) = state_rx.recv() => match state {
                RTCPeerConnectionState::Connected => {
                    let _ = signal_tx.send(r#"{"type":"active"}"#.to_owned()).await;
                    force_keyframe.store(true, Ordering::Relaxed);
                    let _ = connected_tx.send(true);
                }
                RTCPeerConnectionState::Failed => {
                    let _ = signal_tx.send(r#"{"type":"terminate"}"#.to_owned()).await;
                }
                _ => {}
            },
        }
    }

    input_ctrl.release_all_modifiers();
    stop_capture.store(true, std::sync::atomic::Ordering::Relaxed);
    session.close().await;
    Ok(())
}

fn get_primary_screen_dimensions() -> (u32, u32) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        let w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        if w > 0 && h > 0 { return (w as u32, h as u32); }
    }
    (1920, 1080)
}

fn show_active_notification() {
    #[cfg(target_os = "windows")]
    std::thread::spawn(|| {
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK, MB_ICONINFORMATION, MB_TOPMOST};
        use windows::core::PCWSTR;
        let title: Vec<u16> = "Remota — Remote Access Active\0".encode_utf16().collect();
        let msg: Vec<u16> = "Remote access is now active.\nClose Remota to end the session.\0"
            .encode_utf16().collect();
        unsafe { MessageBoxW(None, PCWSTR(msg.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONINFORMATION | MB_TOPMOST); }
    });
}

// ── Input helpers ─────────────────────────────────────────────────────────────

fn send_click(tx: &tokio::sync::mpsc::Sender<String>, button: &str, x: f64, y: f64) {
    let down = serde_json::json!({"type":"mouse_button","action":"down","button":button,"x":x,"y":y});
    let up   = serde_json::json!({"type":"mouse_button","action":"up",  "button":button,"x":x,"y":y});
    let _ = tx.try_send(down.to_string());
    let _ = tx.try_send(up.to_string());
}

fn send_key(tx: &tokio::sync::mpsc::Sender<String>, action: &str, key: &str) {
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
        egui::Key::F1  => "F1",  egui::Key::F2  => "F2",
        egui::Key::F3  => "F3",  egui::Key::F4  => "F4",
        egui::Key::F5  => "F5",  egui::Key::F6  => "F6",
        egui::Key::F7  => "F7",  egui::Key::F8  => "F8",
        egui::Key::F9  => "F9",  egui::Key::F10 => "F10",
        egui::Key::F11 => "F11", egui::Key::F12 => "F12",
        _ => "",
    }
}
