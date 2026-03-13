#!/usr/bin/env python3
# Copyright (c) 2025-2026 Harbr Data. All rights reserved.
"""Claude-o-Meter — macOS menu bar app showing Claude Code plan usage."""

import collections
import json
import logging
import os
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
    NSImageScaleProportionallyUpOrDown,
    NSImageView,
    NSMenu,
    NSMenuItem,
    NSObject,
    NSStatusBar,
    NSTimer,
    NSVariableStatusItemLength,
    NSView,
)
from Foundation import NSAttributedString, NSString
from PyObjCTools import AppHelper

class RingBufferHandler(logging.Handler):
    """Keeps the last N log records in a deque for display in the menu."""

    def __init__(self, capacity: int = 20):
        super().__init__()
        self.records: collections.deque[str] = collections.deque(maxlen=capacity)

    def emit(self, record):
        self.records.append(self.format(record))


log_buffer = RingBufferHandler(capacity=20)
log_buffer.setFormatter(logging.Formatter("%(asctime)s [%(levelname)s] %(message)s"))

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
    handlers=[logging.StreamHandler(), log_buffer],
)
log = logging.getLogger("claude_meter")

# Sentinel returned by fetch_usage on a 429 to distinguish from other errors
RATE_LIMITED = "RATE_LIMITED"
RATE_LIMIT_PAUSE_DEFAULT = 60  # seconds — fallback when no Retry-After header
RATE_LIMIT_PAUSE_MAX = 600  # seconds (10 minutes) — cap for exponential backoff

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

_VERSION_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "VERSION")
VERSION = open(_VERSION_FILE).read().strip() if os.path.exists(_VERSION_FILE) else "0.0.0"

KEYCHAIN_SERVICE = "Claude Code-credentials"
USAGE_URL = "https://api.anthropic.com/api/oauth/usage"
USER_AGENT = "claude-code/2.1.70"
ANTHROPIC_BETA = "oauth-2025-04-20"

POLL_INTERVAL_DEFAULT = 120  # seconds
POLL_INTERVAL_OPTIONS = [60, 120, 300, 600]  # seconds

THRESHOLD_WARNING = 0.60
THRESHOLD_DANGER = 0.80
THRESHOLD_CRITICAL = 0.95

LOGO_ICON_HEIGHT = 80

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


def fetch_usage(access_token: str) -> dict | tuple[str, int] | None:
    """Fetch usage data from the Anthropic API.

    Returns the parsed JSON dict on success, a (RATE_LIMITED, retry_seconds)
    tuple on 429, or None on other errors.
    """
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
            retry_after_raw = resp.headers.get("Retry-After")
            try:
                retry_after = int(retry_after_raw) if retry_after_raw else 0
            except (ValueError, TypeError):
                retry_after = 0
            log.warning(
                "API rate-limited (429). Retry-After: %s",
                retry_after_raw or "not set",
            )
            return (RATE_LIMITED, retry_after)
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


# ---------------------------------------------------------------------------
# Gauge icon drawing
# ---------------------------------------------------------------------------


# Gauge arc constants (match logo dial: 7 o'clock to 5 o'clock)
_GAUGE_START_DEG = 225
_GAUGE_SWEEP = 270


def _draw_pie_wedge(cx, cy, radius, inner_r, start_deg, sweep, color):
    """Draw a filled pie/annular wedge."""
    path = NSBezierPath.bezierPath()
    end_deg = start_deg - sweep
    path.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise_(
        (cx, cy), radius, start_deg, end_deg, True
    )
    path.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise_(
        (cx, cy), inner_r, end_deg, start_deg, False
    )
    path.closePath()
    color.setFill()
    path.fill()


