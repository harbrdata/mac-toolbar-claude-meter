#!/bin/bash
# Build a self-contained Claude-o-Meter.dmg for distribution.
# The DMG contains a standalone .app bundle with all dependencies embedded.
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
APP_NAME="Claude-o-Meter"
BUILD_DIR="$SCRIPT_DIR/build"
DIST_DIR="$SCRIPT_DIR/dist"
DMG_DIR="$DIST_DIR/dmg"
VENV_DIR="$SCRIPT_DIR/.venv"
APP_DIR="$DMG_DIR/$APP_NAME.app"
CONTENTS="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS/MacOS"
RESOURCES="$CONTENTS/Resources"

echo "=== Building $APP_NAME.dmg ==="
echo ""

# 1. Ensure venv and deps are installed
if [ ! -d "$VENV_DIR" ]; then
    echo "Creating virtual environment..."
    python3 -m venv "$VENV_DIR"
fi
echo "Installing dependencies..."
"$VENV_DIR/bin/pip" install --quiet --index-url https://pypi.org/simple/ -r "$SCRIPT_DIR/requirements.txt"

# 2. Clean previous build
rm -rf "$BUILD_DIR" "$DIST_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES"

# 3. Copy the app source
cp "$SCRIPT_DIR/claude_meter.py" "$RESOURCES/"
cp "$SCRIPT_DIR/requirements.txt" "$RESOURCES/"

# 4. Bundle the venv's site-packages (only what we need)
echo "Bundling Python dependencies..."
SITE_PACKAGES="$RESOURCES/lib"
mkdir -p "$SITE_PACKAGES"

# Copy the needed packages from the venv
VENV_SITE=$("$VENV_DIR/bin/python3" -c "import site; print(site.getsitepackages()[0])")
for pkg in requests urllib3 charset_normalizer certifi idna objc AppKit Foundation CoreFoundation PyObjCTools; do
    if [ -d "$VENV_SITE/$pkg" ]; then
        cp -R "$VENV_SITE/$pkg" "$SITE_PACKAGES/"
    fi
done
# Also copy .dylib files for PyObjC
find "$VENV_SITE" -maxdepth 2 -name "*.so" -path "*/objc/*" -exec cp {} "$SITE_PACKAGES/objc/" \; 2>/dev/null || true

# Copy top-level .so/.dylib files for pyobjc framework bindings
for framework in AppKit Foundation CoreFoundation; do
    find "$VENV_SITE/$framework" -name "*.so" 2>/dev/null | while read f; do
        cp "$f" "$SITE_PACKAGES/$framework/" 2>/dev/null || true
    done
done

# 5. Create the launcher script
cat > "$MACOS_DIR/launch" << 'LAUNCHER'
#!/bin/bash
# Self-contained launcher for Claude-o-Meter
DIR="$(cd "$(dirname "$0")/.." && pwd)"
RESOURCES="$DIR/Resources"

export PYTHONPATH="$RESOURCES/lib:$PYTHONPATH"
exec /usr/bin/env python3 "$RESOURCES/claude_meter.py" 2>/tmp/claude_meter.log
LAUNCHER
chmod +x "$MACOS_DIR/launch"

# 6. Create install helper script (shown in DMG)
cat > "$DMG_DIR/Install.command" << 'INSTALL'
#!/bin/bash
# Install Claude-o-Meter: copies app to /Applications and sets up login launch.
set -e

APP_NAME="Claude-o-Meter"
DMG_APP="$(cd "$(dirname "$0")" && pwd)/$APP_NAME.app"
INSTALL_DIR="/Applications"
PLIST_LABEL="com.local.claude-o-meter"
PLIST_PATH="$HOME/Library/LaunchAgents/$PLIST_LABEL.plist"

echo "=== Installing $APP_NAME ==="
echo ""

# Stop any running instance
echo "Stopping any running instances..."
osascript -e "quit app \"$APP_NAME\"" 2>/dev/null || true
pkill -f "claude_meter.py" 2>/dev/null || true
pkill -f "$APP_NAME" 2>/dev/null || true
launchctl bootout "gui/$(id -u)/$PLIST_LABEL" 2>/dev/null || true
sleep 1
pkill -9 -f "claude_meter.py" 2>/dev/null || true
pkill -9 -f "$APP_NAME" 2>/dev/null || true

# Remove old launch agent plist
rm -f "$PLIST_PATH"

# Copy app to /Applications
echo "Copying to $INSTALL_DIR..."
rm -rf "$INSTALL_DIR/$APP_NAME.app"
cp -R "$DMG_APP" "$INSTALL_DIR/"

# Install launch agent for auto-start on login
echo "Setting up auto-start on login..."
mkdir -p "$(dirname "$PLIST_PATH")"
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
        <string>$INSTALL_DIR/$APP_NAME.app</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardErrorPath</key>
    <string>/tmp/claude_meter.log</string>
</dict>
</plist>
EOF

# Start now
echo "Starting $APP_NAME..."
launchctl bootstrap "gui/$(id -u)" "$PLIST_PATH"

echo ""
echo "Done! $APP_NAME is running and will start automatically on login."
echo "You can now eject the disk image."
echo ""
echo "To uninstall later:"
echo "  launchctl bootout gui/$(id -u)/$PLIST_LABEL"
echo "  rm -f $PLIST_PATH"
echo "  rm -rf $INSTALL_DIR/$APP_NAME.app"
INSTALL
chmod +x "$DMG_DIR/Install.command"

# 7. Create Info.plist
cat > "$CONTENTS/Info.plist" << 'EOF'
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
    <key>CFBundleShortVersionString</key>
    <string>1.0.0</string>
    <key>CFBundleExecutable</key>
    <string>launch</string>
    <key>LSUIElement</key>
    <true/>
    <key>LSBackgroundOnly</key>
    <false/>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

# 8. Create the DMG
echo "Creating DMG..."
DMG_PATH="$DIST_DIR/$APP_NAME.dmg"
hdiutil create -volname "$APP_NAME" \
    -srcfolder "$DMG_DIR" \
    -ov -format UDZO \
    "$DMG_PATH"

echo ""
echo "=== Done! ==="
echo "DMG created at: $DMG_PATH"
echo "Size: $(du -h "$DMG_PATH" | cut -f1)"
