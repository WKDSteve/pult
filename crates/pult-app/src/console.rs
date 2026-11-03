//! The whole window: header bar, input strips, bus section, matrix and event views.

use std::time::Duration;

use eframe::egui::{
    self, Align, CornerRadius, Frame, Layout, Margin, Rect, RichText, ScrollArea, Sense, Stroke,
    Ui, pos2, vec2,
};

use crate::binding::Values;
use crate::layout::{self, Control, Meters, Place};
use crate::log::{self, Log, Source, format_elapsed};
use crate::matrix;
use crate::theme::Theme;
use crate::widgets::badge::pill;
use crate::widgets::strip::{self, Zones};
use crate::widgets::toggle::{Role, toggle};
use pult_core::view::{Action, Desk, MeterPoint, MixerView};

/// The views of the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Console,
    Matrix,
    Events,
}

/// What the console keeps between frames.
#[derive(Clone, Debug, Default)]
pub struct ConsoleState {
    pub tab: Tab,
    meters: Meters,
    /// Clipped samples per bus at the previous view, to light the lamp on new ones.
    bus_clipped: Vec<u64>,
    /// What the desk reported and refused, oldest first.
    log: Log,
}

impl ConsoleState {
    /// Takes what a view brings that the console must remember: events and new clips. The
    /// window calls it itself while it is hidden and draws nothing.
    pub fn take(&mut self, view: &MixerView) {
        self.meters.fit(view);
        self.bus_clipped.resize(view.buses.len(), 0);
        for ((bus, meter), seen) in view
            .buses
            .iter()
            .zip(self.meters.buses_mut())
            .zip(&mut self.bus_clipped)
        {
            if bus.clipped > *seen {
                meter.mark_clipped();
            }
            *seen = bus.clipped;
        }
        self.log.take(&view.events);
    }

    /// Hands `action` to `desk` at `elapsed` and notes the answer: a refusal with its reason,
    /// and what a save did, so the status line confirms it. Other confirmations stay out of
    /// the log; a fader drag would fill it.
    pub fn act(&mut self, desk: &mut dyn Desk, elapsed: Duration, action: Action) {
        let confirm = matches!(action, Action::SaveProfile { .. });
        match desk.act(action) {
            Ok(lines) if confirm => {
                for line in lines {
                    self.log.done(elapsed, &line);
                }
            }
            Ok(_) => {}
            Err(reason) => self.refused(elapsed, &reason),
        }
    }

    /// Notes that the desk refused an action, with its reason, at `elapsed`; the status line and
    /// the events view show it.
    pub fn refused(&mut self, elapsed: Duration, reason: &str) {
        self.log.refused(elapsed, reason);
    }

    /// Notes a problem with the settings of the window at `elapsed`, as the status line shows.
    pub fn settings_problem(&mut self, elapsed: Duration, problem: &str) {
        self.log.settings(elapsed, problem);
    }

    #[must_use]
    pub fn log(&self) -> &Log {
        &self.log
    }
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
        .show(ui, |ui| status_bar(ui, view, &state.log));

