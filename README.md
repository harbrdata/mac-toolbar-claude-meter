# Claude-o-Meter

A lightweight macOS menu bar app that shows your Claude Code plan usage at a glance.

Displays a color-coded ring gauge in the menu bar showing your current 5-hour usage window. Click it for a full breakdown of all usage windows with progress bars and reset countdowns.

**macOS only** — requires macOS 13 (Ventura) or later.

## Quick start

Requires Python 3.10+ and an active Claude Code login (`claude login`).

```bash
git clone git@github.com:harbrdata/mac-toolbar-claude-meter.git
cd mac-toolbar-claude-meter
./install.sh
```

This sets up the venv, builds the `.app` bundle, installs a Launch Agent, and starts the app. It will auto-start on every login.

To uninstall:

```bash
./uninstall.sh
```

## Features

- Ring gauge icon with percentage in the menu bar (green/yellow/orange/red)
- Dropdown with all usage windows: 5h, 7d, Opus, Sonnet, etc.
- Progress bars and reset time countdowns
- Configurable refresh interval (30s / 1m / 2m / 5m)
- Reads credentials from your existing `claude login` session
- No dock icon — runs purely in the menu bar

## Running manually

### From Terminal

```bash
.venv/bin/python3 claude_meter.py
```

> **Note:** If launched from an IDE terminal (e.g. VS Code), the menu bar icon may not appear. Use Terminal.app or the `.app` bundle instead.

### Using the .app bundle

A pre-built `.app` bundle is included for convenience:

```bash
open Claude-o-Meter.app
```

## Run on startup

The install script (above) is the recommended approach. Alternatives:

### Login Items

1. Open **System Settings > General > Login Items**
2. Click **+** under "Open at Login"
3. Navigate to this folder and select `Claude-o-Meter.app`

### Manual Launch Agent

Create a Launch Agent plist:

```bash
cat > ~/Library/LaunchAgents/com.local.claude-o-meter.plist << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.local.claude-o-meter</string>
    <key>ProgramArguments</key>
    <array>
        <string>$(pwd)/.venv/bin/python3</string>
        <string>$(pwd)/claude_meter.py</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardErrorPath</key>
    <string>/tmp/claude_meter.log</string>
</dict>
</plist>
EOF
```

Then load it:

```bash
launchctl load ~/Library/LaunchAgents/com.local.claude-o-meter.plist
```

To stop and unload:

```bash
launchctl unload ~/Library/LaunchAgents/com.local.claude-o-meter.plist
```

## Rebuilding the .app bundle

After making changes, rebuild the `.app` bundle:

```bash
APP_DIR="Claude-o-Meter.app"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"

cat > "$APP_DIR/Contents/Info.plist" << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Claude-o-Meter</string>
    <key>CFBundleIdentifier</key>
    <string>com.local.claude-o-meter</string>
    <key>CFBundleVersion</key>
    <string>1.0.0</string>
    <key>CFBundleExecutable</key>
    <string>launch</string>
    <key>LSUIElement</key>
    <true/>
</dict>
</plist>
EOF

cat > "$APP_DIR/Contents/MacOS/launch" << SCRIPT
#!/bin/bash
exec $(pwd)/.venv/bin/python3 $(pwd)/claude_meter.py 2>/tmp/claude_meter.log
SCRIPT
chmod +x "$APP_DIR/Contents/MacOS/launch"
```

## How authentication works

Claude-o-Meter reads the OAuth credentials that the Claude Code CLI stores in your macOS Keychain. When you run `claude login`, Claude Code saves an access token and refresh token to a Keychain entry named `Claude Code-credentials`. This app reads that entry using the `security` command-line tool (the same way any app reads Keychain items).

The app uses the access token to call the Anthropic usage API (`https://api.anthropic.com/api/oauth/usage`). If the token has expired, it automatically refreshes it using the stored refresh token.

**No credentials are stored, transmitted, or logged by this app** — it only reads what Claude Code already put in your Keychain. If you revoke your Claude Code session or log out, the app will show an error icon until you run `claude login` again.

## Logs

Logs are written to `/tmp/claude_meter.log` when running via the `.app` bundle or Launch Agent. When running from Terminal directly, logs print to stdout.

## Troubleshooting

- **Menu bar icon not visible:** Your menu bar may be full. Hold Cmd and drag other icons to make space, or use a menu bar manager like Bartender or Ice.
- **"Loading..." in dropdown:** The API may be rate-limited. Wait for the next refresh cycle.
- **"!" error icon:** Run `claude login` to refresh your credentials.
