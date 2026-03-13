#!/usr/bin/env python3
"""Generate logo.png — retro 1950s banner with gauge + title text."""

import math
import os

from AppKit import (
    NSBezierPath,
    NSBitmapImageRep,
    NSColor,
    NSFont,
    NSFontAttributeName,
    NSForegroundColorAttributeName,
    NSGraphicsContext,
    NSImage,
    NSPNGFileType,
    NSString,
)

_VERSION_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "VERSION")
VERSION = open(_VERSION_FILE).read().strip() if os.path.exists(_VERSION_FILE) else "0.0.0"

# Layout constants
HEIGHT = 160
GAUGE_SIZE = 130
GAUGE_X = 16
GAUGE_Y = (HEIGHT - GAUGE_SIZE) / 2
PADDING = 8  # border thickness


def draw_gauge(cx, cy, size):
    """Draw the vintage gauge dial."""
    outer_r = (size / 2) - 0.5
    lw = max(1.0, size / 50.0)

    cream = NSColor.colorWithCalibratedRed_green_blue_alpha_(0.94, 0.92, 0.87, 1.0)
    chrome = NSColor.colorWithCalibratedRed_green_blue_alpha_(0.55, 0.58, 0.62, 1.0)
    chrome_lt = NSColor.colorWithCalibratedRed_green_blue_alpha_(0.78, 0.80, 0.82, 1.0)
    tick_clr = NSColor.colorWithCalibratedRed_green_blue_alpha_(0.3, 0.3, 0.28, 1.0)
    red = NSColor.colorWithCalibratedRed_green_blue_alpha_(0.75, 0.15, 0.1, 1.0)

    bezel = NSBezierPath.bezierPathWithOvalInRect_(
        ((cx - outer_r, cy - outer_r), (outer_r * 2, outer_r * 2))
    )
    chrome.setFill()
    bezel.fill()

    face_r = outer_r * 0.88
    face = NSBezierPath.bezierPathWithOvalInRect_(
        ((cx - face_r, cy - face_r), (face_r * 2, face_r * 2))
    )
    cream.setFill()
    face.fill()

    start_deg = 225
    sweep = 270
    num_major = 10
    ticks_per = 5
    total_ticks = num_major * ticks_per

    for i in range(total_ticks + 1):
        frac = i / total_ticks
        angle_rad = math.radians(start_deg - frac * sweep)
        is_major = i % ticks_per == 0
        inner = face_r * (0.72 if is_major else 0.82)
        outer = face_r * 0.90
        tick = NSBezierPath.bezierPath()
        tick.moveToPoint_((
            cx + inner * math.cos(angle_rad),
            cy + inner * math.sin(angle_rad),
        ))
        tick.lineToPoint_((
            cx + outer * math.cos(angle_rad),
            cy + outer * math.sin(angle_rad),
        ))
        tick_clr.setStroke()
        tick.setLineWidth_(lw * (1.2 if is_major else 0.6))
        tick.stroke()

    zone_r = face_r * 0.94
    zone_w = lw * 2.5
    zones = [
        (0.0, 0.6, 0.3, 0.7, 0.3, 0.5),
        (0.6, 0.8, 0.85, 0.7, 0.1, 0.5),
        (0.8, 1.0, 0.8, 0.2, 0.15, 0.5),
    ]
    for frac_start, frac_end, r, g, b, a in zones:
        arc = NSBezierPath.bezierPath()
        a1 = start_deg - frac_start * sweep
        a2 = start_deg - frac_end * sweep
        arc.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise_(
            (cx, cy), zone_r, a1, a2, True
        )
        NSColor.colorWithCalibratedRed_green_blue_alpha_(r, g, b, a).setStroke()
        arc.setLineWidth_(zone_w)
        arc.stroke()

    num_attrs = {
        NSFontAttributeName: NSFont.systemFontOfSize_(max(5, size * 0.09)),
        NSForegroundColorAttributeName: tick_clr,
    }
    for i in range(num_major + 1):
        frac = i / num_major
        angle_rad = math.radians(start_deg - frac * sweep)
        nr = face_r * 0.60
        label = NSString.stringWithString_(str(int(frac * 100)))
        ls = label.sizeWithAttributes_(num_attrs)
        label.drawAtPoint_withAttributes_(
            (
                cx + nr * math.cos(angle_rad) - ls.width / 2,
                cy + nr * math.sin(angle_rad) - ls.height / 2,
            ),
            num_attrs,
        )

    n_frac = 0.35
    n_rad = math.radians(start_deg - n_frac * sweep)
    n_len = face_r * 0.78
    needle = NSBezierPath.bezierPath()
    needle.moveToPoint_((cx, cy))
    needle.lineToPoint_((
        cx + n_len * math.cos(n_rad),
        cy + n_len * math.sin(n_rad),
    ))
    red.setStroke()
    needle.setLineWidth_(lw * 1.5)
    needle.stroke()

    t_rad = math.radians(start_deg - n_frac * sweep + 180)
    t_len = face_r * 0.15
    tail = NSBezierPath.bezierPath()
    tail.moveToPoint_((cx, cy))
    tail.lineToPoint_((
        cx + t_len * math.cos(t_rad),
        cy + t_len * math.sin(t_rad),
    ))
    red.setStroke()
    tail.setLineWidth_(lw * 2.0)
    tail.stroke()

    hub_r = size * 0.06
    hub = NSBezierPath.bezierPathWithOvalInRect_(
        ((cx - hub_r, cy - hub_r), (hub_r * 2, hub_r * 2))
    )
    chrome_lt.setFill()
    hub.fill()
    chrome.setStroke()
    hub.setLineWidth_(lw * 0.5)
    hub.stroke()

    c_font = NSFont.fontWithName_size_(
        "Snell Roundhand", size * 0.18
    ) or NSFont.systemFontOfSize_(size * 0.18)
    c_attrs = {
        NSFontAttributeName: c_font,
        NSForegroundColorAttributeName: tick_clr,
    }
    c = NSString.stringWithString_("C")
    cs = c.sizeWithAttributes_(c_attrs)
    c.drawAtPoint_withAttributes_((cx - cs.width / 2, cy - face_r * 0.45), c_attrs)


