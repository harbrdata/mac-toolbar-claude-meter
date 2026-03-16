#!/bin/bash
# Build a self-contained Claude-o-Meter.dmg containing a .pkg installer.
# The .pkg provides a native macOS installer UI that:
#   - Installs the app to /Applications
#   - Configures it to start at login (Launch Agent)
#   - Launches the app after installation
set -e

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

# 5. Build the .pkg installer
echo "Building .pkg installer..."
PKG_DIR="$DIST_DIR/pkg"
PKG_SCRIPTS="$PKG_DIR/scripts"
PKG_RESOURCES="$PKG_DIR/resources"
COMPONENT_PKG="$PKG_DIR/component.pkg"
FINAL_PKG="$DIST_DIR/$APP_NAME.pkg"

mkdir -p "$PKG_SCRIPTS" "$PKG_RESOURCES"

# 5a. Preinstall script — quit any running instance
cat > "$PKG_SCRIPTS/preinstall" << 'PREINSTALL'
#!/bin/bash
APP_NAME="Claude-o-Meter"
if pgrep -xq "$APP_NAME"; then
    osascript -e "quit app \"$APP_NAME\"" 2>/dev/null || true
    sleep 2
    pkill -x "$APP_NAME" 2>/dev/null || true
    sleep 1
fi
exit 0
PREINSTALL
chmod +x "$PKG_SCRIPTS/preinstall"

# 5b. Postinstall script — install Launch Agent and launch the app
cat > "$PKG_SCRIPTS/postinstall" << 'POSTINSTALL'
#!/bin/bash
APP_NAME="Claude-o-Meter"
LABEL="com.local.claude-o-meter"
CURRENT_USER="$USER"
if [ "$CURRENT_USER" = "root" ]; then
    CURRENT_USER="$SUDO_USER"
fi
HOME_DIR=$(eval echo "~$CURRENT_USER")
LAUNCH_AGENTS_DIR="$HOME_DIR/Library/LaunchAgents"
PLIST_PATH="$LAUNCH_AGENTS_DIR/$LABEL.plist"

# Create LaunchAgents directory if needed
mkdir -p "$LAUNCH_AGENTS_DIR"

# Write the Launch Agent plist
cat > "$PLIST_PATH" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$LABEL</string>
    <key>ProgramArguments</key>
    <array>
        <string>open</string>
        <string>-a</string>
        <string>$APP_NAME</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardErrorPath</key>
    <string>/tmp/claude_meter.log</string>
</dict>
</plist>
PLIST

# Fix ownership (installer runs as root)
chown "$CURRENT_USER" "$PLIST_PATH"

# Bootstrap the Launch Agent
UID_NUM=$(id -u "$CURRENT_USER")
launchctl bootstrap "gui/$UID_NUM" "$PLIST_PATH" 2>/dev/null || true

# Launch the app as the current user
sudo -u "$CURRENT_USER" open -a "$APP_NAME"

exit 0
POSTINSTALL
chmod +x "$PKG_SCRIPTS/postinstall"

# 5c. Welcome page for the installer
cat > "$PKG_RESOURCES/welcome.html" << 'WELCOME'
<html>
<body style="font-family: -apple-system, Helvetica Neue, sans-serif; font-size: 13px; line-height: 1.5; padding: 10px;">
<h2>Claude-o-Meter</h2>
<p>A lightweight macOS menu bar app that shows your Claude Code plan usage at a glance.</p>
<p>This installer will:</p>
<ul>
    <li>Install Claude-o-Meter to <b>/Applications</b></li>
    <li>Configure it to <b>start automatically at login</b></li>
    <li>Launch the app when installation completes</li>
</ul>
<p><b>Prerequisite:</b> You must have an active Claude Code session.
If you haven't already, open Terminal and run:</p>
<pre style="background: #f0f0f0; padding: 8px 12px; border-radius: 4px; font-size: 12px;">claude login</pre>
</body>
</html>
WELCOME

# 5d. Conclusion page
cat > "$PKG_RESOURCES/conclusion.html" << 'CONCLUSION'
<html>
<body style="font-family: -apple-system, Helvetica Neue, sans-serif; font-size: 13px; line-height: 1.5; padding: 10px;">
<h2>Installation Complete</h2>
<p>Claude-o-Meter is now running in your menu bar. Look for the gauge icon at the top of your screen.</p>
<p>Click the icon to see:</p>
<ul>
    <li>Usage breakdown across all windows</li>
    <li>Reset countdowns</li>
    <li>Settings for refresh interval and alert threshold</li>
</ul>
<p>The app is configured to start automatically at login. You can toggle this from the menu.</p>
<p>To uninstall, just drag Claude-o-Meter from Applications to the Trash — it cleans up automatically.</p>
</body>
</html>
CONCLUSION

# 5e. Build component package from the .app bundle
# Stage the app in a payload root matching the install location
PKG_ROOT="$PKG_DIR/root"
mkdir -p "$PKG_ROOT/Applications"
cp -R "$APP_DIR" "$PKG_ROOT/Applications/"

pkgbuild \
    --root "$PKG_ROOT" \
    --identifier "$BUNDLE_ID" \
    --version "$VERSION" \
    --scripts "$PKG_SCRIPTS" \
    "$COMPONENT_PKG"

# 5f. Create distribution XML for productbuild
DIST_XML="$PKG_DIR/distribution.xml"
cat > "$DIST_XML" << DIST
<?xml version="1.0" encoding="UTF-8"?>
<installer-gui-script minSpecVersion="2">
    <title>Claude-o-Meter</title>
    <welcome file="welcome.html" mime-type="text/html"/>
    <conclusion file="conclusion.html" mime-type="text/html"/>
    <options customize="never" require-scripts="false"/>
    <domains enable_localSystem="true"/>
    <pkg-ref id="$BUNDLE_ID"/>
    <choices-outline>
        <line choice="default">
            <line choice="$BUNDLE_ID"/>
        </line>
    </choices-outline>
    <choice id="default"/>
    <choice id="$BUNDLE_ID" visible="false">
        <pkg-ref id="$BUNDLE_ID"/>
    </choice>
    <pkg-ref id="$BUNDLE_ID" version="$VERSION" onConclusion="none">component.pkg</pkg-ref>
</installer-gui-script>
DIST

# 5g. Build the final .pkg with the distribution and resources
productbuild \
    --distribution "$DIST_XML" \
    --resources "$PKG_RESOURCES" \
    --package-path "$PKG_DIR" \
    "$FINAL_PKG"

echo "PKG created: $FINAL_PKG"

# 6. Stage DMG contents
echo "Staging DMG..."
DMG_STAGE="$DIST_DIR/dmg"
mkdir -p "$DMG_STAGE"

# Copy the .pkg into the DMG
cp "$FINAL_PKG" "$DMG_STAGE/"

# 7. Add README
cp "$SCRIPT_DIR/DMG_README.txt" "$DMG_STAGE/README.txt"

# 8. Create the DMG
echo "Creating DMG..."
DMG_PATH="$DIST_DIR/$APP_NAME.dmg"

hdiutil create -volname "$APP_NAME" \
    -srcfolder "$DMG_STAGE" \
    -ov -format UDZO \
    "$DMG_PATH"

# Clean up intermediate files
rm -rf "$PKG_DIR" "$APP_DIR" "$FINAL_PKG"

echo ""
echo "=== Done! ==="
echo "DMG created at: $DMG_PATH"
echo "Size: $(du -h "$DMG_PATH" | cut -f1)"
echo ""
echo "Users install by opening the DMG and double-clicking $APP_NAME.pkg"
