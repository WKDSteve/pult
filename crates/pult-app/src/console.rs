//! The whole window: header bar, input strips, bus section, matrix and event views.

use std::time::Duration;

use eframe::egui::{
    self, Align, CornerRadius, Frame, Layout, Margin, Rect, RichText, ScrollArea, Sense, Stroke,
    StrokeKind, Ui, pos2, vec2,
};

use crate::theme::Theme;
use crate::widgets::meter::MeterState;
use crate::widgets::strip::{Zones, bus_strip, input_strip};
use crate::widgets::toggle::{Role, toggle};
use pult_core::view::{Action, EventView, MeterPoint, MixerView};

/// The views of the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Console,
    Matrix,
    Events,
}

/// Events the console keeps; older ones fall off.
pub const EVENT_LOG: usize = 500;

/// What the console keeps between frames.
#[derive(Clone, Debug, Default)]
pub struct ConsoleState {
    pub tab: Tab,
    input_meters: Vec<MeterState>,
    bus_meters: Vec<MeterState>,
    /// Clipped samples per bus at the previous view, to light the lamp on new ones.
    bus_clipped: Vec<u64>,
    /// Every event seen, oldest first; a view brings only the new ones.
    events: Vec<EventView>,
}

impl ConsoleState {
    /// Takes what a view brings that the console must remember: events and new clips.
    fn take(&mut self, view: &MixerView) {
        self.input_meters
            .resize_with(view.inputs.len(), MeterState::default);
        self.bus_meters
            .resize_with(view.buses.len(), MeterState::default);
        self.bus_clipped.resize(view.buses.len(), 0);
        for ((bus, meter), seen) in view
            .buses
            .iter()
            .zip(&mut self.bus_meters)
            .zip(&mut self.bus_clipped)
        {
            if bus.clipped > *seen {
                meter.mark_clipped();
            }
            *seen = bus.clipped;
        }
        self.events.extend(view.events.iter().cloned());
        let excess = self.events.len().saturating_sub(EVENT_LOG);
        self.events.drain(..excess);
    }

    /// Every event seen, oldest first.
    #[must_use]
    pub fn events(&self) -> &[EventView] {
        &self.events
    }
}

/// `02:14:07`
#[must_use]
pub fn format_runtime(runtime: Duration) -> String {
    let s = runtime.as_secs();
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// Draws the window into `ui` at time `now` (seconds); returns what the user changed.
pub fn show(ui: &mut Ui, view: &MixerView, state: &mut ConsoleState, now: f64) -> Vec<Action> {
    let theme = Theme::current();
    let p = &theme.palette;
    state.take(view);
    let mut actions = Vec::new();

    egui::Panel::top("header")
        .exact_size(theme.metrics.header_height)
        .frame(
            Frame::new()
                .fill(p.surface)
                .stroke(Stroke::new(1.0, p.line))
                .inner_margin(Margin::symmetric(14, 0)),
        )
        .show(ui, |ui| header_bar(ui, view, state, &mut actions));

    egui::Panel::bottom("status")
        .exact_size(24.0)
        .frame(
            Frame::new()
                .fill(p.surface)
                .stroke(Stroke::new(1.0, p.line))
                .inner_margin(Margin::symmetric(14, 0)),
        )
        .show(ui, |ui| status_bar(ui, view, &state.events));

    egui::CentralPanel::default()
        .frame(Frame::new().fill(p.base).inner_margin(Margin::same(10)))
        .show(ui, |ui| match state.tab {
            Tab::Console => console(ui, view, state, now, &mut actions),
            Tab::Matrix => matrix(ui, view, &mut actions),
            Tab::Events => events(ui, &state.events),
        });
    actions
}

fn header_bar(ui: &mut Ui, view: &MixerView, state: &mut ConsoleState, actions: &mut Vec<Action>) {
    let p = &Theme::current().palette;
    ui.horizontal_centered(|ui| {
        // Word mark: a small lit square and the name.
        let (dot, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
        ui.painter()
            .rect_filled(dot, CornerRadius::same(2), p.accent);
        ui.label(RichText::new("pult").size(17.0).strong().color(p.text));
        let profile = view
            .profile
            .as_ref()
            .and_then(|path| path.file_name())
            .map_or_else(|| "no profile".into(), |name| name.to_string_lossy());
        ui.label(RichText::new(profile).size(11.5).color(p.text_dim));
        ui.add_space(18.0);

        stat(ui, "RUNTIME", &format_runtime(view.elapsed), p.text);
        let dropouts = if view.dropouts_per_hour > 0.0 {
            p.solo
        } else {
            p.text
        };
        stat(
            ui,
            "DROPOUTS/H",
            &format!("{:.1}", view.dropouts_per_hour),
            dropouts,
        );
        let glitches = if view.glitches > 0 { p.solo } else { p.text };
        stat(ui, "GLITCHES", &view.glitches.to_string(), glitches);
        pill(ui, "RUNNING", p.ok);

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if ui.button("Save profile").clicked() {
                actions.push(Action::SaveProfile { path: None });
            }
            ui.add_space(8.0);
            let mut post = view.meter_point == MeterPoint::PostFader;
            let label = if post { "POST" } else { "PRE" };
            if toggle(ui, &mut post, label, Role::Assign, vec2(46.0, 22.0)).changed() {
                actions.push(Action::SetMeterPoint(if post {
                    MeterPoint::PostFader
                } else {
                    MeterPoint::PreFader
                }));
            }
            ui.label(RichText::new("METER").size(9.5).color(p.text_faint));
            ui.add_space(14.0);
            for (tab, name) in [
                (Tab::Events, "Events"),
                (Tab::Matrix, "Matrix"),
                (Tab::Console, "Console"),
            ] {
                tab_button(ui, &mut state.tab, tab, name);
            }
        });
    });
}

