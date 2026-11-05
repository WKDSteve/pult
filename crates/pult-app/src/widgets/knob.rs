//! The pan knob: a bipolar rotary control from -1 (left) to 1 (right).

use std::f32::consts::PI;

use eframe::egui::{Response, Sense, Shape, Stroke, Ui, Vec2, WidgetInfo, vec2};

use crate::theme::Theme;

/// Vertical drag distance for the whole range, in points.
const DRAG_RANGE: f32 = 160.0;
/// Shift slows a drag down by this factor.
const FINE_DRAG: f32 = 0.1;
/// Pan change per wheel notch.
const WHEEL_STEP: f32 = 0.05;
/// The knob turns through 270 degrees, with the gap at the bottom.
const SWEEP: f32 = 1.5 * PI;

/// The pan after dragging by `dy` points (up is negative, turns right).
#[must_use]
pub fn drag(pan: f32, dy: f32, fine: bool) -> f32 {
    let speed = if fine { FINE_DRAG } else { 1.0 };
    (pan - dy / DRAG_RANGE * 2.0 * speed).clamp(-1.0, 1.0)
}

/// The pan as the strip prints it: `C`, `L40`, `R25`.
#[must_use]
pub fn format_pan(pan: f32) -> String {
    let percent = (pan * 100.0).round();
    if percent == 0.0 {
        "C".to_owned()
    } else if percent < 0.0 {
        format!("L{}", -percent)
    } else {
        format!("R{percent}")
    }
}

/// Angle of a pan value, in radians, clockwise from straight up.
fn angle(pan: f32) -> f32 {
    pan * SWEEP / 2.0
}

/// Draws a pan knob of diameter `size`; `changed()` tells a move.
pub fn knob(ui: &mut Ui, pan: &mut f32, size: f32, label: &str) -> Response {
    let theme = Theme::current();
    let p = &theme.palette;
    let (rect, mut response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click_and_drag());
    let before = *pan;
    if response.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        *pan = drag(*pan, response.drag_delta().y, fine);
    }
    if response.double_clicked() {
        *pan = 0.0;
    }
    if response.hovered() {
        let wheel = super::wheel_notches(ui);
        if wheel != 0.0 {
            *pan = (*pan + wheel * WHEEL_STEP).clamp(-1.0, 1.0);
        }
    }
    if let Some(asked) = super::assisted(ui, response.id) {
        *pan = match asked {
            #[allow(clippy::cast_possible_truncation)]
            super::Assist::Set(value) => (value as f32).clamp(-1.0, 1.0),
            super::Assist::Steps(steps) => (*pan + steps * WHEEL_STEP).clamp(-1.0, 1.0),
        };
    }
    if pan.to_bits() != before.to_bits() {
        response.mark_changed();
    }
    response.widget_info(|| WidgetInfo::slider(true, f64::from(*pan), format!("{label} pan")));
    super::offer_range(ui, response.id, -1.0..=1.0, f64::from(WHEEL_STEP));

    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    let center = rect.center();
    let radius = size / 2.0 - 3.0;
    let point = |a: f32, r: f32| center + vec2(a.sin(), -a.cos()) * r;
    let arc = |from: f32, to: f32, r: f32| -> Vec<_> {
        let steps = 32;
        (0..=steps)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let t = i as f32 / steps as f32;
                point(from + (to - from) * t, r)
            })
            .collect()
    };

    // Track and value arc (from the center to the value), outside the body.
    painter.add(Shape::line(
        arc(-SWEEP / 2.0, SWEEP / 2.0, radius + 1.0),
        Stroke::new(2.0, p.well),
    ));
    if *pan != 0.0 {
        painter.add(Shape::line(
            arc(0.0, angle(*pan), radius + 1.0),
            Stroke::new(2.0, p.accent),
        ));
    }
    let active = response.dragged() || response.has_focus();
    let body = if response.hovered() || active {
        p.raised_hover
    } else {
        p.raised
    };
    painter.circle_filled(center + vec2(0.0, 1.0), radius - 3.0, p.well);
    painter.circle(center, radius - 3.5, body, Stroke::new(1.0, p.line_strong));
    painter.line_segment(
        [point(angle(*pan), 3.0), point(angle(*pan), radius - 5.0)],
        Stroke::new(2.0, if active { p.accent } else { p.text }),
    );
    // Center detent mark.
    painter.line_segment(
        [point(0.0, radius + 3.0), point(0.0, radius + 5.0)],
        Stroke::new(1.0, p.text_faint),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_turns_and_stops() {
        assert!(drag(0.0, -10.0, false) > 0.0);
        assert!(drag(0.0, 10.0, false) < 0.0);
        assert_eq!(drag(0.0, -1000.0, false), 1.0);
        assert_eq!(drag(0.0, 1000.0, false), -1.0);
        assert!((drag(0.0, -80.0, false) - 1.0).abs() < 1e-6);
        assert!((drag(0.0, -80.0, true) - 0.1).abs() < 1e-6);
    }

    #[test]
    fn formats() {
        assert_eq!(format_pan(0.0), "C");
        assert_eq!(format_pan(0.004), "C");
        assert_eq!(format_pan(-0.4), "L40");
        assert_eq!(format_pan(0.25), "R25");
        assert_eq!(format_pan(1.0), "R100");
    }
}
