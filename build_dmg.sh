#!/bin/bash
# Build a self-contained Claude-o-Meter.dmg for distribution.
# The DMG contains the .app bundle, an Install.command script, and an
# Applications symlink for drag-to-install.
set -e
export COPYFILE_DISABLE=1

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
APP_NAME="Claude-o-Meter"
DIST_DIR="$SCRIPT_DIR/dist"
BUNDLE_ID="com.local.claude-o-meter"

# Determine version: use BUILD_VERSION env var, or read from VERSION file
if [ -n "$BUILD_VERSION" ]; then
    VERSION="$BUILD_VERSION"
else
    VERSION=$(cat "$SCRIPT_DIR/VERSION")
fi

echo "=== Building $APP_NAME.dmg (v$VERSION) ==="
echo ""

# 1. Build release binary
echo "Building Rust binary..."
cargo build --release --manifest-path "$SCRIPT_DIR/Cargo.toml"

BINARY="$SCRIPT_DIR/target/release/claude-o-meter"
if [ ! -f "$BINARY" ]; then
    echo "ERROR: Binary not found at $BINARY"
    exit 1
fi
echo "Binary size: $(du -h "$BINARY" | cut -f1)"

# 2. Create .app bundle
echo "Creating app bundle..."
APP_DIR="$DIST_DIR/$APP_NAME.app"
CONTENTS="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS/MacOS"
RESOURCES_DIR="$CONTENTS/Resources"

rm -rf "$DIST_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

cp "$BINARY" "$MACOS_DIR/$APP_NAME"

# 3. Copy icon if present
if [ -f "$SCRIPT_DIR/AppIcon.icns" ]; then
    cp "$SCRIPT_DIR/AppIcon.icns" "$RESOURCES_DIR/"
fi

# 4. Write Info.plist
cat > "$CONTENTS/Info.plist" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>$APP_NAME</string>
    <key>CFBundleDisplayName</key>
    <string>$APP_NAME</string>
    <key>CFBundleIdentifier</key>
    <string>$BUNDLE_ID</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleExecutable</key>
    <string>$APP_NAME</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSUIElement</key>
    <true/>
    <key>LSBackgroundOnly</key>
    <false/>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSMenuBarItemProviding</key>
    <true/>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
</dict>
</plist>
EOF

# 5. Stage DMG contents
echo "Staging DMG..."
DMG_STAGE="$DIST_DIR/dmg"
mkdir -p "$DMG_STAGE"
mv "$APP_DIR" "$DMG_STAGE/"
APP_DIR="$DMG_STAGE/$APP_NAME.app"

# 6. Create install/upgrade helper script
cat > "$DMG_STAGE/Install.command" << 'INSTALL_SCRIPT'
#!/bin/bash
# Install or upgrade Claude-o-Meter.
set -e

APP_NAME="Claude-o-Meter"
LABEL="com.local.claude-o-meter"
DMG_APP="$(cd "$(dirname "$0")" && pwd)/$APP_NAME.app"

echo "=== Installing $APP_NAME ==="
echo ""

# Quit any running instance
if pgrep -xq "$APP_NAME"; then
    echo "Quitting running instance..."
    osascript -e "quit app \"$APP_NAME\"" 2>/dev/null || true
    sleep 2
    pkill -x "$APP_NAME" 2>/dev/null || true
    sleep 1
fi

# Copy to /Applications
echo "Copying to /Applications..."
rm -rf "/Applications/$APP_NAME.app"
cp -R "$DMG_APP" "/Applications/"

# Install Launch Agent for start-at-login
PLIST_PATH="$HOME/Library/LaunchAgents/$LABEL.plist"
mkdir -p "$HOME/Library/LaunchAgents"
cat > "$PLIST_PATH" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$LABEL</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Applications/$APP_NAME.app/Contents/MacOS/$APP_NAME</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardErrorPath</key>
    <string>/tmp/claude_meter.log</string>
</dict>
</plist>
PLIST

# Load the Launch Agent
launchctl bootstrap "gui/$(id -u)" "$PLIST_PATH" 2>/dev/null || true

# Launch the new version
echo "Launching $APP_NAME..."
open "/Applications/$APP_NAME.app"

echo ""
echo "Done! $APP_NAME is installed and running."
echo "You can now close this window and eject the disk image."
INSTALL_SCRIPT
chmod +x "$DMG_STAGE/Install.command"

# 7. Add Applications symlink for drag-to-install
ln -s /Applications "$DMG_STAGE/Applications"

# 8. Add README
cp "$SCRIPT_DIR/DMG_README.txt" "$DMG_STAGE/README.txt"

# 9. Create the DMG
echo "Creating DMG..."
DMG_PATH="$DIST_DIR/$APP_NAME.dmg"

hdiutil create -volname "$APP_NAME" \
    -srcfolder "$DMG_STAGE" \
    -ov -format UDZO \
    "$DMG_PATH"

echo ""
echo "=== Done! ==="
echo "DMG created at: $DMG_PATH"
echo "Size: $(du -h "$DMG_PATH" | cut -f1)"
echo ""
echo "Users can install by opening the DMG and either:"
echo "  - Dragging $APP_NAME to Applications"
echo "  - Double-clicking Install.command"