fn stat(ui: &mut Ui, label: &str, value: &str, color: egui::Color32) {
    let p = &Theme::current().palette;
    ui.label(RichText::new(label).size(9.5).color(p.text_faint));
    ui.label(
        RichText::new(value)
            .font(Theme::value_font(12.5))
            .color(color),
    );
    ui.add_space(10.0);
}

fn pill(ui: &mut Ui, text: &str, color: egui::Color32) {
    let font = Theme::label_font(9.5);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let size = galley.size() + vec2(18.0, 6.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(9), color.gamma_multiply(0.14));
    painter.rect_stroke(
        rect,
        CornerRadius::same(9),
        Stroke::new(1.0, color.gamma_multiply(0.5)),
        StrokeKind::Inside,
    );
    painter.circle_filled(pos2(rect.left() + 8.0, rect.center().y), 2.5, color);
    painter.galley(
        pos2(rect.left() + 13.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
}

fn tab_button(ui: &mut Ui, current: &mut Tab, tab: Tab, name: &str) {
    let p = &Theme::current().palette;
    let selected = *current == tab;
    let color = if selected { p.text } else { p.text_dim };
    let response = ui.add(
        egui::Button::new(RichText::new(name).size(12.0).color(color))
            .fill(if selected {
                p.raised
            } else {
                egui::Color32::TRANSPARENT
            })
            .stroke(Stroke::new(
                1.0,
                if selected {
                    p.line_strong
                } else {
                    egui::Color32::TRANSPARENT
                },
            ))
            .min_size(vec2(64.0, 24.0)),
    );
    if selected {
        let r = response.rect;
        ui.painter().line_segment(
            [
                pos2(r.left() + 10.0, r.bottom() + 6.0),
                pos2(r.right() - 10.0, r.bottom() + 6.0),
            ],
            (2.0, p.accent),
        );
    }
    if response.clicked() {
        *current = tab;
    }
}

fn status_bar(ui: &mut Ui, view: &MixerView, log: &[EventView]) {
    let p = &Theme::current().palette;
    ui.horizontal_centered(|ui| {
        if let Some(event) = log.last() {
            ui.label(
                RichText::new(format_runtime(event.elapsed))
                    .font(Theme::value_font(10.5))
                    .color(p.text_faint),
            );
            ui.label(RichText::new(&event.text).size(11.0).color(p.text_dim));
        } else {
            ui.label(RichText::new("no events").size(11.0).color(p.text_faint));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let solo = view.inputs.iter().filter(|i| i.solo).count();
            if solo > 0 {
                ui.label(
                    RichText::new(format!("SOLO {solo}"))
                        .size(10.0)
                        .color(p.solo),
                );
            }
            let text = format!("{} inputs · {} buses", view.inputs.len(), view.buses.len());
            ui.label(RichText::new(text).size(10.5).color(p.text_faint));
        });
    });
}

fn console(
    ui: &mut Ui,
    view: &MixerView,
    state: &mut ConsoleState,
    now: f64,
    actions: &mut Vec<Action>,
) {
    let theme = Theme::current();
    let (p, m) = (&theme.palette, &theme.metrics);
    let bus_names: Vec<String> = view.buses.iter().map(|b| b.name.clone()).collect();
    let height = ui
        .available_height()
        .max(Zones::min_height(bus_names.len()));
    #[allow(clippy::cast_precision_loss)]
    let section_width =
        view.buses.len() as f32 * (m.bus_strip_width + m.strip_gap) + 2.0 * m.strip_gap + 1.0;

    // Bus section on the right, set off by its own background.
    let full = ui.available_rect_before_wrap();
    let section = Rect::from_min_max(pos2(full.right() - section_width, full.top()), full.max);
    ui.painter()
        .rect_filled(section, CornerRadius::same(m.radius + 2), p.surface_alt);
    let mut x = section.left() + m.strip_gap;
    for (j, bus) in view.buses.iter().enumerate() {
        let rect = Rect::from_min_size(
            pos2(x, section.top() + m.strip_gap),
            vec2(m.bus_strip_width, height - 2.0 * m.strip_gap),
        );
        actions.extend(bus_strip(
            ui,
            rect,
            j,
            bus,
            view.buses.len(),
            &mut state.bus_meters[j],
            now,
        ));
        x += m.bus_strip_width + m.strip_gap;
    }

    // Input strips, scrolling sideways when they do not fit.
    let inputs_rect = Rect::from_min_max(
        full.min + vec2(0.0, m.strip_gap),
        pos2(section.left() - m.strip_gap * 2.0, full.bottom()),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(inputs_rect), |ui| {
        ScrollArea::horizontal().auto_shrink(false).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = m.strip_gap;
                for (i, input) in view.inputs.iter().enumerate() {
                    let (rect, _) = ui.allocate_exact_size(
                        vec2(m.strip_width, height - 2.0 * m.strip_gap),
                        Sense::hover(),
                    );
                    actions.extend(input_strip(
                        ui,
                        rect,
                        i,
                        input,
                        &bus_names,
                        &mut state.input_meters[i],
                        now,
                    ));
                }
            });
        });
    });
}

