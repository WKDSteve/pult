//! How the window starts: the command line, and the screen it shows while no desk is open.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use eframe::egui::{
    Align, Button, Context, CornerRadius, Frame, Layout, Margin, RichText, Spinner, Stroke, Ui,
    vec2,
};

use crate::theme::Theme;

pub const USAGE: &str = "\
usage: pult-app [PROFILE | --demo]

  PROFILE   a pult profile (TOML), for example pult.toml; starts its route
  --demo    a desk in memory with moving meters, no audio device touched
  --help    this text
  --version the version

Without an argument the window opens and says how to start.";

/// What the command line asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Start {
    /// Open this profile and run its route.
    Profile(PathBuf),
    /// The demo desk.
    Demo,
    /// Nothing given: the window opens on the start screen.
    Nothing,
    Help,
    Version,
}

/// Reads the arguments after the program name.
///
/// # Errors
///
/// A message for an unknown option or more than one profile.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Start, String> {
    let mut start = Start::Nothing;
    for arg in args {
        let next = match arg.as_str() {
            "--demo" => Start::Demo,
            "-h" | "--help" => return Ok(Start::Help),
            "-V" | "--version" => return Ok(Start::Version),
            option if option.starts_with('-') && option.len() > 1 => {
                return Err(format!("unknown option {option}"));
            }
            path => Start::Profile(PathBuf::from(path)),
        };
        if start != Start::Nothing {
            return Err("give one profile or --demo, not both or two".to_owned());
        }
        start = next;
    }
    Ok(start)
}

/// What the user picked on the start screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Open the profile again (after plugging a device in, say).
    Retry,
    Demo,
}

/// A profile being opened on a thread of its own, so the window shows at once even when the
/// devices are slow to start.
#[derive(Debug)]
pub struct Opening<D> {
    result: Receiver<Result<D, String>>,
}

impl<D: Send + 'static> Opening<D> {
    /// Runs `open` on its own thread and repaints `ctx` when it is done.
    pub fn spawn(ctx: &Context, open: impl FnOnce() -> Result<D, String> + Send + 'static) -> Self {
        let (send, result) = mpsc::channel();
        let ctx = ctx.clone();
        let failed = send.clone();
        let spawned = thread::Builder::new()
            .name("pult-open".to_owned())
            .spawn(move || {
                // The window may be gone by now; then nobody waits for the desk.
                let _ = send.send(open());
                ctx.request_repaint();
            });
        if let Err(err) = spawned {
            let _ = failed.send(Err(format!("could not start opening the profile: {err}")));
        }
        Self { result }
    }

    /// The desk or why it did not open, once the thread is done; `None` while it runs.
    #[must_use]
    pub fn poll(&self) -> Option<Result<D, String>> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err("opening the profile stopped unexpectedly".to_owned()))
            }
        }
    }
}

/// The card in the middle of the start screen.
fn card(ui: &mut Ui, title: &str, profile: Option<&Path>, add: impl FnOnce(&mut Ui)) {
    let theme = Theme::current();
    let (p, m) = (&theme.palette, &theme.metrics);
    ui.with_layout(Layout::top_down(Align::Center), |ui| {
        ui.add_space((ui.available_height() * 0.22).max(24.0));
        Frame::new()
            .fill(p.surface)
            .stroke(Stroke::new(1.0, p.line))
            .corner_radius(CornerRadius::same(m.radius + 2))
            .inner_margin(Margin::same(24))
            .show(ui, |ui| {
                ui.set_width(520.0);
                ui.spacing_mut().item_spacing.y = 10.0;
                ui.label(RichText::new(title).size(18.0).color(p.text));
                if let Some(path) = profile {
                    ui.label(
                        RichText::new(path.display().to_string())
                            .font(Theme::value_font(11.0))
                            .color(p.text_dim),
                    );
                }
                add(ui);
            });
    });
}

/// The start screen while `profile` opens.
pub fn starting(ui: &mut Ui, profile: Option<&Path>) {
    let p = &Theme::current().palette;
    card(ui, "Starting the route", profile, |ui| {
        ui.horizontal(|ui| {
            ui.add(Spinner::new().size(14.0).color(p.accent));
            ui.label(
                RichText::new("Opening the devices of the profile…")
                    .size(12.5)
                    .color(p.text_dim),
            );
        });
    });
}

/// The start screen: why no desk is open (`problem`), and the way out.
pub fn screen(ui: &mut Ui, profile: Option<&Path>, problem: Option<&str>) -> Option<Choice> {
    let theme = Theme::current();
    let (p, m) = (&theme.palette, &theme.metrics);
    let mut choice = None;
    let title = if problem.is_some() {
        "The route did not start"
    } else {
        "No profile open"
    };
    card(ui, title, profile, |ui| {
        if let Some(problem) = problem {
            Frame::new()
                .fill(p.mute.gamma_multiply(theme.shades.wash))
                .stroke(Stroke::new(
                    1.0,
                    p.mute.gamma_multiply(theme.shades.outline),
                ))
                .corner_radius(CornerRadius::same(m.radius_small))
                .inner_margin(Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(problem).size(12.5).color(p.text));
                });
        } else {
            ui.label(
                RichText::new(
                    "Start pult-app with a profile, for example\n\
                             pult-app pult.toml\n\
                             or look around in the demo desk first.",
                )
                .size(12.5)
                .color(p.text_dim),
            );
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let size = vec2(120.0, 28.0);
            if profile.is_some()
                && ui
                    .add(Button::new(RichText::new("Try again").color(p.text)).min_size(size))
                    .clicked()
            {
                choice = Some(Choice::Retry);
            }
            if ui
                .add(Button::new(RichText::new("Demo desk").color(p.text)).min_size(size))
                .clicked()
            {
                choice = Some(Choice::Demo);
            }
        });
    });
    choice
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Start, String> {
        parse(list.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn command_lines() {
        assert_eq!(args(&[]), Ok(Start::Nothing));
        assert_eq!(args(&["--demo"]), Ok(Start::Demo));
        assert_eq!(
            args(&["studio.toml"]),
            Ok(Start::Profile(PathBuf::from("studio.toml")))
        );
        assert_eq!(args(&["--help", "x", "y"]), Ok(Start::Help));
        assert_eq!(args(&["-V"]), Ok(Start::Version));
        // A lone dash is a file name, as elsewhere.
        assert_eq!(args(&["-"]), Ok(Start::Profile(PathBuf::from("-"))));
    }

    fn wait<D: Send + 'static>(opening: &Opening<D>) -> Result<D, String> {
        let start = std::time::Instant::now();
        loop {
            if let Some(result) = opening.poll() {
                return result;
            }
            assert!(start.elapsed().as_secs() < 10, "the opening thread hangs");
            thread::yield_now();
        }
    }

    #[test]
    fn opening_hands_over_the_desk_or_the_problem() {
        let ctx = Context::default();
        assert_eq!(wait(&Opening::spawn(&ctx, || Ok(7))), Ok(7));
        assert_eq!(
            wait(&Opening::<u8>::spawn(&ctx, || Err("no device".to_owned()))),
            Err("no device".to_owned())
        );
    }

    #[test]
    fn opening_that_panics_says_so() {
        let ctx = Context::default();
        let opening = Opening::<u8>::spawn(&ctx, || panic!("boom"));
        assert!(wait(&opening).unwrap_err().contains("unexpectedly"));
    }

    #[test]
    fn bad_command_lines() {
        assert!(args(&["--loud"]).unwrap_err().contains("--loud"));
        assert!(args(&["a.toml", "b.toml"]).is_err());
        assert!(args(&["a.toml", "--demo"]).is_err());
    }
}
