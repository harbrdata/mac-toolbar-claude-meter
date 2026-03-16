#!/usr/bin/env python3
"""Generate AppIcon.icns — square app icon using the vintage gauge dial."""

import math
import os
import subprocess
import tempfile

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


def draw_gauge(cx, cy, size):
    """Draw the vintage gauge dial (same as generate_logo.py)."""
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


def render_icon_png(size):
    """Render the gauge as a square PNG at the given pixel size."""
    padding = size * 0.08
    gauge_size = size - padding * 2

    img = NSImage.alloc().initWithSize_((size, size))
    img.lockFocus()

    # Transparent background (the gauge has its own chrome bezel)
    NSColor.clearColor().setFill()
    NSBezierPath.fillRect_(((0, 0), (size, size)))

    draw_gauge(size / 2, size / 2, gauge_size)

    img.unlockFocus()
    return img


def save_png(image, path):
    """Save an NSImage as PNG."""
    tiff = image.TIFFRepresentation()
    rep = NSBitmapImageRep.imageRepWithData_(tiff)
    png_data = rep.representationUsingType_properties_(NSPNGFileType, None)
    png_data.writeToFile_atomically_(path, True)


def main():
    script_dir = os.path.dirname(os.path.abspath(__file__))
    icns_path = os.path.join(script_dir, "AppIcon.icns")

    # macOS .icns requires specific sizes
    icon_sizes = [16, 32, 64, 128, 256, 512, 1024]

    with tempfile.TemporaryDirectory() as tmpdir:
        iconset_dir = os.path.join(tmpdir, "AppIcon.iconset")
        os.makedirs(iconset_dir)

        for size in icon_sizes:
            img = render_icon_png(size)

            # Standard resolution
            if size <= 512:
                save_png(img, os.path.join(iconset_dir, f"icon_{size}x{size}.png"))

            # @2x variant (retina) — the 1024 image is icon_512x512@2x
            if size >= 32:
                half = size // 2
                save_png(img, os.path.join(iconset_dir, f"icon_{half}x{half}@2x.png"))

        # Convert iconset to icns
        subprocess.run(
            ["iconutil", "-c", "icns", iconset_dir, "-o", icns_path],
            check=True,
        )

    print(f"Wrote {icns_path}")


if __name__ == "__main__":
    main()
