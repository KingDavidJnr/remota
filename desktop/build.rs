// build.rs — runs at compile time, not at runtime.
// Validates that required build-time environment variables are set and
// tells Cargo to rerun this script only when they change.
//
// Set these in your CI / local shell before running `cargo build --release`:
//
//   REMOTA_WS_URL       wss://api.remota.quickdesk.tech   (required)
//   REMOTA_TURN_URL     turn:turn.remota.quickdesk.tech:3478  (optional)
//   REMOTA_TURN_USER    <coturn username>                      (optional)
//   REMOTA_TURN_PASS    <coturn credential>                    (optional)

fn main() {
    // Re-run this build script if any of these vars change
    println!("cargo:rerun-if-env-changed=REMOTA_WS_URL");
    println!("cargo:rerun-if-env-changed=REMOTA_TURN_URL");
    println!("cargo:rerun-if-env-changed=REMOTA_TURN_USER");
    println!("cargo:rerun-if-env-changed=REMOTA_TURN_PASS");

    // REMOTA_WS_URL is required
    if std::env::var("REMOTA_WS_URL").unwrap_or_default().is_empty() {
        eprintln!();
        eprintln!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        eprintln!("  BUILD ERROR: REMOTA_WS_URL is not set.");
        eprintln!("  Set it before building:");
        eprintln!("    $env:REMOTA_WS_URL = 'wss://api.remota.quickdesk.tech'");
        eprintln!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        eprintln!();
        std::process::exit(1);
    }
}