    egui::CentralPanel::default()
        .frame(Frame::new().fill(p.base).inner_margin(Margin::same(10)))
        .show(ui, |ui| match state.tab {
            Tab::Console => console(ui, view, state, now, &mut actions),
            Tab::Matrix => matrix::show(ui, view, &mut actions),
            Tab::Events => log::show(ui, &state.log),
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

        stat(ui, "RUNTIME", &format_elapsed(view.elapsed), p.text);
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
            if toggle(
                ui,
                &mut post,
                label,
                "Meter after fader",
                Role::Assign,
                vec2(46.0, 22.0),
            )
            .changed()
            {
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

fn status_bar(ui: &mut Ui, view: &MixerView, log: &Log) {
    let p = &Theme::current().palette;
    ui.horizontal_centered(|ui| {
        if let Some(entry) = log.last() {
            ui.label(
                RichText::new(format_elapsed(entry.elapsed))
                    .font(Theme::value_font(10.5))
                    .color(p.text_faint),
            );
            let color = match entry.source {
                Source::Refused | Source::Settings => {
                    let (tag, color) = entry.source.tag();
                    ui.label(RichText::new(tag).size(9.5).color(color));
                    p.mute
                }
                Source::Done => {
                    ui.label(RichText::new("DONE").size(9.5).color(p.ok));
                    p.text_dim
                }
                Source::Desk => p.text_dim,
            };
            ui.label(RichText::new(&entry.text).size(11.0).color(color));
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
    let elements = layout::standard(view);
    let strips = |place| {
        elements
            .iter()
            .filter_map(move |element| match &element.control {
                Control::Strip(strip) if element.place == place => Some(strip),
                _ => None,
            })
    };
    let mut values = Values::new(view);
    let height = ui
        .available_height()
        .max(Zones::min_height(view.buses.len()));
    #[allow(clippy::cast_precision_loss)]
    let section_width = strips(Place::BusSection).count() as f32
        * (m.bus_strip_width + m.strip_gap)
        + 2.0 * m.strip_gap
        + 1.0;

    // Bus section on the right, set off by its own background.
    let full = ui.available_rect_before_wrap();
    let section = Rect::from_min_max(pos2(full.right() - section_width, full.top()), full.max);
    ui.painter()
        .rect_filled(section, CornerRadius::same(m.radius + 2), p.surface_alt);
    let mut x = section.left() + m.strip_gap;
    for bus_strip in strips(Place::BusSection) {
        let rect = Rect::from_min_size(
            pos2(x, section.top() + m.strip_gap),
            vec2(m.bus_strip_width, height - 2.0 * m.strip_gap),
        );
        actions.extend(strip::show(
            ui,
            rect,
            bus_strip,
            &mut values,
            &mut state.meters,
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
                for input_strip in strips(Place::InputRow) {
                    let (rect, _) = ui.allocate_exact_size(
                        vec2(m.strip_width, height - 2.0 * m.strip_gap),
                        Sense::hover(),
                    );
                    actions.extend(strip::show(
                        ui,
                        rect,
                        input_strip,
                        &mut values,
                        &mut state.meters,
                        now,
                    ));
                }
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pult_core::view::{Desk, EventView};

    fn view_with(events: &[&str], clipped: u64) -> MixerView {
        let mut view = pult_core::view::FakeDesk::new().view();
        view.events = events
            .iter()
            .map(|text| EventView {
                elapsed: Duration::ZERO,
                kind: pult_core::view::EventKind::Lost,
                text: (*text).to_owned(),
            })
            .collect();
        view.buses[0].clipped = clipped;
        view
    }

    #[test]
    fn view_events_and_refusals_reach_the_log() {
        let mut state = ConsoleState::default();
        state.take(&view_with(&["a", "b"], 0));
        state.take(&view_with(&[], 0));
        state.refused(Duration::from_secs(1), "no");
        state.take(&view_with(&["c"], 0));
        let lines: Vec<(&str, Source)> = state
            .log()
            .entries()
            .iter()
            .map(|e| (e.text.as_str(), e.source))
            .collect();
        assert_eq!(
            lines,
            [
                ("a", Source::Desk),
                ("b", Source::Desk),
                ("no", Source::Refused),
                ("c", Source::Desk),
            ]
        );
    }

    #[test]
    fn a_new_bus_clip_lights_the_lamp_once() {
        let mut state = ConsoleState::default();
        state.take(&view_with(&[], 0));
        assert!(!state.meters.buses_mut()[0].clipped());
        state.take(&view_with(&[], 5));
        assert!(state.meters.buses_mut()[0].clipped());
        state.meters.buses_mut()[0].reset_clip();
        state.take(&view_with(&[], 5));
        assert!(!state.meters.buses_mut()[0].clipped());
    }
}
