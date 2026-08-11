use block2::RcBlock;
use objc2::Message;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBezierPath, NSColor, NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSImage,
    NSStringDrawing,
};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

const GAUGE_START_DEG: f64 = 225.0;
const GAUGE_SWEEP: f64 = 270.0;

const THRESHOLD_WARNING: f64 = 0.60;
const THRESHOLD_DANGER: f64 = 0.80;
const THRESHOLD_CRITICAL: f64 = 0.95;

/// Alpha applied to the muted (secondary/underlay) color ramp.
const MUTED_ALPHA: f64 = 0.45;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Ok,
    Warning,
    Danger,
    Critical,
}

pub fn severity(u: f64) -> Severity {
    if u >= THRESHOLD_CRITICAL {
        Severity::Critical
    } else if u >= THRESHOLD_DANGER {
        Severity::Danger
    } else if u >= THRESHOLD_WARNING {
        Severity::Warning
    } else {
        Severity::Ok
    }
}

pub fn usage_color(u: f64) -> Retained<NSColor> {
    match severity(u) {
        Severity::Critical => NSColor::redColor(),
        Severity::Danger => NSColor::orangeColor(),
        Severity::Warning => NSColor::yellowColor(),
        Severity::Ok => NSColor::greenColor(),
    }
}

pub fn usage_color_muted(u: f64) -> Retained<NSColor> {
    usage_color(u).colorWithAlphaComponent(MUTED_ALPHA)
}

fn draw_pie_wedge(
    cx: f64,
    cy: f64,
    radius: f64,
    inner_r: f64,
    start_deg: f64,
    sweep: f64,
    color: &NSColor,
) {
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
        let keys = [NSFontAttributeName, NSForegroundColorAttributeName];
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

fn create_image_with_drawing(
    width: f64,
    height: f64,
    draw: impl Fn(f64, f64) + 'static,
) -> Retained<NSImage> {
    let ns_size = NSSize::new(width, height);
    let block = RcBlock::new(move |_rect: NSRect| -> objc2::runtime::Bool {
        draw(width, height);
        objc2::runtime::Bool::YES
    });
    let img = NSImage::imageWithSize_flipped_drawingHandler(ns_size, true, &block);
    img.setTemplate(false);
    img
}

/// Draws a single donut gauge (track + optional muted underlay + bright fill + centered
/// percentage text) at the given center, sized relative to `size`.
fn draw_gauge_at(cx: f64, cy: f64, size: f64, primary: f64, secondary: Option<f64>) {
    let radius = size * 0.46;
    let inner_r = radius * 0.65;

    let bg = NSColor::colorWithCalibratedRed_green_blue_alpha(0.25, 0.25, 0.25, 1.0);
    draw_pie_wedge(cx, cy, radius, inner_r, GAUGE_START_DEG, GAUGE_SWEEP, &bg);

    if let Some(s) = secondary
        && s > 0.0
    {
        let muted_sweep = s.min(1.0) * GAUGE_SWEEP;
        let muted_color = usage_color_muted(s);
        draw_pie_wedge(
            cx,
            cy,
            radius,
            inner_r,
            GAUGE_START_DEG,
            muted_sweep,
            &muted_color,
        );
    }

    if primary > 0.0 {
        let fill_sweep = primary.min(1.0) * GAUGE_SWEEP;
        let color = usage_color(primary);
        draw_pie_wedge(cx, cy, radius, inner_r, GAUGE_START_DEG, fill_sweep, &color);
    }

    let pct = format!("{}", (primary * 100.0) as i32);
    let font = NSFont::boldSystemFontOfSize(size * 0.40);
    let white = NSColor::whiteColor();
    draw_text_centered(&pct, &font, &white, cx, cy, size);
}

pub fn create_gauge_icon(primary: f64, secondary: Option<f64>, size: f64) -> Retained<NSImage> {
    create_image_with_drawing(size, size, move |w, h| {
        let cx = w / 2.0;
        let cy = h / 2.0;
        draw_gauge_at(cx, cy, size, primary, secondary);
    })
}

pub fn create_dual_gauge_icon(primary: f64, secondary: f64, size: f64) -> Retained<NSImage> {
    let width = 2.0 * size + 0.15 * size;
    let height = size;
    create_image_with_drawing(width, height, move |_w, h| {
        let cy = h / 2.0;
        let left_cx = size / 2.0;
        let right_cx = size + 0.15 * size + size / 2.0;
        draw_gauge_at(left_cx, cy, size, primary, None);
        draw_gauge_at(right_cx, cy, size, secondary, None);
    })
}

pub fn create_paused_icon(size: f64, width_mult: f64) -> Retained<NSImage> {
    let width = size * width_mult;
    create_image_with_drawing(width, size, move |w, h| {
        let cx = w / 2.0;
        let cy = h / 2.0;
        let radius = size * 0.46;
        let inner_r = radius * 0.65;

        let grey = NSColor::colorWithCalibratedRed_green_blue_alpha(0.35, 0.35, 0.35, 1.0);
        draw_pie_wedge(cx, cy, radius, inner_r, GAUGE_START_DEG, GAUGE_SWEEP, &grey);

        let font = NSFont::boldSystemFontOfSize(size * 0.35);
        let gray = NSColor::grayColor();
        draw_text_centered("||", &font, &gray, cx, cy, size);
    })
}

pub fn create_error_icon(size: f64, width_mult: f64) -> Retained<NSImage> {
    let width = size * width_mult;
    create_image_with_drawing(width, size, move |w, h| {
        let cx = w / 2.0;
        let cy = h / 2.0;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_severity_ok_at_zero() {
        assert_eq!(severity(0.0), Severity::Ok);
    }

    #[test]
    fn test_severity_ok_below_warning() {
        assert_eq!(severity(0.5999999), Severity::Ok);
    }

    #[test]
    fn test_severity_warning_at_boundary() {
        assert_eq!(severity(0.60), Severity::Warning);
    }

    #[test]
    fn test_severity_warning_just_below_danger() {
        assert_eq!(severity(0.7999999), Severity::Warning);
    }

    #[test]
    fn test_severity_danger_at_boundary() {
        assert_eq!(severity(0.80), Severity::Danger);
    }

    #[test]
    fn test_severity_danger_just_below_critical() {
        assert_eq!(severity(0.9499999), Severity::Danger);
    }

    #[test]
    fn test_severity_critical_at_boundary() {
        assert_eq!(severity(0.95), Severity::Critical);
    }

    #[test]
    fn test_severity_critical_at_one() {
        assert_eq!(severity(1.0), Severity::Critical);
    }

    #[test]
    fn test_severity_critical_above_one() {
        assert_eq!(severity(1.5), Severity::Critical);
    }

    #[test]
    fn test_severity_table() {
        let cases = [
            (0.0, Severity::Ok),
            (0.59, Severity::Ok),
            (0.60, Severity::Warning),
            (0.61, Severity::Warning),
            (0.79, Severity::Warning),
            (0.80, Severity::Danger),
            (0.81, Severity::Danger),
            (0.94, Severity::Danger),
            (0.95, Severity::Critical),
            (0.96, Severity::Critical),
            (1.0, Severity::Critical),
            (2.0, Severity::Critical),
        ];
        for (input, expected) in cases {
            assert_eq!(severity(input), expected, "severity({input}) mismatch");
        }
    }
}
