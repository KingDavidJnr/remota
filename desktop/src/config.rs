// ── Compile-time configuration ────────────────────────────────────────────────
// All values are baked into the binary at build time via environment variables.
// The end user never needs to set anything.
//
// Build-time variables (set in your shell or CI before `cargo build --release`):
//
//   REMOTA_WS_URL       wss://api.remota.quickdesk.tech        (required)
//   REMOTA_TURN_URL     turn:turn.remota.quickdesk.tech:3478   (optional)
//   REMOTA_TURN_USER    <coturn username>                       (optional)
//   REMOTA_TURN_PASS    <coturn credential>                     (optional)

/// WebSocket base URL of the Remota backend.
/// e.g. "wss://api.remota.quickdesk.tech"
pub const WS_URL: &str = env!("REMOTA_WS_URL");

/// TURN server URL, if configured at build time.
pub const TURN_URL: Option<&str> = option_env!("REMOTA_TURN_URL");

/// TURN username, if configured at build time.
pub const TURN_USER: Option<&str> = option_env!("REMOTA_TURN_USER");

/// TURN credential, if configured at build time.
pub const TURN_PASS: Option<&str> = option_env!("REMOTA_TURN_PASS");
