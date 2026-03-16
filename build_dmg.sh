#!/bin/bash
# Build a self-contained Claude-o-Meter.dmg for distribution.
# Uses PyInstaller to embed the Python interpreter + all dependencies so
# the app runs on any Mac without requiring a separate Python install.
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
APP_NAME="Claude-o-Meter"
DIST_DIR="$SCRIPT_DIR/dist"
BUILD_VENV="$SCRIPT_DIR/.build_venv"

# Determine version: use BUILD_VERSION env var, or read from VERSION file
if [ -n "$BUILD_VERSION" ]; then
    VERSION="$BUILD_VERSION"
else
    VERSION=$(cat "$SCRIPT_DIR/VERSION")
fi

echo "=== Building $APP_NAME.dmg (v$VERSION) ==="
echo ""

# 1. Find a stable system Python for building.
# Prefer /usr/local/bin or /opt/homebrew/bin (Homebrew) over /usr/bin (macOS
# system Python, often old). Avoid pyenv/asdf shims — they may not be present
# on end-user machines and PyInstaller needs a real interpreter path.
BUILD_PYTHON=""
for candidate in /usr/local/bin/python3 /opt/homebrew/bin/python3 /usr/bin/python3; do
    if [ -x "$candidate" ]; then
        BUILD_PYTHON="$candidate"
        break
    fi
done
if [ -z "$BUILD_PYTHON" ]; then
    echo "ERROR: No system Python3 found"
    exit 1
fi
echo "Using $BUILD_PYTHON ($($BUILD_PYTHON --version 2>&1))"

# 2. Create a clean build venv with all dependencies + PyInstaller
rm -rf "$BUILD_VENV"
"$BUILD_PYTHON" -m venv "$BUILD_VENV"
echo "Installing dependencies..."
"$BUILD_VENV/bin/pip" install --quiet --index-url https://pypi.org/simple/ \
    -r "$SCRIPT_DIR/requirements.txt" \
    pyinstaller

# 3. Clean previous build artifacts
rm -rf "$SCRIPT_DIR/build" "$DIST_DIR"

# 4. Build self-contained .app with PyInstaller
echo "Building app bundle with PyInstaller..."
"$BUILD_VENV/bin/pyinstaller" \
    --windowed \
    --name "$APP_NAME" \
    --icon "$SCRIPT_DIR/AppIcon.icns" \
    --add-data "$SCRIPT_DIR/VERSION:." \
    --add-data "$SCRIPT_DIR/logo.png:." \
    --osx-bundle-identifier com.local.claude-o-meter \
    --distpath "$DIST_DIR" \
    --workpath "$SCRIPT_DIR/build" \
    --noconfirm \
    "$SCRIPT_DIR/claude_meter.py"

APP_DIR="$DIST_DIR/$APP_NAME.app"
CONTENTS="$APP_DIR/Contents"

# 5. Patch Info.plist — add LSUIElement (hide dock icon), version, menu bar keys
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $VERSION" "$CONTENTS/Info.plist"
/usr/libexec/PlistBuddy -c "Add :CFBundleVersion string $VERSION" "$CONTENTS/Info.plist" 2>/dev/null \
    || /usr/libexec/PlistBuddy -c "Set :CFBundleVersion $VERSION" "$CONTENTS/Info.plist"
/usr/libexec/PlistBuddy -c "Add :LSUIElement bool true" "$CONTENTS/Info.plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :LSBackgroundOnly bool false" "$CONTENTS/Info.plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :NSMenuBarItemProviding bool true" "$CONTENTS/Info.plist" 2>/dev/null || true

# 6. Create an install/upgrade helper script next to the .app in the DMG.
# This handles quitting a running instance before replacing it.
DMG_STAGE="$DIST_DIR/dmg"
mkdir -p "$DMG_STAGE"
mv "$APP_DIR" "$DMG_STAGE/"
APP_DIR="$DMG_STAGE/$APP_NAME.app"

cat > "$DMG_STAGE/Install.command" << 'INSTALL_SCRIPT'
#!/bin/bash
# Install or upgrade Claude-o-Meter.
set -e

APP_NAME="Claude-o-Meter"
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

# Launch the new version
echo "Launching $APP_NAME..."
open -a "$APP_NAME"

echo ""
echo "Done! $APP_NAME is installed and running."
echo "You can now close this window and eject the disk image."
INSTALL_SCRIPT
chmod +x "$DMG_STAGE/Install.command"

# 7. Add Applications symlink for drag-to-install
ln -s /Applications "$DMG_STAGE/Applications"

# 8. Create the DMG
echo "Creating DMG..."
DMG_PATH="$DIST_DIR/$APP_NAME.dmg"

hdiutil create -volname "$APP_NAME" \
    -srcfolder "$DMG_STAGE" \
    -ov -format UDZO \
    "$DMG_PATH"

# 9. Clean up build artifacts
rm -rf "$BUILD_VENV" "$SCRIPT_DIR/Claude-o-Meter.spec"

echo ""
echo "=== Done! ==="
echo "DMG created at: $DMG_PATH"
echo "Size: $(du -h "$DMG_PATH" | cut -f1)"
echo ""
echo "Users can install by opening the DMG and dragging $APP_NAME to Applications."
