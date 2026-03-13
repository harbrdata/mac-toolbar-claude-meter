#!/bin/bash
# Uninstall Claude-o-Meter: stop the app and remove the login launch agent.
# Safe to run multiple times.
set -e

PLIST_LABEL="com.local.claude-o-meter"
PLIST_PATH="$HOME/Library/LaunchAgents/$PLIST_LABEL.plist"

echo "=== Claude-o-Meter Uninstaller ==="
echo ""

ANYTHING_TO_DO=false

# Stop running instances
if pgrep -f "claude_meter.py" >/dev/null 2>&1 || pgrep -f "Claude-o-Meter" >/dev/null 2>&1; then
    echo "Stopping Claude-o-Meter..."
    pkill -f "claude_meter.py" 2>/dev/null || true
    pkill -f "Claude-o-Meter" 2>/dev/null || true
    ANYTHING_TO_DO=true
fi

# Remove launch agent
if [ -f "$PLIST_PATH" ]; then
    echo "Removing launch agent..."
    launchctl bootout "gui/$(id -u)/$PLIST_LABEL" 2>/dev/null || true
    rm -f "$PLIST_PATH"
    echo "Launch agent removed."
    ANYTHING_TO_DO=true
fi

echo ""
if [ "$ANYTHING_TO_DO" = false ]; then
    echo "Claude-o-Meter is not installed. Nothing to do."
else
    echo "Done! Claude-o-Meter will no longer start on login."
    echo "The app files are still in $(cd "$(dirname "$0")" && pwd) — delete manually if desired."
fi
