//! The long fader of a strip: drag, wheel, keyboard, double click to 0 dB.

use eframe::egui::{
    self, Align2, CornerRadius, Key, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2,
    WidgetInfo, pos2, vec2,
};

use super::scale::{self, FADER_FLOOR_DB, FADER_MARKS, FADER_TOP_DB};
use crate::theme::Theme;

/// Gain change per wheel notch or arrow key, and with Shift held.
const STEP_DB: f32 = 0.5;
const FINE_STEP_DB: f32 = 0.1;
/// Bigger step for Page Up / Page Down.
const PAGE_DB: f32 = 3.0;
/// Shift slows a drag down by this factor.
const FINE_DRAG: f32 = 0.1;
/// Width of the scale numbers left of the slot.
const SCALE_WIDTH: f32 = 25.0;

/// The gain after dragging the cap by `dy` points (down is positive) on a fader whose cap
/// travels `travel` points.
#[must_use]
pub fn drag(db: f32, dy: f32, travel: f32, fine: bool) -> f32 {
    if travel <= 0.0 {
        return db;
    }
    let speed = if fine { FINE_DRAG } else { 1.0 };
    let pos = (scale::fader_pos(db) - dy / travel * speed).clamp(0.0, 1.0);
    scale::fader_db(pos)
}

/// The gain after `steps` wheel notches or key presses (up is positive). Off comes back at the
/// floor; stepping below the floor turns the fader off.
#[must_use]
pub fn step(db: f32, steps: f32, size: f32) -> f32 {
    if steps == 0.0 {
        return db;
    }
    let from = if db < FADER_FLOOR_DB {
        FADER_FLOOR_DB - size
    } else {
        db
    };
    let to = from + steps * size;
    if to < FADER_FLOOR_DB {
        f32::NEG_INFINITY
    } else {
        // Snap to the step grid so repeated steps land on round values. A gain above the scale
        // (a profile may set up to +24 dB) is a ceiling of its own: stepping up never lowers it.
        ((to / size).round() * size).min(FADER_TOP_DB.max(db))
    }
}

/// Draws a fader into a `size` area and lets the user move it; `changed()` tells a move.
pub fn fader(ui: &mut Ui, gain_db: &mut f32, size: Vec2, label: &str) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let travel = rect.height() - Theme::current().metrics.cap_size[1];

    let before = *gain_db;
    if response.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        *gain_db = drag(*gain_db, response.drag_delta().y, travel, fine);
    }
    if response.double_clicked() {
        *gain_db = 0.0;
    }
    if response.hovered() {
        let (wheel, fine) = (super::wheel_notches(ui), ui.input(|i| i.modifiers.shift));
        if wheel != 0.0 {
            let size = if fine { FINE_STEP_DB } else { STEP_DB };
            *gain_db = step(*gain_db, wheel, size);
        }
    }
    if response.has_focus() {
        *gain_db = ui.input(|i| keys(i, *gain_db));
    }
    // Bits, not `!=`: -inf to -inf is no change, and any other difference is one.
    if gain_db.to_bits() != before.to_bits() {
        response.mark_changed();
    }
    response
        .widget_info(|| WidgetInfo::slider(true, f64::from(*gain_db), format!("{label} gain dB")));
    if ui.is_rect_visible(rect) {
        paint(ui, rect, *gain_db, &response);
    }
    response
}

/// The gain after this frame's keys: arrows step, Page Up/Down jump, 0 resets.
fn keys(input: &egui::InputState, db: f32) -> f32 {
    if input.key_pressed(Key::Num0) {
        return 0.0;
    }
    let size = if input.modifiers.shift {
        FINE_STEP_DB
    } else {
        STEP_DB
    };
    let mut steps = 0.0;
    let mut page = 0.0;
    for event in &input.events {
        if let egui::Event::Key {
            key, pressed: true, ..
        } = event
        {
            match key {
                Key::ArrowUp => steps += 1.0,
                Key::ArrowDown => steps -= 1.0,
                Key::PageUp => page += 1.0,
                Key::PageDown => page -= 1.0,
                _ => {}
            }
        }
    }
    step(step(db, steps, size), page, PAGE_DB)
}