def create_gauge_icon(utilization: float, size: int = 24) -> NSImage:
    """Draw a filled pie gauge (7 to 5 o'clock) with percentage in the center."""
    img = NSImage.alloc().initWithSize_((size, size))
    img.lockFocus()

    cx, cy = size / 2, size / 2
    radius = size * 0.46
    inner_r = radius * 0.65

    # Background wedge
    bg_clr = NSColor.colorWithCalibratedRed_green_blue_alpha_(0.25, 0.25, 0.25, 1.0)
    _draw_pie_wedge(cx, cy, radius, inner_r, _GAUGE_START_DEG, _GAUGE_SWEEP, bg_clr)

    # Filled wedge
    if utilization > 0:
        fill_sweep = min(utilization, 1.0) * _GAUGE_SWEEP
        _draw_pie_wedge(
            cx, cy, radius, inner_r, _GAUGE_START_DEG, fill_sweep,
            usage_color(utilization),
        )

    # Percentage text centered
    pct = f"{int(utilization * 100)}"
    font_size = size * 0.40
    attrs = {
        NSFontAttributeName: NSFont.boldSystemFontOfSize_(font_size),
        NSForegroundColorAttributeName: NSColor.whiteColor(),
    }
    s = NSString.stringWithString_(pct)
    s_size = s.sizeWithAttributes_(attrs)
    s.drawAtPoint_withAttributes_(
        (cx - s_size.width / 2, cy - s_size.height / 2 + size * 0.04), attrs
    )

    img.unlockFocus()
    img.setTemplate_(False)
    return img


def create_paused_icon(size: int = 24) -> NSImage:
    """Draw a greyed-out pie gauge indicating polling is paused."""
    img = NSImage.alloc().initWithSize_((size, size))
    img.lockFocus()

    cx, cy = size / 2, size / 2
    radius = size * 0.46
    inner_r = radius * 0.65

    # Grey wedge
    grey = NSColor.colorWithCalibratedRed_green_blue_alpha_(0.35, 0.35, 0.35, 1.0)
    _draw_pie_wedge(cx, cy, radius, inner_r, _GAUGE_START_DEG, _GAUGE_SWEEP, grey)

    # "||" text in center
    attrs = {
        NSFontAttributeName: NSFont.boldSystemFontOfSize_(size * 0.35),
        NSForegroundColorAttributeName: NSColor.grayColor(),
    }
    s = NSString.stringWithString_("||")
    s_size = s.sizeWithAttributes_(attrs)
    s.drawAtPoint_withAttributes_(
        (cx - s_size.width / 2, cy - s_size.height / 2 + size * 0.04), attrs
    )

    img.unlockFocus()
    img.setTemplate_(False)
    return img


def create_error_icon(size: int = 24) -> NSImage:
    """Draw an error/warning pie gauge icon."""
    img = NSImage.alloc().initWithSize_((size, size))
    img.lockFocus()

    cx, cy = size / 2, size / 2
    radius = size * 0.46
    inner_r = radius * 0.65

    # Dark wedge
    dark = NSColor.colorWithCalibratedRed_green_blue_alpha_(0.25, 0.25, 0.25, 1.0)
    _draw_pie_wedge(cx, cy, radius, inner_r, _GAUGE_START_DEG, _GAUGE_SWEEP, dark)

    # "!" in center
    attrs = {
        NSFontAttributeName: NSFont.boldSystemFontOfSize_(size * 0.45),
        NSForegroundColorAttributeName: NSColor.orangeColor(),
    }
    s = NSString.stringWithString_("!")
    s_size = s.sizeWithAttributes_(attrs)
    s.drawAtPoint_withAttributes_(
        (cx - s_size.width / 2, cy - s_size.height / 2 + size * 0.04), attrs
    )

    img.unlockFocus()
    img.setTemplate_(False)
    return img


_LOGO_DIR = os.path.dirname(os.path.abspath(__file__))


def create_logo_icon(height: int = 18) -> NSImage:
    """Load the pre-rendered logo PNG and scale proportionally to *height*."""
    path = os.path.join(_LOGO_DIR, "logo.png")
    img = NSImage.alloc().initByReferencingFile_(path)
    orig = img.size()
    aspect = orig.width / orig.height if orig.height else 1.0
    img.setSize_((height * aspect, height))
    img.setTemplate_(False)
    return img


# ---------------------------------------------------------------------------
# Styled menu helpers
# ---------------------------------------------------------------------------


def _styled_item(text, font=None, color=None,
                 enabled=True) -> NSMenuItem:
    """Create a menu item with full-color attributed text.

    Items with no action and no target are non-interactive but
    stay enabled so macOS does not dim the attributed title.
    """
    item = NSMenuItem.alloc().initWithTitle_action_keyEquivalent_(
        "", None, "")
    attrs = {
        NSFontAttributeName: font or NSFont.menuFontOfSize_(13),
        NSForegroundColorAttributeName: color or NSColor.labelColor(),
    }
    item.setAttributedTitle_(
        NSAttributedString.alloc().initWithString_attributes_(
            text, attrs)
    )
    item.setEnabled_(enabled)
    return item


