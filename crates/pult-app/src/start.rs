//! How the window starts: the command line, and the screen it shows while no desk is open.

use std::path::{Path, PathBuf};

use eframe::egui::{
    Align, Button, CornerRadius, Frame, Layout, Margin, RichText, Stroke, Ui, vec2,
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

/// The start screen: why no desk is open (`problem`), and the way out.
pub fn screen(ui: &mut Ui, profile: Option<&Path>, problem: Option<&str>) -> Option<Choice> {
    let theme = Theme::current();
    let (p, m) = (&theme.palette, &theme.metrics);
    let mut choice = None;
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
                let title = if problem.is_some() {
                    "The route did not start"
                } else {
                    "No profile open"
                };
                ui.label(RichText::new(title).size(18.0).color(p.text));
                if let Some(path) = profile {
                    ui.label(
                        RichText::new(path.display().to_string())
                            .font(Theme::value_font(11.0))
                            .color(p.text_dim),
                    );
                }
                if let Some(problem) = problem {
                    Frame::new()
                        .fill(p.mute.gamma_multiply(0.1))
                        .stroke(Stroke::new(1.0, p.mute.gamma_multiply(0.45)))
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
                            .add(
                                Button::new(RichText::new("Try again").color(p.text))
                                    .min_size(size),
                            )
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

    #[test]
    fn bad_command_lines() {
        assert!(args(&["--loud"]).unwrap_err().contains("--loud"));
        assert!(args(&["a.toml", "b.toml"]).is_err());
        assert!(args(&["a.toml", "--demo"]).is_err());
    }
}
