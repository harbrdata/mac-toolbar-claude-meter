#!/bin/bash
# Build a self-contained Claude-o-Meter.dmg for distribution.
# Creates a styled DMG with background image showing drag-to-install arrow.
set -e
export COPYFILE_DISABLE=1

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
APP_NAME="Claude-o-Meter"
DIST_DIR="$SCRIPT_DIR/dist"
BUNDLE_ID="com.local.claude-o-meter"

# --restyle regenerates dmg_ds_store from a live Finder session on a GUI Mac.
# Default (no flag) is the CI path: bake the committed .DS_Store, no Finder involved.
RESTYLE=0
if [ "$1" = "--restyle" ]; then
    RESTYLE=1
fi

# Echo the single mountpoint of an attached image. `set -e` cannot catch a failed
# attach on its own — the mountpoint is extracted through a pipeline whose last
# command always succeeds — so an empty or multi-line result would otherwise flow
# on and surface as a misleading styling error.
attach_dmg() {
    local out mount
    out=$(hdiutil attach "$@") || {
        echo "ERROR: hdiutil attach failed:" >&2
        echo "$out" >&2
        return 1
    }
    mount=$(printf '%s\n' "$out" | sed -n 's|.*\(/Volumes/.*\)$|\1|p')
    if [ -z "$mount" ] || [ "$(printf '%s\n' "$mount" | wc -l)" -ne 1 ]; then
        echo "ERROR: expected exactly one /Volumes mountpoint from hdiutil attach, got: ${mount:-<none>}" >&2
        return 1
    fi
    printf '%s\n' "$mount"
}

# Determine version: use BUILD_VERSION env var, or read from VERSION file
if [ -n "$BUILD_VERSION" ]; then
    VERSION="$BUILD_VERSION"
else
    VERSION=$(cat "$SCRIPT_DIR/VERSION")
fi

echo "=== Building $APP_NAME.dmg (v$VERSION) ==="
echo ""

# 1. Build universal release binary (arm64 + x86_64)
echo "Building Rust binaries..."
CARGO="cargo"
# Use rustup toolchain if available (needed for cross-compilation targets)
if [ -x "/opt/homebrew/opt/rustup/bin/cargo" ]; then
    CARGO="/opt/homebrew/opt/rustup/bin/cargo"
    # Ensure cargo uses rustup's rustc (not Homebrew's) so it can find cross-compilation targets
    RUSTUP_RUSTC="$(rustup which rustc 2>/dev/null)"
    if [ -n "$RUSTUP_RUSTC" ]; then
        export RUSTC="$RUSTUP_RUSTC"
    fi
fi

$CARGO build --release --manifest-path "$SCRIPT_DIR/Cargo.toml" --target aarch64-apple-darwin
$CARGO build --release --manifest-path "$SCRIPT_DIR/Cargo.toml" --target x86_64-apple-darwin

ARM_BIN="$SCRIPT_DIR/target/aarch64-apple-darwin/release/claude-o-meter"
X86_BIN="$SCRIPT_DIR/target/x86_64-apple-darwin/release/claude-o-meter"
BINARY="$SCRIPT_DIR/target/release/claude-o-meter-universal"

if [ ! -f "$ARM_BIN" ] || [ ! -f "$X86_BIN" ]; then
    echo "ERROR: One or both binaries not found"
    exit 1
fi

lipo -create "$ARM_BIN" "$X86_BIN" -output "$BINARY"
echo "Universal binary size: $(du -h "$BINARY" | cut -f1)"

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

# 5. Codesign the .app bundle
# A stable signature is required so macOS Keychain "Always Allow" persists across launches.
# Set CODESIGN_IDENTITY to a Developer ID or certificate name for production signing.
# Default: ad-hoc signing (-), which is stable for a given binary and sufficient for local use.
CODESIGN_IDENTITY="${CODESIGN_IDENTITY:--}"
echo "Codesigning with identity: $CODESIGN_IDENTITY"
codesign --force --deep --sign "$CODESIGN_IDENTITY" "$APP_DIR"
codesign --verify "$APP_DIR"
echo "Codesign verified."

# 6. Stage DMG contents
echo "Staging DMG..."
DMG_STAGE="$DIST_DIR/dmg"
mkdir -p "$DMG_STAGE/.background"
mv "$APP_DIR" "$DMG_STAGE/"

# Install script — strips quarantine and re-signs after copy to preserve signature
cat > "$DMG_STAGE/Install.command" << 'INSTALL_SCRIPT'
#!/bin/bash
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

# Strip quarantine attributes and re-sign to ensure stable code signature
# (quarantine stripping can invalidate the original signature)
xattr -c "/Applications/$APP_NAME.app" 2>/dev/null || true
find "/Applications/$APP_NAME.app" -exec xattr -c {} \; 2>/dev/null || true
codesign --force --deep --sign - "/Applications/$APP_NAME.app" 2>/dev/null || true

# Launch
echo "Launching $APP_NAME..."
open "/Applications/$APP_NAME.app"

