use std::fs;
use std::path::PathBuf;
use std::process::Command;

const LABEL: &str = "com.local.claude-o-meter";

fn plist_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home)
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

pub fn is_enabled() -> bool {
    plist_path().exists()
}

pub fn enable() {
    let path = plist_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Applications/Claude-o-Meter.app/Contents/MacOS/Claude-o-Meter</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardErrorPath</key>
    <string>/tmp/claude_meter.log</string>
</dict>
</plist>"#
    );

    let _ = fs::write(&path, plist);
    // Don't bootstrap — the app is already running. The plist takes effect on next login.
}

pub fn disable() {
    let path = plist_path();
    let uid = unsafe { libc::getuid() };
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}/{LABEL}")])
        .output();
    let _ = fs::remove_file(&path);
}

/// If the .app bundle is gone (user dragged to Trash), clean up the launch agent.
pub fn cleanup_if_uninstalled() {
    if !is_enabled() {
        return;
    }
    let app_path = PathBuf::from("/Applications/Claude-o-Meter.app");
    if !app_path.exists() {
        disable();
    }
}
