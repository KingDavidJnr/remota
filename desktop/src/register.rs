// ── Protocol handler registration ─────────────────────────────────────────────
//
// Windows: writes remota:// into HKCU\Software\Classes\remota (no admin needed)
// macOS:   writes/updates LSHandlers in ~/Library/Preferences/com.apple.LaunchServices.plist
//          then calls `lsregister` to notify LaunchServices of the change.
//          The app bundle's Info.plist must also declare CFBundleURLTypes (see
//          desktop/Info.plist) — the runtime call here is only needed when
//          running as a plain binary outside an .app bundle.

// ── Windows ───────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
pub fn register_protocol_handler() -> anyhow::Result<()> {
    use std::env;
    use winreg::{enums::*, RegKey};

    let exe = env::current_exe()?.to_string_lossy().into_owned();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);

    let (base, _) = hkcu.create_subkey(r"Software\Classes\remota")?;
    base.set_value("", &"URL:Remota Protocol")?;
    base.set_value("URL Protocol", &"")?;

    let (icon, _) = base.create_subkey("DefaultIcon")?;
    icon.set_value("", &format!("{exe},0"))?;

    let (cmd, _) = base.create_subkey(r"shell\open\command")?;
    cmd.set_value("", &format!("\"{exe}\" \"%1\""))?;

    tracing::info!("[register] remota:// protocol handler registered (Windows registry)");
    Ok(())
}

// ── macOS ─────────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
pub fn register_protocol_handler() -> anyhow::Result<()> {
    use std::env;
    use std::process::Command;

    let exe = env::current_exe()?;

    // `lsregister` is the canonical tool to register a binary with LaunchServices.
    // When running as a plain .app bundle, this happens automatically on first launch.
    // When running as a bare binary (development / CLI), we call it explicitly.
    //
    // Path on all macOS versions:
    let lsregister =
        "/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/\
         LaunchServices.framework/Versions/A/Support/lsregister";

    let status = Command::new(lsregister)
        .arg("-f")           // force re-registration
        .arg(exe.to_str().unwrap_or(""))
        .status();

    match status {
        Ok(s) if s.success() => {
            tracing::info!("[register] remota:// protocol handler registered (LaunchServices)");
        }
        Ok(s) => {
            tracing::warn!("[register] lsregister exited with status {s} — deep-link may not work");
        }
        Err(e) => {
            tracing::warn!("[register] lsregister not found or failed: {e}");
        }
    }

    Ok(())
}

// ── Other platforms ───────────────────────────────────────────────────────────

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn register_protocol_handler() -> anyhow::Result<()> {
    Ok(())
}

// ── URI parsing (shared) ──────────────────────────────────────────────────────

/// Parse `remota://session/<token>` → token string.
/// Returns `None` for any other format.
pub fn parse_deep_link(uri: &str) -> Option<String> {
    let path = uri
        .strip_prefix("remota://session/")?
        .trim_end_matches('/');

    if path.is_empty() {
        return None;
    }

    Some(path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_deep_link() {
        assert_eq!(
            parse_deep_link("remota://session/abc123XYZ"),
            Some("abc123XYZ".to_owned())
        );
    }

    #[test]
    fn rejects_wrong_scheme() {
        assert!(parse_deep_link("https://remota.example.com/session/abc").is_none());
    }

    #[test]
    fn rejects_empty_token() {
        assert!(parse_deep_link("remota://session/").is_none());
    }

    #[test]
    fn handles_trailing_slash() {
        assert_eq!(
            parse_deep_link("remota://session/mytoken/"),
            Some("mytoken".to_owned())
        );
    }
}
