//! The console's event log: what the desk reported and what it refused to do.
//!
//! A [`MixerView`](pult_core::view::MixerView) brings only the events since the previous view,
//! and a refused action comes back as the error of `Desk::act`; the log keeps both, newest
//! last, and the events view and the status line show them.

use std::time::Duration;

use eframe::egui::{Align2, Color32, CornerRadius, RichText, ScrollArea, Sense, Ui, pos2, vec2};
use pult_core::view::{EventKind, EventView};

use crate::theme::Theme;
use crate::widgets::badge::{elided, show_elided, tag};

/// Entries the log keeps; older ones fall off.
pub const CAPACITY: usize = 500;

/// Who wrote an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The desk: a device gone, back or refused.
    Desk(EventKind),
    /// The console: the desk refused an action (a gain out of range, an input left on no bus).
    Refused,
    /// The console: the desk confirmed an action worth noting (a profile saved).
    Done,
    /// The window: its settings could not be read or written, or the start on signing in not
    /// changed.
    Settings,
}

impl Source {
    /// The tag the events view shows, and its color.
    #[must_use]
    pub fn tag(self) -> (&'static str, Color32) {
        let p = &Theme::current().palette;
        match self {
            Self::Desk(EventKind::Lost) => ("GONE", p.mute),
            Self::Desk(EventKind::Back) => ("BACK", p.ok),
            Self::Desk(EventKind::Refused) => ("NOT YET", p.solo),
            Self::Desk(EventKind::Slow) => ("SLOW", p.solo),
            Self::Refused => ("REFUSED", p.mute),
            Self::Done => ("DONE", p.ok),
            Self::Settings => ("SETTINGS", p.mute),
        }
    }
}

/// One line of the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// When, as time since the route started.
    pub elapsed: Duration,
    pub text: String,
    pub source: Source,
}

/// The log itself, oldest first, at most [`CAPACITY`] entries.
#[derive(Clone, Debug, Default)]
pub struct Log {
    entries: Vec<Entry>,
}

impl Log {
    /// Takes the events a view brought.
    pub fn take(&mut self, events: &[EventView]) {
        self.entries.extend(events.iter().map(|event| Entry {
            elapsed: event.elapsed,
            text: event.text.clone(),
            source: Source::Desk(event.kind),
        }));
        self.trim();
    }

    /// Notes what an action did, as the desk says it.
    pub fn done(&mut self, elapsed: Duration, text: &str) {
        self.note(elapsed, text, Source::Done);
    }

    /// Notes an action the desk refused, with its reason, at `elapsed`.
    pub fn refused(&mut self, elapsed: Duration, reason: &str) {
        self.note(elapsed, reason, Source::Refused);
    }

    /// Notes a problem with the settings of the window at `elapsed`.
    pub fn settings(&mut self, elapsed: Duration, problem: &str) {
        self.note(elapsed, problem, Source::Settings);
    }

    fn note(&mut self, elapsed: Duration, text: &str, source: Source) {
        self.entries.push(Entry {
            elapsed,
            text: text.to_owned(),
            source,
        });
        self.trim();
    }

    fn trim(&mut self) {
        let excess = self.entries.len().saturating_sub(CAPACITY);
        self.entries.drain(..excess);
    }

    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    #[must_use]
    pub fn last(&self) -> Option<&Entry> {
        self.entries.last()
    }
}