def _measure_text(text, font):
    """Return the size of text rendered with the given font."""
    s = NSString.stringWithString_(text)
    attrs = {NSFontAttributeName: font}
    return s.sizeWithAttributes_(attrs)


def draw_text_with_shadow(text, x, y, font, color, shadow_color, shadow_offset=3):
    """Draw text with a drop shadow."""
    shadow_attrs = {
        NSFontAttributeName: font,
        NSForegroundColorAttributeName: shadow_color,
    }
    s = NSString.stringWithString_(text)
    s.drawAtPoint_withAttributes_((x + shadow_offset, y - shadow_offset), shadow_attrs)

    text_attrs = {
        NSFontAttributeName: font,
        NSForegroundColorAttributeName: color,
    }
    s.drawAtPoint_withAttributes_((x, y), text_attrs)
    return s.sizeWithAttributes_(text_attrs)


def _draw_star(cx, cy, size, color):
    """Draw a 4-pointed retro star/sparkle."""
    path = NSBezierPath.bezierPath()
    path.moveToPoint_((cx, cy - size))
    path.lineToPoint_((cx - size * 0.2, cy))
    path.lineToPoint_((cx, cy + size))
    path.lineToPoint_((cx + size * 0.2, cy))
    path.closePath()
    path.moveToPoint_((cx - size, cy))
    path.lineToPoint_((cx, cy - size * 0.2))
    path.lineToPoint_((cx + size, cy))
    path.lineToPoint_((cx, cy + size * 0.2))
    path.closePath()
    color.setFill()
    path.fill()