fn paint(ui: &Ui, rect: Rect, gain_db: f32, response: &Response) {
    let theme = Theme::current();
    let (p, m) = (&theme.palette, &theme.metrics);
    let [cap_w, cap_h] = m.cap_size;
    let slot_x = rect.left() + SCALE_WIDTH + (rect.width() - SCALE_WIDTH) / 2.0;
    let top = rect.top() + cap_h / 2.0;
    let travel = rect.height() - cap_h;
    let y_of = |db: f32| top + travel * (1.0 - scale::fader_pos(db));
    let painter = ui.painter_at(rect.expand(1.0));

    // Scale: ticks across the slot, numbers on the left; 0 dB stands out.
    for mark in FADER_MARKS {
        let y = y_of(mark).round() + 0.5;
        let unity = mark == 0.0;
        let tick = if unity { p.text_dim } else { p.line_strong };
        let half = if unity {
            cap_w / 2.0
        } else {
            cap_w / 2.0 - 6.0
        };
        painter.line_segment(
            [pos2(slot_x - half, y), pos2(slot_x + half, y)],
            (1.0, tick),
        );
        let color = if unity { p.text } else { p.text_faint };
        let text = if mark <= FADER_FLOOR_DB {
            "-inf".to_owned()
        } else {
            scale::format_mark(mark)
        };
        painter.text(
            pos2(rect.left() + SCALE_WIDTH - 4.0, y),
            Align2::RIGHT_CENTER,
            text,
            Theme::value_font(8.5),
            color,
        );
    }

    // Slot.
    let slot = Rect::from_center_size(pos2(slot_x, rect.center().y), vec2(4.0, travel + 4.0));
    painter.rect_filled(slot, CornerRadius::same(2), p.well);
    painter.rect_stroke(
        slot,
        CornerRadius::same(2),
        Stroke::new(1.0, p.line),
        StrokeKind::Outside,
    );

    // Cap: a raised block with a center line; the line lights up while in use.
    let cap_y = if gain_db.is_finite() {
        y_of(gain_db)
    } else {
        top + travel
    };
    let cap = Rect::from_center_size(pos2(slot_x, cap_y), vec2(cap_w, cap_h));
    let active = response.dragged() || response.has_focus();
    let fill = if response.hovered() || active {
        p.raised_hover
    } else {
        p.raised
    };
    painter.rect_filled(
        cap.translate(vec2(0.0, 1.5)),
        CornerRadius::same(m.radius_small),
        p.well,
    );
    painter.rect_filled(cap, CornerRadius::same(m.radius_small), fill);
    painter.rect_stroke(
        cap,
        CornerRadius::same(m.radius_small),
        Stroke::new(1.0, if active { p.accent } else { p.line_strong }),
        StrokeKind::Inside,
    );
    for dy in [-4.0, 4.0] {
        let y = cap.center().y + dy;
        painter.line_segment(
            [pos2(cap.left() + 6.0, y), pos2(cap.right() - 6.0, y)],
            (1.0, p.line_strong),
        );
    }
    painter.line_segment(
        [
            pos2(cap.left() + 3.0, cap.center().y),
            pos2(cap.right() - 3.0, cap.center().y),
        ],
        (2.0, if active { p.accent } else { p.text }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_up_raises_and_stops_at_the_ends() {
        assert!(drag(0.0, -10.0, 300.0, false) > 0.0);
        assert!(drag(0.0, 10.0, 300.0, false) < 0.0);
        assert_eq!(drag(0.0, -1000.0, 300.0, false), FADER_TOP_DB);
        assert_eq!(drag(0.0, 1000.0, 300.0, false), f32::NEG_INFINITY);
        assert_eq!(drag(-6.0, 50.0, 0.0, false), -6.0);
    }

    #[test]
    fn shift_drag_is_ten_times_finer() {
        let coarse = drag(0.0, -3.0, 300.0, false);
        let fine = drag(0.0, -30.0, 300.0, true);
        assert!((coarse - fine).abs() < 1e-3, "{coarse} vs {fine}");
    }

    #[test]
    fn steps_snap_and_cross_off() {
        assert_eq!(step(0.0, 1.0, STEP_DB), 0.5);
        assert_eq!(step(-6.2, 1.0, STEP_DB), -5.5);
        assert_eq!(step(11.8, 1.0, STEP_DB), FADER_TOP_DB);
        assert_eq!(step(FADER_FLOOR_DB, -1.0, STEP_DB), f32::NEG_INFINITY);
        assert_eq!(step(f32::NEG_INFINITY, 1.0, STEP_DB), FADER_FLOOR_DB);
        assert_eq!(step(f32::NEG_INFINITY, -1.0, STEP_DB), f32::NEG_INFINITY);
        assert_eq!(step(-3.0, 0.0, STEP_DB), -3.0);
        assert_eq!(step(0.0, -1.0, PAGE_DB), -3.0);
        // Above the scale: up keeps the gain, down steps from it.
        assert_eq!(step(18.0, 1.0, STEP_DB), 18.0);
        assert_eq!(step(18.0, -1.0, STEP_DB), 17.5);
    }
}