/// `02:14:07`
#[must_use]
pub fn format_elapsed(elapsed: Duration) -> String {
    let s = elapsed.as_secs();
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

const ROW_HEIGHT: f32 = 28.0;
const TIME_WIDTH: f32 = 84.0;
const TAG_WIDTH: f32 = 64.0;

/// The events view: one row per entry, newest on top.
pub fn show(ui: &mut Ui, log: &Log) {
    let theme = Theme::current();
    let (p, m) = (&theme.palette, &theme.metrics);
    ui.horizontal(|ui| {
        ui.label(RichText::new("EVENTS").size(10.0).color(p.text_faint));
        let refused = log
            .entries()
            .iter()
            .filter(|e| e.source == Source::Refused)
            .count();
        let text = format!("{} entries · {refused} refused", log.entries().len());
        ui.label(RichText::new(text).size(10.0).color(p.text_faint));
    });
    ui.add_space(6.0);
    if log.entries().is_empty() {
        ui.label(
            RichText::new(
                "Nothing happened yet: devices that go, come back or are refused show up here.",
            )
            .size(12.0)
            .color(p.text_dim),
        );
        return;
    }
    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        let width = ui.available_width();
        for (row, entry) in log.entries().iter().rev().enumerate() {
            let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
            if !ui.is_rect_visible(rect) {
                continue;
            }
            let painter = ui.painter();
            if row % 2 == 0 {
                painter.rect_filled(rect, CornerRadius::same(m.radius_small), p.surface);
            }
            let y = rect.center().y;
            painter.text(
                pos2(rect.left() + 10.0, y),
                Align2::LEFT_CENTER,
                format_elapsed(entry.elapsed),
                Theme::value_font(11.0),
                p.text_faint,
            );
            let (name, color) = entry.source.tag();
            tag(
                painter,
                pos2(rect.left() + TIME_WIDTH, y),
                TAG_WIDTH,
                name,
                color,
            );
            let left = rect.left() + TIME_WIDTH + TAG_WIDTH + 12.0;
            let text_color = if entry.source == Source::Refused {
                p.text
            } else {
                p.text_dim
            };
            let galley = elided(
                painter,
                &entry.text,
                Theme::label_font(12.5),
                text_color,
                rect.right() - left - 10.0,
            );
            let at = pos2(left, y - galley.size().y / 2.0);
            show_elided(ui, at, galley, text_color, &entry.text, row);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(text: &str) -> EventView {
        EventView {
            elapsed: Duration::ZERO,
            kind: pult_core::view::EventKind::Lost,
            text: text.to_owned(),
        }
    }

    #[test]
    fn keeps_desk_events_and_refusals_in_order() {
        let mut log = Log::default();
        log.take(&[event("a"), event("b")]);
        log.take(&[]);
        log.refused(Duration::from_secs(3), "gain 30 dB is outside -60 to 24 dB");
        log.take(&[event("c")]);
        let lines: Vec<(&str, Source)> = log
            .entries()
            .iter()
            .map(|e| (e.text.as_str(), e.source))
            .collect();
        assert_eq!(
            lines,
            [
                ("a", Source::Desk(EventKind::Lost)),
                ("b", Source::Desk(EventKind::Lost)),
                ("gain 30 dB is outside -60 to 24 dB", Source::Refused),
                ("c", Source::Desk(EventKind::Lost)),
            ]
        );
        assert_eq!(log.last().map(|e| e.text.as_str()), Some("c"));
    }

    #[test]
    fn a_device_event_is_tagged_by_its_kind() {
        let tag = |kind| Source::Desk(kind).tag().0;
        assert_eq!(tag(EventKind::Lost), "GONE");
        assert_eq!(tag(EventKind::Back), "BACK");
        assert_eq!(tag(EventKind::Refused), "NOT YET");
        assert_eq!(tag(EventKind::Slow), "SLOW");
    }

    #[test]
    fn old_entries_fall_off() {
        let mut log = Log::default();
        log.take(&[event("first")]);
        for _ in 0..CAPACITY {
            log.refused(Duration::ZERO, "x");
        }
        assert_eq!(log.entries().len(), CAPACITY);
        assert!(log.entries().iter().all(|e| e.text == "x"));
    }

    #[test]
    fn elapsed() {
        assert_eq!(format_elapsed(Duration::from_secs(0)), "00:00:00");
        assert_eq!(format_elapsed(Duration::from_secs(8047)), "02:14:07");
        assert_eq!(format_elapsed(Duration::from_secs(100 * 3600)), "100:00:00");
    }
}
