use std::process::Command;

const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// Read Claude Code OAuth credentials from macOS Keychain.
pub fn read_credentials() -> Option<serde_json::Value> {
    let output = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .output()
        .ok()?;

    if !output.status.success() {
        eprintln!("Keychain read failed (rc={})", output.status);
        return None;
    }

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let mut data: serde_json::Value = serde_json::from_str(&raw).ok()?;

    if data.get("claudeAiOauth").is_some() {
        data = data["claudeAiOauth"].take();
    }

    Some(data)
}
