#!/bin/bash
# Build a self-contained Claude-o-Meter.dmg for distribution.
# The DMG contains the .app bundle and an Applications symlink —
# users drag the app to Applications to install.
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

# Applications symlink for drag-to-install
ln -s /Applications "$DMG_STAGE/Applications"

# 6. Create the DMG
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
echo "Install by opening the DMG and dragging $APP_NAME to Applications."
