// ── Protocol handler registration ─────────────────────────────────────────────
// Registers the `remota://` URI scheme in the current user's registry so that
// clicking a `remota://session/<token>` link in any browser launches this exe.
//
// Registry layout written under HKCU\Software\Classes\remota:
//
//   (Default)            = "URL:Remota Protocol"
//   URL Protocol         = ""
//   DefaultIcon\(Default)= "<exe>,0"
//   shell\open\command\(Default) = "<exe>" "%1"
//
// Using HKCU (current user) avoids requiring admin privileges.

#[cfg(target_os = "windows")]
pub fn register_protocol_handler() -> anyhow::Result<()> {
    use std::env;
    use winreg::{enums::*, RegKey};

    let exe = env::current_exe()?.to_string_lossy().into_owned();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);

    // HKCU\Software\Classes\remota
    let (base, _) = hkcu.create_subkey(r"Software\Classes\remota")?;
    base.set_value("", &"URL:Remota Protocol")?;
    base.set_value("URL Protocol", &"")?;

    // …\DefaultIcon
    let (icon, _) = base.create_subkey("DefaultIcon")?;
    icon.set_value("", &format!("{exe},0"))?;

    // …\shell\open\command
    let (cmd, _) = base.create_subkey(r"shell\open\command")?;
    cmd.set_value("", &format!("\"{exe}\" \"%1\""))?;

    tracing::info!("[register] remota:// protocol handler registered");
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn register_protocol_handler() -> anyhow::Result<()> {
    Ok(()) // no-op on non-Windows
}

// ── URI parsing ───────────────────────────────────────────────────────────────
// Parses `remota://session/<token>` and returns the token string.
// Returns None if the URI doesn't match the expected format.

pub fn parse_deep_link(uri: &str) -> Option<String> {
    // Expected: remota://session/<token>
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
        let token = parse_deep_link("remota://session/abc123XYZ");
        assert_eq!(token, Some("abc123XYZ".to_owned()));
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
        let token = parse_deep_link("remota://session/mytoken/");
        assert_eq!(token, Some("mytoken".to_owned()));
    }
}
