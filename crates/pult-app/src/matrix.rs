//! The matrix view: inputs as rows, buses as columns, a crosspoint where an input feeds a bus.

use eframe::egui::{
    Align2, CornerRadius, Frame, Margin, Pos2, Rect, Response, RichText, ScrollArea, Sense, Stroke,
    StrokeKind, Ui, WidgetInfo, pos2, vec2,
};
use pult_core::view::{Action, InputView, MixerView, SourceKind};

use crate::binding::{Binding, Value};
use crate::theme::Theme;
use crate::widgets::badge::{elided, show_elided};
use crate::widgets::strip::format_rate;

const NAME_WIDTH: f32 = 230.0;
const CELL_WIDTH: f32 = 96.0;
const CELL_HEIGHT: f32 = 38.0;
const HEAD_HEIGHT: f32 = 58.0;
/// The level bar under an input's name.
const BAR_SIZE: [f32; 2] = [72.0, 3.0];

/// The cell under `pointer` as (row, column) in a grid whose cells start at `origin`; `None`
/// outside the cells.
#[must_use]
pub fn cell_at(origin: Pos2, rows: usize, columns: usize, pointer: Pos2) -> Option<(usize, usize)> {
    let offset = pointer - origin;
    if offset.x < 0.0 || offset.y < 0.0 {
        return None;
    }
    // Pointer positions are small and positive here; the floor is the cell index.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (column, row) = (
        (offset.x / CELL_WIDTH).floor() as usize,
        (offset.y / CELL_HEIGHT).floor() as usize,
    );
    (row < rows && column < columns).then_some((row, column))
}

/// Draws the matrix of `view`; a click on a crosspoint asks to route or unroute.
pub fn show(ui: &mut Ui, view: &MixerView, actions: &mut Vec<Action>) {
    let theme = Theme::current();
    let (p, m) = (&theme.palette, &theme.metrics);
    ui.horizontal(|ui| {
        ui.label(RichText::new("ROUTING").size(10.0).color(p.text_faint));
        ui.label(
            RichText::new("click a crosspoint to send an input to a bus")
                .size(10.0)
                .color(p.text_faint),
        );
    });
    ui.add_space(6.0);
    ScrollArea::both().auto_shrink(false).show(ui, |ui| {
        Frame::new()
            .fill(p.surface)
            .stroke(Stroke::new(1.0, p.line))
            .corner_radius(CornerRadius::same(m.radius))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| grid(ui, view, actions));
    });
}

fn grid(ui: &mut Ui, view: &MixerView, actions: &mut Vec<Action>) {
    let theme = Theme::current();
    let (p, m) = (&theme.palette, &theme.metrics);
    let (rows, columns) = (view.inputs.len(), view.buses.len());
    #[allow(clippy::cast_precision_loss)]
    let size = vec2(
        NAME_WIDTH + columns as f32 * CELL_WIDTH,
        HEAD_HEIGHT + rows as f32 * CELL_HEIGHT,
    );
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let origin = rect.min + vec2(NAME_WIDTH, HEAD_HEIGHT);
    let hovered = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|pos| rect.contains(*pos))
        .and_then(|pos| cell_at(origin, rows, columns, pos));
    let cell = |row: usize, column: usize| {
        #[allow(clippy::cast_precision_loss)]
        let min = origin + vec2(column as f32 * CELL_WIDTH, row as f32 * CELL_HEIGHT);
        Rect::from_min_size(min, vec2(CELL_WIDTH, CELL_HEIGHT))
    };

    // Row and column of the pointer, lit faintly across the whole grid.
    if let Some((row, column)) = hovered {
        let tint = p.raised.gamma_multiply(theme.shades.hover);
        let row_rect = Rect::from_x_y_ranges(rect.x_range(), cell(row, 0).y_range());
        let column_rect = Rect::from_x_y_ranges(cell(0, column).x_range(), rect.y_range());
        ui.painter()
            .rect_filled(row_rect, CornerRadius::same(m.radius_small), tint);
        ui.painter()
            .rect_filled(column_rect, CornerRadius::same(m.radius_small), tint);
    }

    for column in 0..columns {
        let head = Rect::from_min_size(
            pos2(cell(0, column).left(), rect.top()),
            vec2(CELL_WIDTH, HEAD_HEIGHT),
        );
        column_head(ui, head, view, column);
    }
    for (row, input) in view.inputs.iter().enumerate() {
        let name = Rect::from_min_size(
            pos2(rect.left(), cell(row, 0).top()),
            vec2(NAME_WIDTH, CELL_HEIGHT),
        );
        row_head(ui, name, row, input);
        let y = name.bottom().round() - 0.5;
        if row + 1 < rows {
            ui.painter()
                .line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], (1.0, p.line));
        }
        for column in 0..view.buses.len() {
            let route = Binding::Route {
                input: row,
                bus: column,
            };
            let label = route.title(view);
            let on = route.read(view) == Some(Value::Switch(true));
            let response = crosspoint(
                ui,
                cell(row, column),
                (row, column),
                on,
                input.present,
                &label,
            );
            if response.clicked() {
                actions.extend(route.write(Value::Switch(!on)));
            }
        }
    }
    let y = origin.y.round() - 0.5;
    ui.painter().line_segment(
        [pos2(rect.left(), y), pos2(rect.right(), y)],
        (1.0, p.line_strong),
    );
}

