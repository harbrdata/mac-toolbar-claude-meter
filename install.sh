#!/bin/bash
# Install Claude-o-Meter to run on startup via macOS Launch Agent.
# Safe to run multiple times — will update the existing installation.
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
APP_NAME="Claude-o-Meter"
APP_DIR="$SCRIPT_DIR/$APP_NAME.app"
VENV_DIR="$SCRIPT_DIR/.venv"
PLIST_LABEL="com.local.claude-o-meter"
PLIST_PATH="$HOME/Library/LaunchAgents/$PLIST_LABEL.plist"

echo "=== Claude-o-Meter Installer ==="
echo ""

# 1. Create venv and install deps
if [ ! -d "$VENV_DIR" ]; then
    echo "Creating virtual environment..."
    python3 -m venv "$VENV_DIR"
else
    echo "Virtual environment already exists."
fi

echo "Installing/updating dependencies..."
"$VENV_DIR/bin/pip" install --quiet --index-url https://pypi.org/simple/ -r "$SCRIPT_DIR/requirements.txt"

# 2. Build .app bundle (always rebuild to pick up code changes)
echo "Building $APP_NAME.app..."
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"

cat > "$APP_DIR/Contents/Info.plist" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>$APP_NAME</string>
    <key>CFBundleIdentifier</key>
    <string>$PLIST_LABEL</string>
    <key>CFBundleVersion</key>
    <string>1.0.0</string>
    <key>CFBundleExecutable</key>
    <string>launch</string>
    <key>LSUIElement</key>
    <true/>
    <key>LSBackgroundOnly</key>
    <false/>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSMenuBarItemProviding</key>
    <true/>
</dict>
</plist>
EOF

cat > "$APP_DIR/Contents/MacOS/launch" << SCRIPT
#!/bin/bash
exec "$VENV_DIR/bin/python3" "$SCRIPT_DIR/claude_meter.py" 2>/tmp/claude_meter.log
SCRIPT
chmod +x "$APP_DIR/Contents/MacOS/launch"

# 3. Stop any running instances
echo "Stopping any running instances..."
# Gracefully quit the app via AppleScript
osascript -e "quit app \"$APP_NAME\"" 2>/dev/null || true
# Also kill by process pattern in case it was launched from terminal
pkill -f "claude_meter.py" 2>/dev/null || true
pkill -f "$APP_NAME" 2>/dev/null || true
# Bootout the launch agent so launchctl doesn't fight us
launchctl bootout "gui/$(id -u)/$PLIST_LABEL" 2>/dev/null || true
# Wait for processes to exit, force-kill stragglers
sleep 1
pkill -9 -f "claude_meter.py" 2>/dev/null || true
pkill -9 -f "$APP_NAME" 2>/dev/null || true

# 4. Remove old launch agent plist
rm -f "$PLIST_PATH"

# 5. Install launch agent
cat > "$PLIST_PATH" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$PLIST_LABEL</string>
    <key>ProgramArguments</key>
    <array>
        <string>open</string>
        <string>-a</string>
        <string>$APP_DIR</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardErrorPath</key>
    <string>/tmp/claude_meter.log</string>
</dict>
</plist>
EOF

# 6. Load and start now
echo "Starting $APP_NAME..."
launchctl bootstrap "gui/$(id -u)" "$PLIST_PATH"

echo ""
echo "Done! $APP_NAME is running and will start automatically on login."
echo "To uninstall, run: ./uninstall.sh"
