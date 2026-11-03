//! The controls of the console, each in its own module; drawn only from theme tokens.

pub mod badge;
pub mod fader;
pub mod knob;
pub mod meter;
pub mod scale;
pub mod strip;
pub mod toggle;

use eframe::egui::{Event, Ui};

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