def create_banner():
    """Create the full banner image with gauge + retro title."""
    # --- Measure text to compute tight width ---
    title_font = (
        NSFont.fontWithName_size_("Rockwell Bold", 48)
        or NSFont.fontWithName_size_("Rockwell", 48)
        or NSFont.fontWithName_size_("American Typewriter Bold", 46)
        or NSFont.fontWithName_size_("American Typewriter", 46)
        or NSFont.boldSystemFontOfSize_(44)
    )
    version_font = (
        NSFont.fontWithName_size_("American Typewriter", 18)
        or NSFont.systemFontOfSize_(18)
    )

    text_x = GAUGE_X + GAUGE_SIZE + 12
    title_sz = _measure_text("Claude-o-Meter", title_font)
    version_sz = _measure_text(f"v{VERSION}", version_font)

    # Width = text_x + title + gap + version + small right margin
    content_right = text_x + title_sz.width + 10 + version_sz.width
    WIDTH = int(content_right + PADDING + 4)

    img = NSImage.alloc().initWithSize_((WIDTH, HEIGHT))
    img.lockFocus()

    # Warm cream background with border
    bg = NSColor.colorWithCalibratedRed_green_blue_alpha_(0.96, 0.94, 0.89, 1.0)
    border_clr = NSColor.colorWithCalibratedRed_green_blue_alpha_(
        0.78, 0.72, 0.62, 1.0
    )
    border_clr.setFill()
    NSBezierPath.fillRect_(((0, 0), (WIDTH, HEIGHT)))
    bg.setFill()
    NSBezierPath.fillRect_(((4, 4), (WIDTH - 8, HEIGHT - 8)))

    # Inner decorative line
    inner_border = NSColor.colorWithCalibratedRed_green_blue_alpha_(
        0.75, 0.68, 0.55, 0.5
    )
    inner_border.setStroke()
    inner = NSBezierPath.bezierPathWithRect_(((8, 8), (WIDTH - 16, HEIGHT - 16)))
    inner.setLineWidth_(1.0)
    inner.stroke()

    # Draw gauge
    gauge_cx = GAUGE_X + GAUGE_SIZE / 2
    gauge_cy = GAUGE_Y + GAUGE_SIZE / 2
    draw_gauge(gauge_cx, gauge_cy, GAUGE_SIZE)

    # Retro colors
    title_red = NSColor.colorWithCalibratedRed_green_blue_alpha_(
        0.80, 0.18, 0.12, 1.0
    )
    shadow_dark = NSColor.colorWithCalibratedRed_green_blue_alpha_(
        0.25, 0.12, 0.08, 0.45
    )
    subtitle_brown = NSColor.colorWithCalibratedRed_green_blue_alpha_(
        0.45, 0.35, 0.25, 1.0
    )

    # Title
    title_size = draw_text_with_shadow(
        "Claude-o-Meter",
        text_x, HEIGHT / 2 - 4,
        title_font, title_red, shadow_dark,
        shadow_offset=3,
    )

    # Version
    draw_text_with_shadow(
        f"v{VERSION}",
        text_x + title_size.width + 10, HEIGHT / 2 + 8,
        version_font, subtitle_brown,
        NSColor.colorWithCalibratedRed_green_blue_alpha_(0.2, 0.1, 0.05, 0.3),
        shadow_offset=2,
    )

    # Tagline
    tagline_font = (
        NSFont.fontWithName_size_("American Typewriter", 16)
        or NSFont.fontWithName_size_("Menlo", 14)
        or NSFont.systemFontOfSize_(14)
    )
    tagline_attrs = {
        NSFontAttributeName: tagline_font,
        NSForegroundColorAttributeName: subtitle_brown,
    }
    tagline = NSString.stringWithString_("Your Claude Code Usage at a Glance")
    tagline.drawAtPoint_withAttributes_((text_x + 4, HEIGHT / 2 - 36), tagline_attrs)

    # Decorative stars
    star_clr = NSColor.colorWithCalibratedRed_green_blue_alpha_(
        0.80, 0.55, 0.15, 0.6
    )
    _draw_star(WIDTH - 40, HEIGHT - 35, 8, star_clr)
    _draw_star(WIDTH - 18, HEIGHT - 50, 5, star_clr)
    _draw_star(WIDTH - 32, 30, 6, star_clr)
    _draw_star(text_x + 2, HEIGHT - 28, 5, star_clr)

    img.unlockFocus()
    return img, WIDTH


def main():
    img, width = create_banner()
    tiff = img.TIFFRepresentation()
    rep = NSBitmapImageRep.imageRepWithData_(tiff)
    png_data = rep.representationUsingType_properties_(NSPNGFileType, None)

    out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "logo.png")
    png_data.writeToFile_atomically_(out, True)
    print(f"Wrote {out} ({width}x{HEIGHT})")


if __name__ == "__main__":
    main()
