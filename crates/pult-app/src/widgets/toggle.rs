//! Latching buttons of a strip: mute, solo, bus assignment.

use eframe::egui::{
    Align2, Color32, CornerRadius, Response, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo,
};

use crate::theme::Theme;

/// What a toggle switches; decides its color when on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Mute,
    Solo,
    /// Sends the input to a bus.
    Assign,
}

impl Role {
    fn color(self) -> Color32 {
        let p = &Theme::current().palette;
        match self {
            Self::Mute => p.mute,
            Self::Solo => p.solo,
            Self::Assign => p.accent,
        }
    }
}

/// Draws a toggle of `size` with `text`; a click flips `on` and marks the response changed.
pub fn toggle(ui: &mut Ui, on: &mut bool, text: &str, role: Role, size: Vec2) -> Response {
    let theme = Theme::current();
    let (p, m) = (&theme.palette, &theme.metrics);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| WidgetInfo::selected(eframe::egui::Role::CheckBox, true, *on, text));
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    let radius = CornerRadius::same(m.radius_small);
    let color = role.color();
    let (fill, stroke, text_color) = if *on {
        (color, color, p.well)
    } else if response.hovered() {
        (p.raised_hover, p.line_strong, p.text)
    } else {
        (p.raised, p.line, p.text_dim)
    };
    painter.rect_filled(rect, radius, fill);
    painter.rect_stroke(rect, radius, Stroke::new(1.0, stroke), StrokeKind::Inside);
    if response.has_focus() {
        painter.rect_stroke(
            rect.expand(1.5),
            radius,
            Stroke::new(1.0, p.accent),
            StrokeKind::Outside,
        );
    }
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        Theme::label_font(11.0),
        text_color,
    );
    response
}
