use objc2::rc::Retained;
use objc2::{AnyThread, Message};
use objc2_app_kit::{NSBezierPath, NSColor, NSFont, NSImage, NSStringDrawing,
                    NSFontAttributeName, NSForegroundColorAttributeName};
use objc2_foundation::{NSString, NSSize, NSPoint, NSDictionary};

const GAUGE_START_DEG: f64 = 225.0;
const GAUGE_SWEEP: f64 = 270.0;

const THRESHOLD_WARNING: f64 = 0.60;
const THRESHOLD_DANGER: f64 = 0.80;
const THRESHOLD_CRITICAL: f64 = 0.95;

pub fn usage_color(utilization: f64) -> Retained<NSColor> {
    if utilization >= THRESHOLD_CRITICAL {
        NSColor::redColor()
    } else if utilization >= THRESHOLD_DANGER {
        NSColor::orangeColor()
    } else if utilization >= THRESHOLD_WARNING {
        NSColor::yellowColor()
    } else {
        NSColor::greenColor()
    }
}

fn draw_pie_wedge(cx: f64, cy: f64, radius: f64, inner_r: f64, start_deg: f64, sweep: f64, color: &NSColor) {
    {
        let path = NSBezierPath::bezierPath();
        let center = NSPoint::new(cx, cy);
        let end_deg = start_deg - sweep;

        path.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise(
            center, radius, start_deg, end_deg, true,
        );
        path.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise(
            center, inner_r, end_deg, start_deg, false,
        );
        path.closePath();
        color.setFill();
        path.fill();
    }
}

fn draw_text_centered(text: &str, font: &NSFont, color: &NSColor, cx: f64, cy: f64, size: f64) {
    unsafe {
        let keys = [
            NSFontAttributeName,
            NSForegroundColorAttributeName,
        ];
        let vals: [Retained<objc2::runtime::AnyObject>; 2] = [
            Retained::into_super(font.retain()).into(),
            Retained::into_super(color.retain()).into(),
        ];
        let attrs = NSDictionary::from_retained_objects(&keys, &vals);

        let ns_str = NSString::from_str(text);
        let s_size = ns_str.sizeWithAttributes(Some(&attrs));
        let pt = NSPoint::new(
            cx - s_size.width / 2.0,
            cy - s_size.height / 2.0 + size * 0.04,
        );
        ns_str.drawAtPoint_withAttributes(pt, Some(&attrs));
    }
}

fn create_image_with_drawing(size: f64, draw: impl FnOnce(f64)) -> Retained<NSImage> {
    let ns_size = NSSize::new(size, size);
    #[allow(deprecated)]
    {
        let img = NSImage::initWithSize(NSImage::alloc(), ns_size);
        img.lockFocus();
        draw(size);
        img.unlockFocus();
        img.setTemplate(false);
        img
    }
}

pub fn create_gauge_icon(utilization: f64, size: f64) -> Retained<NSImage> {
    create_image_with_drawing(size, |size| {
        let cx = size / 2.0;
        let cy = size / 2.0;
        let radius = size * 0.46;
        let inner_r = radius * 0.65;

        let bg = NSColor::colorWithCalibratedRed_green_blue_alpha(0.25, 0.25, 0.25, 1.0);
        draw_pie_wedge(cx, cy, radius, inner_r, GAUGE_START_DEG, GAUGE_SWEEP, &bg);

        if utilization > 0.0 {
            let fill_sweep = utilization.min(1.0) * GAUGE_SWEEP;
            let color = usage_color(utilization);
            draw_pie_wedge(cx, cy, radius, inner_r, GAUGE_START_DEG, fill_sweep, &color);
        }

        let pct = format!("{}", (utilization * 100.0) as i32);
        let font = NSFont::boldSystemFontOfSize(size * 0.40);
        let white = NSColor::whiteColor();
        draw_text_centered(&pct, &font, &white, cx, cy, size);
    })
}

pub fn create_paused_icon(size: f64) -> Retained<NSImage> {
    create_image_with_drawing(size, |size| {
        let cx = size / 2.0;
        let cy = size / 2.0;
        let radius = size * 0.46;
        let inner_r = radius * 0.65;

        let grey = NSColor::colorWithCalibratedRed_green_blue_alpha(0.35, 0.35, 0.35, 1.0);
        draw_pie_wedge(cx, cy, radius, inner_r, GAUGE_START_DEG, GAUGE_SWEEP, &grey);

        let font = NSFont::boldSystemFontOfSize(size * 0.35);
        let gray = NSColor::grayColor();
        draw_text_centered("||", &font, &gray, cx, cy, size);
    })
}

pub fn create_error_icon(size: f64) -> Retained<NSImage> {
    create_image_with_drawing(size, |size| {
        let cx = size / 2.0;
        let cy = size / 2.0;
        let radius = size * 0.46;
        let inner_r = radius * 0.65;

        let dark = NSColor::colorWithCalibratedRed_green_blue_alpha(0.25, 0.25, 0.25, 1.0);
        draw_pie_wedge(cx, cy, radius, inner_r, GAUGE_START_DEG, GAUGE_SWEEP, &dark);

        let font = NSFont::boldSystemFontOfSize(size * 0.45);
        let orange = NSColor::orangeColor();
        draw_text_centered("!", &font, &orange, cx, cy, size);
    })
}


/// Color for a position in the bar (0.0 = start, 1.0 = end).
/// Green for 0–60%, orange for 60–80%, red for 80–100%.
pub fn position_color(position: f64) -> Retained<NSColor> {
    if position >= THRESHOLD_DANGER {
        NSColor::systemRedColor()
    } else if position >= THRESHOLD_WARNING {
        NSColor::systemOrangeColor()
    } else {
        NSColor::systemGreenColor()
    }
}

/// Muted version of position color for unfilled segments.
pub fn position_color_muted(position: f64) -> Retained<NSColor> {
    if position >= THRESHOLD_DANGER {
        NSColor::colorWithCalibratedRed_green_blue_alpha(0.8, 0.3, 0.3, 0.25)
    } else if position >= THRESHOLD_WARNING {
        NSColor::colorWithCalibratedRed_green_blue_alpha(0.8, 0.6, 0.2, 0.25)
    } else {
        NSColor::colorWithCalibratedRed_green_blue_alpha(0.3, 0.7, 0.3, 0.25)
    }
}
