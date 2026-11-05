//! The options of the window: whether pult starts when the user signs in on the system, and
//! whether it then comes up in the background or with the window open.
//!
//! The dialog only says what the user chose. Making it so belongs to the program (`main`), which
//! tells the system first and keeps it in the settings; the dialog shows what holds afterwards.

use eframe::egui::{Align, Button, Id, Layout, Modal, RichText, Ui, vec2};

use crate::settings::{AutoStart, StartMode};
use crate::theme::Theme;
use crate::tray::START_LABEL;

/// The words of the second choice, which a screen reader and a test sheet go by.
pub const BACKGROUND_LABEL: &str = "Start in the background";

/// What the user did in the dialog during one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    /// A new choice for the start on signing in.
    pub choice: Option<AutoStart>,
    /// The dialog is done: "Close", Escape or a click beside it.
    pub close: bool,
}

/// What the choice `start` means, in one sentence under the checks.
#[must_use]
pub fn explain(start: AutoStart) -> &'static str {
    match (start.on, start.mode) {
        (false, _) => "pult starts only when you start it.",
        (true, StartMode::Tray) => {
            "pult plays the profile opened last and stays in the tray, without a window."
        }
        (true, StartMode::Window) => "pult opens its window with the profile opened last.",
    }
}

/// Draws the dialog over the window, with what holds now (`start`) and why the last choice did
/// not take (`problem`).
pub fn show(ui: &mut Ui, start: AutoStart, problem: Option<&str>) -> Answer {
    let theme = Theme::current();
    let p = &theme.palette;
    let mut answer = Answer::default();
    let modal = Modal::new(Id::unique("options")).show(ui.ctx(), |ui| {
        ui.set_width(380.0);
        ui.spacing_mut().item_spacing.y = 10.0;
        ui.label(RichText::new("pult options").size(16.0).color(p.text));

        let mut on = start.on;
        if ui.checkbox(&mut on, START_LABEL).changed() {
            answer.choice = Some(AutoStart { on, ..start });
        }
        // It says how that start comes up, so it means nothing while there is none; the choice
        // stays for when there is one again.
        let mut background = start.mode == StartMode::Tray;
        let response = ui.add_enabled_ui(start.on, |ui| {
            ui.checkbox(&mut background, BACKGROUND_LABEL)
        });
        if response.inner.changed() {
            let mode = if background {
                StartMode::Tray
            } else {
                StartMode::Window
            };
            answer.choice = Some(AutoStart { mode, ..start });
        }
        ui.label(RichText::new(explain(start)).size(12.5).color(p.text_dim));
        if let Some(problem) = problem {
            ui.label(RichText::new(problem).size(12.5).color(p.mute));
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let close =
                Button::new(RichText::new("Close").color(p.text)).min_size(vec2(110.0, 28.0));
            if ui.add(close).clicked() {
                answer.close = true;
            }
        });
    });
    if modal.should_close() {
        answer.close = true;
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_say_what_the_choice_does() {
        let start = |on, mode| explain(AutoStart { on, mode });
        assert!(start(false, StartMode::Window).contains("only when you start it"));
        assert_eq!(
            start(false, StartMode::Tray),
            start(false, StartMode::Window),
            "the way it starts means nothing while it does not start"
        );
        assert!(start(true, StartMode::Tray).contains("tray"));
        assert!(start(true, StartMode::Window).contains("window"));
    }
}
