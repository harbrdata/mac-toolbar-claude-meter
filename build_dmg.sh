#!/bin/bash
# Build a self-contained Claude-o-Meter.dmg for distribution.
# Creates a styled DMG with background image showing drag-to-install arrow.
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
mkdir -p "$DMG_STAGE/.background"
mv "$APP_DIR" "$DMG_STAGE/"

# Applications symlink for drag-to-install
ln -s /Applications "$DMG_STAGE/Applications"

# Copy background image
cp "$SCRIPT_DIR/dmg_background.png" "$DMG_STAGE/.background/background.png"

# 6. Create a read-write DMG, style it, then convert to compressed
echo "Creating styled DMG..."
DMG_RW="$DIST_DIR/$APP_NAME-rw.dmg"
DMG_PATH="$DIST_DIR/$APP_NAME.dmg"

# Create read-write DMG
hdiutil create -volname "$APP_NAME" \
    -srcfolder "$DMG_STAGE" \
    -ov -format UDRW \
    "$DMG_RW"

# Mount it
MOUNT_DIR=$(hdiutil attach -readwrite -noverify "$DMG_RW" | grep "/Volumes/" | sed 's/.*\/Volumes/\/Volumes/')
echo "Mounted at: $MOUNT_DIR"

# Apply Finder window styling via AppleScript
osascript << APPLESCRIPT
tell application "Finder"
    tell disk "$APP_NAME"
        open
        set current view of container window to icon view
        set toolbar visible of container window to false
        set statusbar visible of container window to false
        set bounds of container window to {100, 100, 760, 570}
        set theViewOptions to icon view options of container window
        set arrangement of theViewOptions to not arranged
        set icon size of theViewOptions to 96
        set background picture of theViewOptions to file ".background:background.png"
        set position of item "$APP_NAME.app" of container window to {165, 240}
        set position of item "Applications" of container window to {495, 240}
        close
        open
        update without registering applications
        delay 2
        close
    end tell
end tell
APPLESCRIPT

# Ensure Finder releases the volume
sync
hdiutil detach "$MOUNT_DIR"

# Convert to compressed read-only DMG
hdiutil convert "$DMG_RW" -format UDZO -o "$DMG_PATH"
rm -f "$DMG_RW"

echo ""
echo "=== Done! ==="
echo "DMG created at: $DMG_PATH"
echo "Size: $(du -h "$DMG_PATH" | cut -f1)"
echo ""
echo "Install by opening the DMG and dragging $APP_NAME to Applications."
