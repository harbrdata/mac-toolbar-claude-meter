/// Read Claude Code OAuth credentials from macOS Keychain via the `security` CLI.
///
/// Using the CLI instead of Security.framework's SecItemCopyMatching avoids
/// the per-app Keychain Access authorization prompt.
pub fn read_credentials() -> Option<serde_json::Value> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output()
        .ok()?;

    if !output.status.success() {
        eprintln!(
            "Keychain read failed (exit {})",
            output.status.code().unwrap_or(-1)
        );
        return None;
    }

    let raw = String::from_utf8(output.stdout).ok()?;
    let raw = raw.trim();

    let mut data: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Keychain JSON parse error: {e}");
            return None;
        }
    };

    if data.get("claudeAiOauth").is_some() {
        data = data["claudeAiOauth"].take();
    }

    Some(data)
}