fn column_head(ui: &Ui, rect: Rect, view: &MixerView, column: usize) {
    let p = &Theme::current().palette;
    let bus = &view.buses[column];
    let painter = ui.painter();
    let x = rect.center().x;
    let name_color = if bus.present { p.text } else { p.mute };
    painter.text(
        pos2(x, rect.top() + 10.0),
        Align2::CENTER_CENTER,
        &bus.name,
        Theme::label_font(14.0),
        name_color,
    );
    let device = elided(
        painter,
        &bus.device,
        Theme::label_font(10.0),
        p.text_dim,
        CELL_WIDTH - 12.0,
    );
    let at = pos2(x - device.size().x / 2.0, rect.top() + 21.0);
    show_elided(ui, at, device, p.text_dim, &bus.device, ("bus", column));
    let fed = view
        .inputs
        .iter()
        .filter(|input| input.buses.contains(&column))
        .count();
    let count = if fed == 1 {
        "1 input".to_owned()
    } else {
        format!("{fed} inputs")
    };
    painter.text(
        pos2(x, rect.top() + 44.0),
        Align2::CENTER_CENTER,
        count,
        Theme::label_font(9.5),
        p.text_faint,
    );
}

fn row_head(ui: &Ui, rect: Rect, row: usize, input: &InputView) {
    let p = &Theme::current().palette;
    let painter = ui.painter();
    let name_color = if input.present { p.text } else { p.text_faint };
    let name = elided(
        painter,
        &input.name,
        Theme::label_font(12.5),
        name_color,
        NAME_WIDTH - 24.0,
    );
    let name_height = name.size().y;
    let at = pos2(rect.left() + 4.0, rect.top() + 5.0);
    show_elided(ui, at, name, name_color, &input.name, ("input", row));
    let kind = match input.kind {
        SourceKind::Device => "DEV",
        SourceKind::Program => "APP",
    };
    let detail = if input.present {
        format!("{kind} · {}", format_rate(input.sample_rate))
    } else {
        format!("{kind} · missing")
    };
    let detail_y = rect.top() + 5.0 + name_height + 7.0;
    painter.text(
        pos2(rect.left() + 4.0, detail_y),
        Align2::LEFT_CENTER,
        detail,
        Theme::label_font(9.5),
        if input.present { p.text_faint } else { p.mute },
    );
    let bar = Rect::from_min_size(
        pos2(rect.left() + 86.0, detail_y - BAR_SIZE[1] / 2.0),
        vec2(BAR_SIZE[0], BAR_SIZE[1]),
    );
    crate::widgets::meter::level_bar(painter, bar, input.peak_db);
}

/// One crosspoint; its id comes from its place, not its label: two inputs may share a name.
fn crosspoint(
    ui: &mut Ui,
    rect: Rect,
    place: (usize, usize),
    on: bool,
    present: bool,
    label: &str,
) -> Response {
    let p = &Theme::current().palette;
    let id = ui.scope_id().with(("crosspoint", place));
    let response = ui.interact(rect, id, Sense::click());
    response.widget_info(|| WidgetInfo::selected(eframe::egui::Role::CheckBox, true, on, label));
    let painter = ui.painter();
    let center = rect.center();
    let shades = &Theme::current().shades;
    let dim = |c: eframe::egui::Color32| {
        if present {
            c
        } else {
            c.gamma_multiply(shades.absent)
        }
    };
    if on {
        painter.circle_filled(center, 11.0, dim(p.accent.gamma_multiply(shades.halo)));
        painter.circle_filled(center, 7.0, dim(p.accent));
    } else {
        let ring = if response.hovered() {
            p.text_dim
        } else {
            p.line_strong
        };
        painter.circle_stroke(center, 6.0, Stroke::new(1.5, dim(ring)));
    }
    if response.has_focus() {
        painter.rect_stroke(
            rect.shrink(4.0),
            CornerRadius::same(Theme::current().metrics.radius_small),
            Stroke::new(1.0, p.accent),
            StrokeKind::Inside,
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_under_the_pointer() {
        let origin = pos2(100.0, 50.0);
        assert_eq!(cell_at(origin, 3, 2, pos2(100.0, 50.0)), Some((0, 0)));
        assert_eq!(
            cell_at(
                origin,
                3,
                2,
                pos2(100.0 + CELL_WIDTH + 1.0, 50.0 + 2.0 * CELL_HEIGHT + 1.0)
            ),
            Some((2, 1))
        );
        assert_eq!(cell_at(origin, 3, 2, pos2(99.0, 60.0)), None);
        assert_eq!(cell_at(origin, 3, 2, pos2(110.0, 49.0)), None);
        assert_eq!(
            cell_at(origin, 3, 2, pos2(100.0 + 2.0 * CELL_WIDTH, 60.0)),
            None
        );
        assert_eq!(
            cell_at(origin, 3, 2, pos2(110.0, 50.0 + 3.0 * CELL_HEIGHT)),
            None
        );
    }
}
