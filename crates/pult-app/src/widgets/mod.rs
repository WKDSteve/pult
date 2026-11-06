//! The controls of the console, each in its own module; drawn only from theme tokens.

pub mod badge;
pub mod dial;
pub mod fader;
pub mod knob;
pub mod meter;
pub use pult_core::scale;
pub mod strip;
pub mod toggle;

use std::ops::RangeInclusive;

use eframe::egui::accesskit::{Action, ActionData};
use eframe::egui::{Event, Id, Ui};

/// What assistive technology asked of a slider of ours: a screen reader, or a test sheet that
/// goes the same way (UI Automation, AccessKit).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Assist {
    /// Take this value.
    Set(f64),
    /// Go this many steps, up positive.
    Steps(f32),
}

/// This frame's request for the slider `id`, the last one when there are several. A value that is
/// not a number is no request.
pub(crate) fn assisted(ui: &Ui, id: Id) -> Option<Assist> {
    ui.input(|input| {
        let set = input
            .accesskit_action_requests(id, Action::SetValue)
            .filter_map(|request| match request.data {
                Some(ActionData::NumericValue(value)) if !value.is_nan() => Some(value),
                _ => None,
            })
            .last();
        if let Some(value) = set {
            return Some(Assist::Set(value));
        }
        let up = input.num_accesskit_action_requests(id, Action::Increment);
        let down = input.num_accesskit_action_requests(id, Action::Decrement);
        #[allow(clippy::cast_precision_loss)]
        (up != down).then_some(Assist::Steps(up as f32 - down as f32))
    })
}

/// Says what a slider can be set to; without it the system reports a range of 0 to 0 and a screen
/// reader cannot set a level at all.
pub(crate) fn offer_range(ui: &Ui, id: Id, range: RangeInclusive<f64>, step: f64) {
    ui.ctx().accesskit_node_builder(id, |builder| {
        builder.set_min_numeric_value(*range.start());
        builder.set_max_numeric_value(*range.end());
        builder.set_numeric_value_step(step);
        builder.add_action(Action::SetValue);
        builder.add_action(Action::Increment);
        builder.add_action(Action::Decrement);
    });
}

/// Wheel notches over the last frame, up positive; a touchpad's many small events count one
/// each, so a slow swipe still moves in steps.
pub(crate) fn wheel_notches(ui: &Ui) -> f32 {
    ui.input(|i| {
        i.events
            .iter()
            .map(|event| match event {
                Event::MouseWheel { delta, .. } if delta.y != 0.0 => delta.y.signum(),
                _ => 0.0,
            })
            .sum()
    })
}
