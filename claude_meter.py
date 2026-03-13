#!/usr/bin/env python3
"""Claude-o-Meter — macOS menu bar app showing Claude Code plan usage."""

import json
import logging
import subprocess
import time
from datetime import datetime, timezone

import objc
import requests
from AppKit import (
    NSApplication,
    NSApplicationActivationPolicyAccessory,
    NSBezierPath,
    NSColor,
    NSFont,
    NSFontAttributeName,
    NSForegroundColorAttributeName,
    NSImage,
    NSMenu,
    NSMenuItem,
    NSObject,
    NSStatusBar,
    NSTimer,
    NSVariableStatusItemLength,
)
from PyObjCTools import AppHelper

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
    handlers=[logging.StreamHandler()],
)
log = logging.getLogger("claude_meter")

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

KEYCHAIN_SERVICE = "Claude Code-credentials"
USAGE_URL = "https://api.anthropic.com/api/oauth/usage"
USER_AGENT = "claude-code/2.1.70"
ANTHROPIC_BETA = "oauth-2025-04-20"

POLL_INTERVAL_DEFAULT = 60  # seconds
POLL_INTERVAL_OPTIONS = [30, 60, 120, 300]  # seconds

THRESHOLD_WARNING = 0.60
THRESHOLD_DANGER = 0.80
THRESHOLD_CRITICAL = 0.95


# ---------------------------------------------------------------------------
# Keychain
# ---------------------------------------------------------------------------


