// ── Input injection via enigo ─────────────────────────────────────────────────
// Translates incoming ControlMessages into real Win32 SendInput calls.

use crate::protocol::{ButtonAction, ControlMessage, KeyAction, MouseButton};
use anyhow::Result;
use enigo::{
    Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings,
};
use tracing::{debug, warn};

pub struct InputController {
    enigo: Enigo,
    screen_w: u32,
    screen_h: u32,
}

impl InputController {
    pub fn new(screen_w: u32, screen_h: u32) -> Result<Self> {
        let enigo = Enigo::new(&Settings::default())?;
        Ok(Self {
            enigo,
            screen_w,
            screen_h,
        })
    }

    /// Dispatch one control message to the OS.
    pub fn dispatch(&mut self, msg: ControlMessage) {
        if let Err(e) = self.try_dispatch(msg) {
            warn!("Input dispatch error: {e}");
        }
    }

    fn try_dispatch(&mut self, msg: ControlMessage) -> Result<()> {
        match msg {
            ControlMessage::MouseMove { x, y } => {
                let px = (x * self.screen_w as f64).round() as i32;
                let py = (y * self.screen_h as f64).round() as i32;
                debug!("mouse_move ({px}, {py})");
                self.enigo
                    .move_mouse(px, py, Coordinate::Abs)?;
            }

            ControlMessage::MouseButton { action, button } => {
                let btn = map_button(button);
                let dir = match action {
                    ButtonAction::Down => Direction::Press,
                    ButtonAction::Up => Direction::Release,
                };
                debug!("mouse_button {btn:?} {dir:?}");
                self.enigo.button(btn, dir)?;
            }

            ControlMessage::MouseDblclick { x, y } => {
                let px = (x * self.screen_w as f64).round() as i32;
                let py = (y * self.screen_h as f64).round() as i32;
                self.enigo.move_mouse(px, py, Coordinate::Abs)?;
                self.enigo.button(Button::Left, Direction::Click)?;
                self.enigo.button(Button::Left, Direction::Click)?;
            }

            ControlMessage::Scroll { delta_x, delta_y } => {
                // Positive deltaY = scroll down in browser convention
                if delta_y.abs() > 0.1 {
                    self.enigo.scroll(delta_y as i32, enigo::Axis::Vertical)?;
                }
                if delta_x.abs() > 0.1 {
                    self.enigo
                        .scroll(delta_x as i32, enigo::Axis::Horizontal)?;
                }
            }

            ControlMessage::Keyboard { action, key } => {
                let dir = match action {
                    KeyAction::Down => Direction::Press,
                    KeyAction::Up => Direction::Release,
                };
                let k = map_key(&key);
                debug!("keyboard {k:?} {dir:?}");
                self.enigo.key(k, dir)?;
            }
        }
        Ok(())
    }

    /// Release all held modifier keys. Called on connection drop to prevent
    /// stuck keys (fail-closed requirement from §22).
    pub fn release_all_modifiers(&mut self) {
        let modifiers = [
            Key::Shift,
            Key::Control,
            Key::Alt,
            Key::Meta,
        ];
        for k in modifiers {
            let _ = self.enigo.key(k, Direction::Release);
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn map_button(b: MouseButton) -> Button {
    match b {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
    }
}

/// Map a JS KeyboardEvent.key string to an enigo Key.
/// Covers all keys required by §13 of the PRD.
fn map_key(key: &str) -> Key {
    match key {
        // Modifier keys
        "Shift" | "ShiftLeft" | "ShiftRight" => Key::Shift,
        "Control" | "ControlLeft" | "ControlRight" => Key::Control,
        "Alt" | "AltLeft" | "AltRight" => Key::Alt,
        "Meta" | "MetaLeft" | "MetaRight" => Key::Meta,
        "CapsLock" => Key::CapsLock,

        // Navigation
        "ArrowUp" => Key::UpArrow,
        "ArrowDown" => Key::DownArrow,
        "ArrowLeft" => Key::LeftArrow,
        "ArrowRight" => Key::RightArrow,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,

        // Editing
        "Backspace" => Key::Backspace,
        "Delete" => Key::Delete,
        "Enter" => Key::Return,
        "Tab" => Key::Tab,
        "Escape" => Key::Escape,
        "Insert" => Key::Insert,
        " " => Key::Space,

        // Function keys
        "F1" => Key::F1,
        "F2" => Key::F2,
        "F3" => Key::F3,
        "F4" => Key::F4,
        "F5" => Key::F5,
        "F6" => Key::F6,
        "F7" => Key::F7,
        "F8" => Key::F8,
        "F9" => Key::F9,
        "F10" => Key::F10,
        "F11" => Key::F11,
        "F12" => Key::F12,

        // Single printable characters
        s if s.chars().count() == 1 => {
            Key::Unicode(s.chars().next().unwrap())
        }

        // Unknown — log and ignore
        other => {
            warn!("Unknown key: {other:?}");
            Key::Unicode('\0')
        }
    }
}