# ---------------------------------------------------------------------------
# Menu Bar App
# ---------------------------------------------------------------------------


def bar_chart(utilization: float, width: int = 20) -> str:
    """Return a text-based progress bar."""
    filled = min(int(utilization * width), width)
    empty = width - filled
    return "▰" * filled + "▱" * empty


class ClaudeMeterDelegate(NSObject):
    def init(self):
        self = objc.super(ClaudeMeterDelegate, self).init()
        if self is None:
            return None

        self.status_item = NSStatusBar.systemStatusBar().statusItemWithLength_(
            NSVariableStatusItemLength
        )
        # Use the modern button() API (required on macOS 26+, recommended since 10.12)
        button = self.status_item.button()
        button.setImage_(create_gauge_icon(0.0))
        button.setTitle_("")

        self.poll_interval = POLL_INTERVAL_DEFAULT
        self.poll_timer = None
        self.polling_enabled = True
        self._last_windows = []
        self._last_primary = None
        self._rate_limited = False
        self._rate_limit_resume_time = None
        self._rate_limit_timer = None
        self._rate_limit_countdown_timer = None
        self._rate_limit_backoff = 0  # tracks consecutive rate-limits for backoff
        self._cached_token = None

        self.menu = NSMenu.alloc().init()
        self.menu.setAutoenablesItems_(False)
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

        # banner header — use custom view with padding to align with menu text
        MENU_PAD = 14  # matches macOS menu item left/right text inset
        header = NSMenuItem.alloc().initWithTitle_action_keyEquivalent_(
            "", None, ""
        )
        logo_img = create_logo_icon(LOGO_ICON_HEIGHT)
        logo_size = logo_img.size()
        container_w = logo_size.width + MENU_PAD * 2
        container_h = logo_size.height
        container = NSView.alloc().initWithFrame_(
            ((0, 0), (container_w, container_h))
        )
        img_view = NSImageView.alloc().initWithFrame_(
            ((MENU_PAD, 0), (logo_size.width, logo_size.height))
        )
        img_view.setImage_(logo_img)
        img_view.setImageScaling_(NSImageScaleProportionallyUpOrDown)
        container.addSubview_(img_view)
        header.setView_(container)
        self.menu.addItem_(header)
        self.menu.addItem_(NSMenuItem.separatorItem())

        # Rate-limit banner
        if self._rate_limited and self._rate_limit_resume_time:
            remaining = int(self._rate_limit_resume_time - time.time())
            if remaining < 0:
                remaining = 0
            mins, secs = divmod(remaining, 60)
            banner = _styled_item(
                f"\u26a0\ufe0f  Rate limited \u2014 polling paused ({mins}m {secs}s)",
                color=NSColor.systemOrangeColor(),
            )
            self.menu.addItem_(banner)
            self.menu.addItem_(NSMenuItem.separatorItem())

        mono = (
            NSFont.fontWithName_size_("Menlo", 12)
            or NSFont.systemFontOfSize_(12)
        )

        if not windows:
            self.menu.addItem_(
                _styled_item("Loading...", color=NSColor.secondaryLabelColor())
            )
        else:
            for w in windows:
                pct = int(w["utilization"] * 100)
                bar = bar_chart(w["utilization"])
                reset = format_reset_time(w.get("resets_at"))

                line = _styled_item(
                    f" {w['label']}: {pct}%  {bar}", font=mono,
                    color=NSColor.controlTextColor(),
                )
                line.setImage_(create_gauge_icon(w["utilization"], size=16))
                self.menu.addItem_(line)
                self.menu.addItem_(
                    _styled_item(
                        f"       Resets in: {reset}",
                        font=NSFont.fontWithName_size_("Menlo", 11)
                        or NSFont.systemFontOfSize_(11),
                        color=NSColor.labelColor(),
                    )
                )
                self.menu.addItem_(NSMenuItem.separatorItem())

        refresh_item = NSMenuItem.alloc().initWithTitle_action_keyEquivalent_(
            "Refresh Now", "refresh:", ""
        )
        refresh_item.setTarget_(self)
        self.menu.addItem_(refresh_item)

        # Polling toggle
        polling_label = (
            "Polling: On" if self.polling_enabled else "Polling: Off"
        )
        polling_item = NSMenuItem.alloc().initWithTitle_action_keyEquivalent_(
            polling_label, "togglePolling:", ""
        )
        polling_item.setTarget_(self)
        self.menu.addItem_(polling_item)

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

        # Recent logs submenu
        logs_item = NSMenuItem.alloc().initWithTitle_action_keyEquivalent_(
            "Recent Logs", None, ""
        )
        logs_submenu = NSMenu.alloc().init()
        logs_submenu.setAutoenablesItems_(False)
        log_lines = list(log_buffer.records)
        if not log_lines:
            logs_submenu.addItem_(
                _styled_item("(no logs yet)", color=NSColor.secondaryLabelColor())
            )
        else:
            log_font = (
                NSFont.fontWithName_size_("Menlo", 12)
                or NSFont.systemFontOfSize_(12)
            )
            for line in log_lines[-10:]:
                display = line if len(line) <= 100 else line[:97] + "..."
                logs_submenu.addItem_(
                    _styled_item(display, font=log_font,
                                 color=NSColor.labelColor())
                )
        logs_item.setSubmenu_(logs_submenu)
        self.menu.addItem_(logs_item)

        self.menu.addItem_(NSMenuItem.separatorItem())

        about_item = NSMenuItem.alloc().initWithTitle_action_keyEquivalent_(
            "About Claude-o-Meter", "showAbout:", ""
        )
        about_item.setTarget_(self)
        self.menu.addItem_(about_item)

        quit_item = NSMenuItem.alloc().initWithTitle_action_keyEquivalent_(
            "Quit", "quit:", ""
        )
        quit_item.setTarget_(self)
        self.menu.addItem_(quit_item)

    @objc.typedSelector(b"v@:@")
    def tick_(self, timer):
        if not self.polling_enabled or self._rate_limited:
            return
        self._fetch_and_update()

    @objc.IBAction
    def refresh_(self, sender):
        # Manual refresh clears rate-limit pause
        self._clear_rate_limit()
        self._fetch_and_update()

    @objc.IBAction
    def togglePolling_(self, sender):
        self.polling_enabled = not self.polling_enabled
        if self.polling_enabled:
            log.info("Polling enabled")
            self._start_timer()
        else:
            log.info("Polling disabled")
            if self.poll_timer:
                self.poll_timer.invalidate()
                self.poll_timer = None
        # Show paused icon when disabled
        if not self.polling_enabled:
            self.status_item.button().setImage_(create_paused_icon())
            self.status_item.button().setTitle_("")
        elif self._last_primary:
            self.status_item.button().setImage_(
                create_gauge_icon(self._last_primary["utilization"])
            )
        self._build_menu(self._last_windows, self._last_primary)

    @objc.IBAction
    def setInterval_(self, sender):
        self.poll_interval = sender.tag()
        self._start_timer()
        log.info("Poll interval changed to %ds", self.poll_interval)

    @objc.IBAction
    def quit_(self, sender):
        NSApplication.sharedApplication().terminate_(self)

    @objc.IBAction
    def showAbout_(self, sender):
        app = NSApplication.sharedApplication()
        app.activateIgnoringOtherApps_(True)
        credits_text = (
            "A macOS menu bar app that shows your Claude Code "
            "plan usage at a glance.\n\n"
            "Displays a color-coded ring gauge showing your current "
            "usage across all rate-limit windows with progress bars "
            "and reset countdowns.\n\n"
            "Reads credentials from your existing `claude login` session. "
            "No credentials are stored, transmitted, or logged by this app."
        )
        app.orderFrontStandardAboutPanelWithOptions_({
            "ApplicationName": "Claude-o-Meter",
            "ApplicationVersion": VERSION,
            "Version": "",
            "Copyright": "\u2622 \u00a9 2025-2026 Harbr Data. All rights reserved.",
            "Credits": NSAttributedString.alloc().initWithString_attributes_(
                credits_text,
                {
                    NSFontAttributeName: NSFont.systemFontOfSize_(11),
                    NSForegroundColorAttributeName: NSColor.labelColor(),
                },
            ),
            "ApplicationIcon": create_gauge_icon(0.0, size=128),
        })

    def _enter_rate_limit_pause(self, retry_after: int = 0):
        """Pause polling after a 429.

        Uses the server's Retry-After value if provided, otherwise applies
        exponential backoff starting from RATE_LIMIT_PAUSE_DEFAULT and
        capping at RATE_LIMIT_PAUSE_MAX.
        """
        self._rate_limit_backoff += 1

        if retry_after > 0:
            pause = min(retry_after, RATE_LIMIT_PAUSE_MAX)
        else:
            # Exponential backoff: 60, 120, 240, 480, capped at 600
            pause = min(
                RATE_LIMIT_PAUSE_DEFAULT * (2 ** (self._rate_limit_backoff - 1)),
                RATE_LIMIT_PAUSE_MAX,
            )

        self._rate_limited = True
        self._rate_limit_resume_time = time.time() + pause
        log.warning("Pausing polling for %d seconds due to rate limit (attempt %d)",
                     pause, self._rate_limit_backoff)

        # Grey out the icon
        self.status_item.button().setImage_(create_paused_icon())
        self.status_item.button().setTitle_("")
        self._build_menu(self._last_windows, self._last_primary)

        # Schedule automatic resume
        if self._rate_limit_timer:
            self._rate_limit_timer.invalidate()
        self._rate_limit_timer = (
            NSTimer.scheduledTimerWithTimeInterval_target_selector_userInfo_repeats_(
                pause, self, "rateLimitResume:", None, False
            )
        )

        # Update the countdown every 10 seconds
        if self._rate_limit_countdown_timer:
            self._rate_limit_countdown_timer.invalidate()
        self._rate_limit_countdown_timer = (
            NSTimer.scheduledTimerWithTimeInterval_target_selector_userInfo_repeats_(
                10, self, "rateLimitCountdown:", None, True
            )
        )

    def _clear_rate_limit(self):
        """Clear rate-limit state."""
        self._rate_limited = False
        self._rate_limit_resume_time = None
        if self._rate_limit_timer:
            self._rate_limit_timer.invalidate()
            self._rate_limit_timer = None
        if self._rate_limit_countdown_timer:
            self._rate_limit_countdown_timer.invalidate()
            self._rate_limit_countdown_timer = None

    @objc.typedSelector(b"v@:@")
    def rateLimitCountdown_(self, timer):
        """Refresh the menu to update the rate-limit countdown display."""
        if self._rate_limited:
            self._build_menu(self._last_windows, self._last_primary)

    @objc.typedSelector(b"v@:@")
    def rateLimitResume_(self, timer):
        """Called when the rate-limit pause expires."""
        log.info("Rate-limit pause expired, resuming polling")
        self._clear_rate_limit()
        self._fetch_and_update()

    def _fetch_and_update(self):
        log.info("Fetching usage data...")

        # Cache the token to avoid hitting keychain + refresh on every poll
        if not self._cached_token:
            creds = read_keychain_credentials()
            if not creds:
                log.error("No credentials found")
                self._show_with_last_data()
                return
            self._cached_token = get_access_token(creds)

        if not self._cached_token:
            log.error("No access token")
            self._show_with_last_data()
            return

        data = fetch_usage(self._cached_token)

        # Handle rate-limit tuple
        if isinstance(data, tuple) and data[0] is RATE_LIMITED:
            self._enter_rate_limit_pause(retry_after=data[1])
            return

        # 401 likely means token expired — clear cache so next poll refreshes
        if data is None:
            self._cached_token = None
            self._show_with_last_data()
            return

        # Success — reset backoff counter
        self._rate_limit_backoff = 0

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
            self.status_item.button().setImage_(create_error_icon())
            self.status_item.button().setTitle_("")
            self._build_menu([], None)

    def _update_display(self, windows, primary):
        """Update the gauge icon and dropdown menu."""
        if primary:
            self.status_item.button().setImage_(create_gauge_icon(primary["utilization"]))
        else:
            self.status_item.button().setImage_(create_gauge_icon(0.0))
        self.status_item.button().setTitle_("")
        self._build_menu(windows, primary)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    app = NSApplication.sharedApplication()
    # On macOS 26+, briefly set Regular policy so the system registers
    # the app as a menu-bar-item provider, then switch to Accessory
    # to hide the Dock icon.
    app.setActivationPolicy_(NSApplicationActivationPolicyAccessory)
    delegate = ClaudeMeterDelegate.alloc().init()
    app.setDelegate_(delegate)

    # Explicitly mark the status item as visible (macOS 26+ respects this)
    try:
        delegate.status_item.setVisible_(True)
    except AttributeError:
        pass  # older macOS versions without setVisible_

    log.info("Claude Meter starting...")
    AppHelper.runEventLoop()