def read_keychain_credentials() -> dict | None:
    """Read Claude Code credentials from macOS Keychain."""
    try:
        result = subprocess.run(
            ["security", "find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        if result.returncode != 0:
            log.error(
                "Keychain read failed (rc=%d): %s",
                result.returncode,
                result.stderr.strip(),
            )
            return None
        raw = result.stdout.strip()
        data = json.loads(raw)
        if "claudeAiOauth" in data:
            data = data["claudeAiOauth"]
        return data
    except (json.JSONDecodeError, subprocess.TimeoutExpired) as exc:
        log.error("Failed to parse keychain credentials: %s", exc)
        return None


def get_access_token(creds: dict) -> str | None:
    """Extract access token, refreshing if expired."""
    expires_at = creds.get("expiresAt", 0)
    if expires_at and time.time() * 1000 < expires_at:
        return creds.get("accessToken")

    refresh_token = creds.get("refreshToken")
    if not refresh_token:
        return creds.get("accessToken")

    try:
        resp = requests.post(
            "https://api.anthropic.com/api/oauth/token",
            json={
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
            },
            headers={"Content-Type": "application/json"},
            timeout=15,
        )
        if resp.ok:
            new_creds = resp.json()
            return new_creds.get("access_token") or new_creds.get("accessToken")
    except requests.RequestException as exc:
        log.warning("Token refresh failed: %s", exc)

    return creds.get("accessToken")


# ---------------------------------------------------------------------------
# API Client
# ---------------------------------------------------------------------------


def fetch_usage(access_token: str) -> dict | None:
    """Fetch usage data from the Anthropic API."""
    try:
        resp = requests.get(
            USAGE_URL,
            headers={
                "Authorization": f"Bearer {access_token}",
                "User-Agent": USER_AGENT,
                "anthropic-beta": ANTHROPIC_BETA,
            },
            timeout=15,
        )
        if resp.status_code == 401:
            log.warning("API returned 401 — re-login with `claude login`.")
            return None
        if resp.status_code == 429:
            log.warning("API rate-limited (429).")
            return None
        resp.raise_for_status()
        return resp.json()
    except requests.RequestException as exc:
        log.error("Usage API request failed: %s", exc)
        return None


# ---------------------------------------------------------------------------
# Usage parsing
# ---------------------------------------------------------------------------


USAGE_WINDOWS = {
    "five_hour": "5h",
    "seven_day": "7d",
    "seven_day_opus": "Opus",
    "seven_day_sonnet": "Sonnet",
    "seven_day_cowork": "Cowork",
    "seven_day_oauth_apps": "OAuth",
}


def parse_usage(data: dict) -> list[dict]:
    """Parse usage windows from API response."""
    windows = []
    for key, label in USAGE_WINDOWS.items():
        window = data.get(key)
        if not window:
            continue
        raw_util = window.get("utilization", 0.0)
        util = raw_util / 100.0
        resets_at = window.get("resets_at")
        windows.append({"label": label, "utilization": util, "resets_at": resets_at})
    return windows


def format_reset_time(resets_at: str | None) -> str:
    """Format reset time as a human-readable countdown."""
    if not resets_at:
        return "unknown"
    try:
        reset_dt = datetime.fromisoformat(resets_at.replace("Z", "+00:00"))
        now = datetime.now(timezone.utc)
        delta = reset_dt - now
        total_seconds = int(delta.total_seconds())
        if total_seconds <= 0:
            return "now"
        hours, remainder = divmod(total_seconds, 3600)
        minutes, _ = divmod(remainder, 60)
        if hours > 0:
            return f"{hours}h {minutes}m"
        return f"{minutes}m"
    except (ValueError, TypeError):
        return "unknown"


def usage_color(utilization: float) -> NSColor:
    """Return an NSColor based on utilization level."""
    if utilization >= THRESHOLD_CRITICAL:
        return NSColor.redColor()
    if utilization >= THRESHOLD_DANGER:
        return NSColor.orangeColor()
    if utilization >= THRESHOLD_WARNING:
        return NSColor.yellowColor()
    return NSColor.greenColor()


def usage_color_text(utilization: float) -> str:
    """Return a colored circle emoji for menu items."""
    if utilization >= THRESHOLD_CRITICAL:
        return "\U0001f534"
    if utilization >= THRESHOLD_DANGER:
        return "\U0001f7e0"
    if utilization >= THRESHOLD_WARNING:
        return "\U0001f7e1"
    return "\U0001f7e2"


# ---------------------------------------------------------------------------
# Gauge icon drawing
# ---------------------------------------------------------------------------


def create_gauge_icon(utilization: float, size: int = 18) -> NSImage:
    """Draw a circular progress ring with percentage in the center."""
    from Foundation import NSString

    img = NSImage.alloc().initWithSize_((size, size))
    img.lockFocus()

    cx, cy = size / 2, size / 2
    radius = (size / 2) - 2.5
    line_width = 2.0

    # Background ring (full circle)
    track = NSBezierPath.bezierPath()
    track.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_(
        (cx, cy), radius, 0, 360
    )
    NSColor.darkGrayColor().setStroke()
    track.setLineWidth_(line_width)
    track.stroke()

    # Filled ring (starts at top = 90°, goes clockwise)
    if utilization > 0:
        end_angle = 90 - (min(utilization, 1.0) * 360)
        arc = NSBezierPath.bezierPath()
        arc.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise_(
            (cx, cy), radius, 90, end_angle, True
        )
        usage_color(utilization).setStroke()
        arc.setLineWidth_(line_width)
        arc.setLineCapStyle_(1)  # round cap
        arc.stroke()

    # Percentage text in center
    pct = f"{int(utilization * 100)}"
    font_size = size * 0.3
    attrs = {
        NSFontAttributeName: NSFont.boldSystemFontOfSize_(font_size),
        NSForegroundColorAttributeName: NSColor.whiteColor(),
    }
    s = NSString.stringWithString_(pct)
    s_size = s.sizeWithAttributes_(attrs)
    s.drawAtPoint_withAttributes_(
        (cx - s_size.width / 2, cy - s_size.height / 2), attrs
    )

    img.unlockFocus()
    img.setTemplate_(False)
    return img


def create_error_icon(size: int = 18) -> NSImage:
    """Draw an error/warning icon."""
    img = NSImage.alloc().initWithSize_((size, size))
    img.lockFocus()

    cx, cy = size / 2, size / 2
    radius = (size / 2) - 2

    circle = NSBezierPath.bezierPathWithOvalInRect_(
        ((cx - radius, cy - radius), (radius * 2, radius * 2))
    )
    NSColor.darkGrayColor().setStroke()
    circle.setLineWidth_(1.5)
    circle.stroke()

    # Draw "!" in center
    attrs = {
        NSFontAttributeName: NSFont.boldSystemFontOfSize_(10),
        NSForegroundColorAttributeName: NSColor.orangeColor(),
    }
    from Foundation import NSString

    s = NSString.stringWithString_("!")
    s_size = s.sizeWithAttributes_(attrs)
    s.drawAtPoint_withAttributes_(
        (cx - s_size.width / 2, cy - s_size.height / 2), attrs
    )

    img.unlockFocus()
    img.setTemplate_(False)
    return img


# ---------------------------------------------------------------------------
# Menu Bar App
# ---------------------------------------------------------------------------


def bar_chart(utilization: float, width: int = 20) -> str:
    """Return a text-based progress bar."""
    filled = min(int(utilization * width), width)
    empty = width - filled
    return f"[{'█' * filled}{'░' * empty}]"


class ClaudeMeterDelegate(NSObject):
    def init(self):
        self = objc.super(ClaudeMeterDelegate, self).init()
        if self is None:
            return None

        self.status_item = NSStatusBar.systemStatusBar().statusItemWithLength_(
            NSVariableStatusItemLength
        )
        # Set initial gauge icon (empty)
        self.status_item.setImage_(create_gauge_icon(0.0))
        self.status_item.setTitle_("")
        self.status_item.setHighlightMode_(True)

        self.poll_interval = POLL_INTERVAL_DEFAULT
        self.poll_timer = None
        self._last_windows = []
        self._last_primary = None

        self.menu = NSMenu.alloc().init()
        self.status_item.setMenu_(self.menu)
        self._build_menu([], None)

        # Fetch immediately, then start repeating timer
        NSTimer.scheduledTimerWithTimeInterval_target_selector_userInfo_repeats_(
            0.1, self, "tick:", None, False
        )
        self._start_timer()

        return self

    def _start_timer(self):
        """Start (or restart) the repeating poll timer."""
        if self.poll_timer:
            self.poll_timer.invalidate()
        self.poll_timer = (
            NSTimer.scheduledTimerWithTimeInterval_target_selector_userInfo_repeats_(
                self.poll_interval, self, "tick:", None, True
            )
        )

    def _build_menu(self, windows, primary_window):
        """Rebuild the dropdown menu with all usage details."""
        self.menu.removeAllItems()

        header = NSMenuItem.alloc().initWithTitle_action_keyEquivalent_(
            "Claude-o-Meter", None, ""
        )
        header.setEnabled_(False)
        self.menu.addItem_(header)
        self.menu.addItem_(NSMenuItem.separatorItem())

        if not windows:
            loading = NSMenuItem.alloc().initWithTitle_action_keyEquivalent_(
                "Loading...", None, ""
            )
            loading.setEnabled_(False)
            self.menu.addItem_(loading)
        else:
            for w in windows:
                pct = int(w["utilization"] * 100)
                sym = usage_color_text(w["utilization"])
                bar = bar_chart(w["utilization"])
                reset = format_reset_time(w.get("resets_at"))

                line = NSMenuItem.alloc().initWithTitle_action_keyEquivalent_(
                    f"{sym} {w['label']}: {pct}%  {bar}", None, ""
                )
                line.setEnabled_(False)
                self.menu.addItem_(line)

                reset_line = NSMenuItem.alloc().initWithTitle_action_keyEquivalent_(
                    f"    Resets in: {reset}", None, ""
                )
                reset_line.setEnabled_(False)
                self.menu.addItem_(reset_line)

                self.menu.addItem_(NSMenuItem.separatorItem())

        refresh_item = NSMenuItem.alloc().initWithTitle_action_keyEquivalent_(
            "Refresh Now", "refresh:", ""
        )
        refresh_item.setTarget_(self)
        self.menu.addItem_(refresh_item)

        # Refresh interval submenu
        interval_item = NSMenuItem.alloc().initWithTitle_action_keyEquivalent_(
            "Refresh Interval", None, ""
        )
        interval_submenu = NSMenu.alloc().init()
        for seconds in POLL_INTERVAL_OPTIONS:
            if seconds < 60:
                label = f"{seconds}s"
            else:
                label = f"{seconds // 60}m"
            opt = NSMenuItem.alloc().initWithTitle_action_keyEquivalent_(
                label, "setInterval:", ""
            )
            opt.setTarget_(self)
            opt.setTag_(seconds)
            if seconds == self.poll_interval:
                opt.setState_(1)  # checkmark
            interval_submenu.addItem_(opt)
        interval_item.setSubmenu_(interval_submenu)
        self.menu.addItem_(interval_item)

        self.menu.addItem_(NSMenuItem.separatorItem())

        quit_item = NSMenuItem.alloc().initWithTitle_action_keyEquivalent_(
            "Quit", "quit:", ""
        )
        quit_item.setTarget_(self)
        self.menu.addItem_(quit_item)

    @objc.typedSelector(b"v@:@")
    def tick_(self, timer):
        self._fetch_and_update()

    @objc.IBAction
    def refresh_(self, sender):
        self._fetch_and_update()

    @objc.IBAction
    def setInterval_(self, sender):
        self.poll_interval = sender.tag()
        self._start_timer()
        log.info("Poll interval changed to %ds", self.poll_interval)

    @objc.IBAction
    def quit_(self, sender):
        NSApplication.sharedApplication().terminate_(self)

    def _fetch_and_update(self):
        log.info("Fetching usage data...")
        creds = read_keychain_credentials()
        if not creds:
            log.error("No credentials found")
            self._show_with_last_data()
            return

        token = get_access_token(creds)
        if not token:
            log.error("No access token")
            self._show_with_last_data()
            return

        data = fetch_usage(token)
        if not data:
            log.error("API fetch failed")
            self._show_with_last_data()
            return

        windows = parse_usage(data)
        log.info("Got %d usage windows", len(windows))

        # Primary window is 5h (the most immediately relevant limit)
        primary = None
        for w in windows:
            if w["label"] == "5h":
                primary = w
                break
        if not primary and windows:
            primary = windows[0]

        # Cache successful results
        self._last_windows = windows
        self._last_primary = primary

        self._update_display(windows, primary)

    def _show_with_last_data(self):
        """Re-show the last successful data, or error icon if none."""
        if self._last_windows:
            self._update_display(self._last_windows, self._last_primary)
        else:
            self.status_item.setImage_(create_error_icon())
            self.status_item.setTitle_("")
            self._build_menu([], None)

    def _update_display(self, windows, primary):
        """Update the gauge icon and dropdown menu."""
        if primary:
            self.status_item.setImage_(create_gauge_icon(primary["utilization"]))
        else:
            self.status_item.setImage_(create_gauge_icon(0.0))
        self.status_item.setTitle_("")
        self._build_menu(windows, primary)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    app = NSApplication.sharedApplication()
    app.setActivationPolicy_(NSApplicationActivationPolicyAccessory)
    delegate = ClaudeMeterDelegate.alloc().init()
    app.setDelegate_(delegate)
    log.info("Claude Meter starting...")
    AppHelper.runEventLoop()