echo ""
echo "Done! $APP_NAME is running in your menu bar."
echo "You can close this window and eject the disk image."
INSTALL_SCRIPT
chmod +x "$DMG_STAGE/Install.command"

# Apply custom icon to Install.command
if command -v fileicon &>/dev/null && [ -f "$SCRIPT_DIR/InstallIcon.icns" ]; then
    fileicon set "$DMG_STAGE/Install.command" "$SCRIPT_DIR/InstallIcon.icns"
fi

# Copy background image
cp "$SCRIPT_DIR/dmg_background.png" "$DMG_STAGE/.background/background.png"

# 7. Create a read-write DMG, style it, then convert to compressed
# Unset COPYFILE_DISABLE so hdiutil preserves dotfiles (.VolumeIcon.icns, .background, .DS_Store)
unset COPYFILE_DISABLE
echo "Creating styled DMG..."
DMG_RW="$DIST_DIR/$APP_NAME-rw.dmg"
DMG_PATH="$DIST_DIR/$APP_NAME.dmg"

# Create read-write DMG (HFS+ required for .VolumeIcon.icns and Finder styling)
hdiutil create -volname "$APP_NAME" \
    -srcfolder "$DMG_STAGE" \
    -ov -format UDRW \
    -fs HFS+ \
    "$DMG_RW"

# Mount it
MOUNT_DIR=$(attach_dmg -readwrite -noverify "$DMG_RW")
echo "Mounted at: $MOUNT_DIR"

# A leftover volume of the same name makes macOS mount this one as
# "$APP_NAME 1", which breaks styling both ways: the restyle AppleScript
# addresses `disk "$APP_NAME"` and would style the stale volume, and the baked
# .DS_Store records its background relative to the volume name, so the image
# would ship with no background picture.
if [ "$MOUNT_DIR" != "/Volumes/$APP_NAME" ]; then
    echo "ERROR: mounted at $MOUNT_DIR, expected /Volumes/$APP_NAME."
    echo "       Detach the stale volume first: hdiutil detach '/Volumes/$APP_NAME'"
    hdiutil detach "$MOUNT_DIR" || hdiutil detach -force "$MOUNT_DIR"
    exit 1
fi

DS_STORE="$SCRIPT_DIR/dmg_ds_store"

if [ "$RESTYLE" = "1" ]; then
    # Positioning .background and .VolumeIcon.icns needs Finder to list them, so this
    # mode requires `defaults write com.apple.finder AppleShowAllFiles true`.
    # Finder can only record a position for an item that exists at styling time.
    # The real .VolumeIcon.icns is copied in AFTER styling (see below), because
    # Finder's "update without registering applications" deletes it — so a copy
    # is placed here too, just so Finder has something named .VolumeIcon.icns to
    # position. The .DS_Store position record is keyed on the filename, so it
    # still applies once the post-styling copy recreates the file.
    if [ -f "$SCRIPT_DIR/AppIcon.icns" ]; then
        cp "$SCRIPT_DIR/AppIcon.icns" "$MOUNT_DIR/.VolumeIcon.icns"
    fi

    # Apply Finder window styling via AppleScript (must run BEFORE the real copy of
    # .VolumeIcon.icns further down, as Finder's "update without registering
    # applications" deletes it).
    # This mode needs a GUI session on a real Mac; there is no fallback — a failed
    # restyle must not silently produce a DMG with a stale or missing layout.
    if ! osascript << APPLESCRIPT
tell application "Finder"
    tell disk "$APP_NAME"
        open
        set current view of container window to icon view
        set toolbar visible of container window to false
        set statusbar visible of container window to false
        set bounds of container window to {200, 150, 680, 550}
        set theViewOptions to icon view options of container window
        set arrangement of theViewOptions to not arranged
        set icon size of theViewOptions to 80
        set background picture of theViewOptions to file ".background:background.png"
        # Finder anchors an item by its cell's top-left, and the cell is wider than
        # the icon, so these sit ~85pt left of where the icons appear: the pair
        # centres on 160 and 320, thirds of the 480pt-wide window.
        set position of item "$APP_NAME.app" of container window to {76, 200}
        set position of item "Install.command" of container window to {236, 200}
        # These two items only matter to macOS, not to anyone browsing the DMG,
        # so they sit well below the 480x400 content area to stay off-screen
        # even with hidden files shown. Their x stays low (10 and 150) so the
        # 128pt-wide icon cell doesn't push the view sideways and contribute
        # to the horizontal scrollbar.
        set position of item ".background" of container window to {10, 700}
        set position of item ".VolumeIcon.icns" of container window to {150, 700}
        close
        open
        update without registering applications
        delay 3
        close
    end tell
