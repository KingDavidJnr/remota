// ── Control protocol (§14) ────────────────────────────────────────────────────
// These types mirror exactly the JSON messages sent over the WebRTC DataChannel.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    MouseMove {
        x: f64, // 0.0 – 1.0 normalised
        y: f64,
    },
    MouseButton {
        action: ButtonAction,
        button: MouseButton,
    },
    MouseDblclick {
        x: f64,
        y: f64,
    },
    Scroll {
        #[serde(rename = "deltaX")]
        delta_x: f64,
        #[serde(rename = "deltaY")]
        delta_y: f64,
    },
    Keyboard {
        action: KeyAction,
        key: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ButtonAction {
    Down,
    Up,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyAction {
    Down,
    Up,
}
