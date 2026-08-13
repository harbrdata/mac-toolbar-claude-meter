#!/bin/bash
# Stamp the release version into VERSION/Cargo.toml, then build the distributable DMG.
#
# Invoked by semantic-release via @semantic-release/exec's prepareCmd, so it runs
# only when a release is actually being published. The resulting
# dist/Claude-o-Meter.dmg is uploaded as a release asset by @semantic-release/github.
set -euo pipefail

VERSION="${1:?usage: prepare_release.sh <version>}"
REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"

echo "$VERSION" > "$REPO_DIR/VERSION"

# Rewrite via a temp file rather than `sed -i`: BSD sed (macOS) requires a backup
# suffix argument that GNU sed rejects, so the in-place form is not portable.
TMP_TOML="$(mktemp)"
sed "s/^version = \".*\"/version = \"$VERSION\"/" "$REPO_DIR/Cargo.toml" > "$TMP_TOML"
mv "$TMP_TOML" "$REPO_DIR/Cargo.toml"

# cargo build inside build_dmg.sh refreshes Cargo.lock with the new version, so the
# lockfile is up to date before @semantic-release/git commits the release assets.
BUILD_VERSION="$VERSION" "$REPO_DIR/build_dmg.sh"

# @semantic-release/github only warns when an asset glob matches nothing, which would
# publish a release with no DMG. Fail the release instead.
DMG_PATH="$REPO_DIR/dist/Claude-o-Meter.dmg"
if [ ! -f "$DMG_PATH" ]; then
    echo "ERROR: expected DMG not found at $DMG_PATH" >&2
    exit 1
fi
