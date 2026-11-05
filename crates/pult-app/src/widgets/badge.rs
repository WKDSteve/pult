//! Small labels: a status pill with a dot, a tag, and text cut to a width.

use std::hash::Hash;
use std::sync::Arc;

use eframe::egui::{
    Color32, CornerRadius, FontId, Galley, Painter, Pos2, Rect, Role, Sense, Stroke, StrokeKind,
    Ui, WidgetInfo, pos2, text::LayoutJob, text::TextWrapping, vec2,
};

use crate::theme::Theme;

/// A rounded pill with a lit dot (`● RUNNING`), for a state.
pub fn pill(ui: &mut Ui, text: &str, color: Color32) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), Theme::label_font(9.5), color);
    let size = galley.size() + vec2(18.0, 6.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, text));
    let painter = ui.painter();
    let radius = CornerRadius::same(9);
    painter.rect_filled(
        rect,
        radius,
        color.gamma_multiply(Theme::current().shades.wash),
    );
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, color.gamma_multiply(Theme::current().shades.outline)),
        StrokeKind::Inside,
    );
    painter.circle_filled(pos2(rect.left() + 8.0, rect.center().y), 2.5, color);
    painter.galley(
        pos2(rect.left() + 13.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
}

/// A square-cornered tag (`REFUSED`) of fixed `width`, left-aligned at `left_center`.
pub fn tag(painter: &Painter, left_center: Pos2, width: f32, text: &str, color: Color32) {
    let rect = Rect::from_min_size(left_center - vec2(0.0, 8.0), vec2(width, 16.0));
    let radius = CornerRadius::same(Theme::current().metrics.radius_small);
    painter.rect_filled(
        rect,
        radius,
        color.gamma_multiply(Theme::current().shades.wash),
    );
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, color.gamma_multiply(Theme::current().shades.outline)),
        StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        eframe::egui::Align2::CENTER_CENTER,
        text,
        Theme::label_font(9.0),
        color,
    );
}

/// `text` laid out on one line, cut with `…` where it would pass `width`.
#[must_use]
pub fn elided(
    painter: &Painter,
    text: &str,
    font: FontId,
    color: Color32,
    width: f32,
) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping::truncate_at_width(width.max(0.0));
    painter.layout_job(job)
}

/// Paints `galley` (from [`elided`]) at `pos`; when it was cut, hovering it shows `full` as a
/// tooltip, and screen readers get `full` as its name. `salt` keeps the hover areas apart.
pub fn show_elided(
    ui: &Ui,
    pos: Pos2,
    galley: Arc<Galley>,
    color: Color32,
    full: &str,
    salt: impl Hash + std::fmt::Debug,
) {
    let rect = Rect::from_min_size(pos, galley.size());
    let cut = galley.elided;
    ui.painter().galley(pos, galley, color);
    if cut {
        let id = ui.scope_id().with(("elided", salt));
        let response = ui.interact(rect, id, Sense::hover());
        response.widget_info(|| WidgetInfo::labeled(Role::Label, true, full));
        response.on_hover_text(full);
    }
}