fn matrix(ui: &mut Ui, view: &MixerView, actions: &mut Vec<Action>) {
    let p = &Theme::current().palette;
    ui.label(RichText::new("ROUTING").size(10.0).color(p.text_faint));
    ui.add_space(4.0);
    egui::Grid::new("matrix")
        .spacing(vec2(6.0, 6.0))
        .show(ui, |ui| {
            ui.label("");
            for bus in &view.buses {
                ui.label(RichText::new(&bus.name).strong().color(p.text));
            }
            ui.end_row();
            for (i, input) in view.inputs.iter().enumerate() {
                ui.label(RichText::new(&input.name).color(if input.present {
                    p.text
                } else {
                    p.text_faint
                }));
                for (bus, name) in view.buses.iter().enumerate() {
                    let mut on = input.buses.contains(&bus);
                    if toggle(ui, &mut on, &name.name, Role::Assign, vec2(44.0, 22.0)).changed() {
                        actions.push(Action::Route { input: i, bus, on });
                    }
                }
                ui.end_row();
            }
        });
}

fn events(ui: &mut Ui, log: &[EventView]) {
    let p = &Theme::current().palette;
    ui.label(RichText::new("EVENTS").size(10.0).color(p.text_faint));
    ui.add_space(4.0);
    for event in log.iter().rev() {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format_runtime(event.elapsed))
                    .font(Theme::value_font(11.0))
                    .color(p.text_faint),
            );
            ui.label(RichText::new(&event.text).color(p.text));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pult_core::view::Desk;

    fn view_with(events: &[&str], clipped: u64) -> MixerView {
        let mut view = pult_core::view::FakeDesk::new().view();
        view.events = events
            .iter()
            .map(|text| EventView {
                elapsed: Duration::ZERO,
                text: (*text).to_owned(),
            })
            .collect();
        view.buses[0].clipped = clipped;
        view
    }

    #[test]
    fn events_pile_up_and_old_ones_fall_off() {
        let mut state = ConsoleState::default();
        state.take(&view_with(&["a", "b"], 0));
        state.take(&view_with(&[], 0));
        state.take(&view_with(&["c"], 0));
        let texts: Vec<&str> = state.events().iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["a", "b", "c"]);
        for _ in 0..EVENT_LOG {
            state.take(&view_with(&["x"], 0));
        }
        assert_eq!(state.events().len(), EVENT_LOG);
        assert_eq!(state.events()[0].text, "x");
    }

    #[test]
    fn a_new_bus_clip_lights_the_lamp_once() {
        let mut state = ConsoleState::default();
        state.take(&view_with(&[], 0));
        assert!(!state.bus_meters[0].clipped());
        state.take(&view_with(&[], 5));
        assert!(state.bus_meters[0].clipped());
        state.bus_meters[0].reset_clip();
        state.take(&view_with(&[], 5));
        assert!(!state.bus_meters[0].clipped());
    }

    #[test]
    fn runtime() {
        assert_eq!(format_runtime(Duration::from_secs(0)), "00:00:00");
        assert_eq!(
            format_runtime(Duration::from_secs(2 * 3600 + 14 * 60 + 7)),
            "02:14:07"
        );
        assert_eq!(format_runtime(Duration::from_secs(100 * 3600)), "100:00:00");
    }
}