end tell
APPLESCRIPT
    then
        echo "ERROR: Finder styling failed — no GUI session, or Finder refused the layout."
        hdiutil detach "$MOUNT_DIR" || hdiutil detach -force "$MOUNT_DIR"
        exit 1
    fi

    # Give Finder time to flush .DS_Store to disk
    sleep 2

    # Finder can write a layout with the window bounds and icon positions but no
    # background reference, which looks like a success but bakes a blank DMG.
    if ! LC_ALL=C grep -q "backgroundImageAlias" "$MOUNT_DIR/.DS_Store"; then
        echo "ERROR: Finder recorded no background image — refusing to bake a blank layout."
        hdiutil detach "$MOUNT_DIR" || hdiutil detach -force "$MOUNT_DIR"
        exit 1
    fi

    # Save the freshly-styled layout so it can be committed and baked into future builds.
    cp "$MOUNT_DIR/.DS_Store" "$DS_STORE"
    echo "Saved styled .DS_Store to $DS_STORE"
else
    # CI path: no Finder involved. A live Finder session isn't available on the
    # CircleCI macOS runner, so styling is baked ahead of time instead of generated
    # at build time — see --restyle above to regenerate this file.
    if [ ! -f "$DS_STORE" ]; then
        echo "ERROR: $DS_STORE not found. Run '$0 --restyle' on a GUI Mac to generate it,"
        echo "       or this build would ship an unstyled DMG."
        hdiutil detach "$MOUNT_DIR" || hdiutil detach -force "$MOUNT_DIR"
        exit 1
    fi
    # The baked .DS_Store only resolves its background image when the volume name
    # matches the one it was recorded on ("$APP_NAME"), since Finder stores the
    # background reference relative to that volume.
    cp "$DS_STORE" "$MOUNT_DIR/.DS_Store"
    echo "Applied baked .DS_Store."
fi

# Copy volume icon AFTER Finder styling (Finder's "update" command deletes it)
if [ -f "$SCRIPT_DIR/AppIcon.icns" ]; then
    cp "$SCRIPT_DIR/AppIcon.icns" "$MOUNT_DIR/.VolumeIcon.icns"
    SetFile -a C "$MOUNT_DIR"
    echo "Volume icon set."
fi

# Remove volume noise macOS creates on a mounted read-write image; it has no place
# in the shipped DMG and .fseventsd in particular can bloat the compressed image.
rm -rf "$MOUNT_DIR/.fseventsd" "$MOUNT_DIR/.Trashes"

# Ensure Finder releases the volume
sync
hdiutil detach "$MOUNT_DIR" || hdiutil detach -force "$MOUNT_DIR"

# Convert to compressed read-only DMG
hdiutil convert "$DMG_RW" -format UDZO -o "$DMG_PATH"
rm -f "$DMG_RW"

# Verify the styling actually made it into the shipped DMG rather than trusting
# the earlier steps silently — this is what catches a regression to an unstyled release.
echo "Verifying DMG styling..."
VERIFY_MOUNT=$(attach_dmg -readonly -noverify -nobrowse "$DMG_PATH")
VERIFY_FAILED=0

if [ ! -f "$VERIFY_MOUNT/.DS_Store" ]; then
    echo "ERROR: .DS_Store missing from shipped DMG."
    VERIFY_FAILED=1
elif ! LC_ALL=C grep -q "bwsp" "$VERIFY_MOUNT/.DS_Store" \
    || ! LC_ALL=C grep -q "icvp" "$VERIFY_MOUNT/.DS_Store" \
    || ! LC_ALL=C grep -q "Iloc" "$VERIFY_MOUNT/.DS_Store"; then
    echo "ERROR: .DS_Store is missing window bounds (bwsp), icon view options (icvp), or icon positions (Iloc)."
    VERIFY_FAILED=1
elif ! LC_ALL=C grep -q "backgroundImageAlias" "$VERIFY_MOUNT/.DS_Store"; then
    # Present but unreferenced background art still opens as a blank window.
    echo "ERROR: .DS_Store has no background image reference (backgroundImageAlias)."
    VERIFY_FAILED=1
fi

if [ ! -f "$VERIFY_MOUNT/.background/background.png" ]; then
    echo "ERROR: .background/background.png missing from shipped DMG."
    VERIFY_FAILED=1
fi

if [ -e "$VERIFY_MOUNT/.fseventsd" ]; then
    echo "ERROR: .fseventsd present in shipped DMG (should have been stripped before detach)."
    VERIFY_FAILED=1
fi

hdiutil detach "$VERIFY_MOUNT" || hdiutil detach -force "$VERIFY_MOUNT"

if [ "$VERIFY_FAILED" = "1" ]; then
    echo "ERROR: DMG styling verification failed — see above."
    exit 1
fi
echo "DMG styling verified."

# Set custom icon on the DMG file itself (visible on desktop / in Finder)
# Must unset COPYFILE_DISABLE so the resource fork is written correctly
if command -v fileicon &>/dev/null && [ -f "$SCRIPT_DIR/AppIcon.icns" ]; then
    unset COPYFILE_DISABLE
    fileicon set "$DMG_PATH" "$SCRIPT_DIR/AppIcon.icns"
fi

echo ""
echo "=== Done! ==="
echo "DMG created at: $DMG_PATH"
echo "Size: $(du -h "$DMG_PATH" | cut -f1)"
echo ""
echo "Install by opening the DMG and double-clicking Install."
