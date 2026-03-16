# Claude-o-Meter

A lightweight macOS menu bar app that shows your Claude Code plan usage at a glance.

Displays a color-coded ring gauge in the menu bar showing your current 5-hour usage window. Click it for a full breakdown of all usage windows with progress bars and reset countdowns.

**macOS only** — requires macOS 13 (Ventura) or later.

## Quick start

Requires an active Claude Code login (`claude login`).

### Option A: Download the DMG (easiest)

1. Download `Claude-o-Meter.dmg` from the [latest release](https://github.com/harbrdata/mac-toolbar-claude-meter/releases/latest)
2. Open the DMG
3. Double-click **Claude-o-Meter.pkg** to launch the installer

The installer will copy the app to `/Applications`, configure it to start at login, and launch it automatically.

To upgrade, repeat the same steps — the installer quits the running instance before replacing it.

To uninstall:

```bash
launchctl bootout gui/$(id -u)/com.local.claude-o-meter
rm ~/Library/LaunchAgents/com.local.claude-o-meter.plist
rm -rf /Applications/Claude-o-Meter.app
```

### Option B: Build from source

Requires [Rust](https://www.rust-lang.org/tools/install) (1.85+).

```bash
git clone git@github.com:harbrdata/mac-toolbar-claude-meter.git
cd mac-toolbar-claude-meter
cargo build --release
open target/release/claude-o-meter
```

### Running locally (development)

```bash
cargo run
```

This builds and launches the app in debug mode. The gauge icon appears in your menu bar immediately. To stop it, click the icon and select **Quit**, or press Ctrl+C in the terminal.

For a faster binary closer to release performance:

```bash
cargo run --release
```

Or build a distributable DMG:

```bash
./build_dmg.sh
```

This creates `dist/Claude-o-Meter.dmg` — a disk image containing a `.pkg` installer.

## Features

- Ring gauge icon with percentage in the menu bar (green/yellow/orange/red)
- Dropdown with all usage windows: 5h, 7d, Opus, Sonnet, Cowork, OAuth
- Progress bars and reset time countdowns
- Configurable refresh interval (1m / 2m / 5m / 10m)
- Usage alert notification with configurable threshold (75% / 80% / 85% / 90% / 95% / Off)
- Toggle polling on/off from the menu
- Start at Login toggle (installs/removes a Launch Agent)
- Automatic rate-limit handling with exponential backoff
- Recent Logs submenu for debugging without leaving the menu bar
- Reads credentials from your existing `claude login` session
- No dock icon — runs purely in the menu bar
- Native Rust binary — ~1.5MB, instant startup, no runtime dependencies

## How authentication works

Claude-o-Meter reads the OAuth credentials that the Claude Code CLI stores in your macOS Keychain. When you run `claude login`, Claude Code saves an access token and refresh token to a Keychain entry named `Claude Code-credentials`. This app reads that entry using the `security` command-line tool.

The app uses the access token to call the Anthropic usage API (`https://api.anthropic.com/api/oauth/usage`). If the token has expired, it automatically refreshes it using the stored refresh token.

**No credentials are stored, transmitted, or logged by this app** — it only reads what Claude Code already put in your Keychain. If you revoke your Claude Code session or log out, the app will show an error icon until you run `claude login` again.

## Troubleshooting

- **Menu bar icon not visible (macOS 26+):** Go to **System Settings > Control Center > Menu Bar Only** and ensure Claude-o-Meter is set to "Show in Menu Bar".
- **Menu bar icon not visible (macOS 15 and earlier):** Your menu bar may be full. Hold Cmd and drag other icons to make space.
- **Greyed-out "||" icon:** Rate-limited (auto-resumes) or polling is turned off. Open the dropdown to check.
- **"!" error icon:** Run `claude login` to refresh your credentials.

## License

Copyright (c) 2025-2026 Harbr Data. All rights reserved.
