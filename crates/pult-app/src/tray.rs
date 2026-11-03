//! The icon in the notification area: the window hides there while the route keeps playing.
//!
//! A click on the icon or "Show pult" brings the window back; "Quit pult" ends the route and
//! the program; "Start with Windows" turns the start into the tray on signing in on or off. Closing the window only hides it while a desk is open, so nothing stops
//! playing by accident; with nothing open, closing quits as before.

use std::sync::mpsc::Sender;

use eframe::egui::{Align, Button, Context, IconData, Id, Layout, Modal, RichText, Ui, vec2};
use eframe::icon_data::from_png_bytes;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::theme::Theme;

const SHOW: &str = "show";
const QUIT: &str = "quit";
const AUTOSTART: &str = "autostart";

/// What the tray, or a second start, asks the window for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Show,
    Quit,
    /// Turn the start with Windows over (from the check in the menu).
    Autostart,
}

/// The tray icon.
pub struct Tray {
    icon: TrayIcon,
    autostart: CheckMenuItem,
}

impl std::fmt::Debug for Tray {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tray").finish_non_exhaustive()
    }
}

impl Tray {
    /// Puts the icon into the notification area, with the start with Windows checked as
    /// `autostart` says; what it is asked goes to `commands` and repaints `ctx`, also while the
    /// window is hidden. Call it once, on the thread of the window.
    ///
    /// # Errors
    ///
    /// When the shell has no notification area or refuses the icon.
    pub fn new(
        ctx: &Context,
        tooltip: &str,
        autostart: bool,
        commands: &Sender<Command>,
    ) -> Result<Self, String> {
        let menu = Menu::new();
        let show = MenuItem::with_id(SHOW, "Show pult", true, None);
        let start = CheckMenuItem::with_id(AUTOSTART, "Start with Windows", true, autostart, None);
        let quit = MenuItem::with_id(QUIT, "Quit pult", true, None);
        let line = PredefinedMenuItem::separator();
        menu.append_items(&[&show, &start, &line, &quit])
            .map_err(|e| e.to_string())?;
        let (to_window, repaint) = (commands.clone(), ctx.clone());
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let command = match event.id.as_ref() {
                SHOW => Command::Show,
                QUIT => Command::Quit,
                AUTOSTART => Command::Autostart,
                _ => return,
            };
            let _ = to_window.send(command);
            repaint.request_repaint();
        }));
        let (send, repaint) = (commands.clone(), ctx.clone());
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = send.send(Command::Show);
                repaint.request_repaint();
            }
        }));

        let pixels = decode(TRAY_PNG)?;
        let image =
            Icon::from_rgba(pixels.rgba, pixels.width, pixels.height).map_err(|e| e.to_string())?;
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip(tooltip)
            .with_icon(image)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            icon,
            autostart: start,
        })
    }

    /// Sets the check of "Start with Windows" to what holds now (the menu turns it over on a
    /// click, before pult knows whether that worked).
    pub fn set_autostart(&self, on: bool) {
        self.autostart.set_checked(on);
    }

    /// Sets the text shown over the icon (the window title).
    pub fn set_tooltip(&self, text: &str) {
        let _ = self.icon.set_tooltip(Some(text));
    }
}

/// The icon in the notification area, from `assets/icons` of this repository (rule APP-11).
const TRAY_PNG: &[u8] = include_bytes!("../../../assets/icons/pult/png/pult-32.png");
/// The icon of the window and the taskbar, from the same place.
const WINDOW_PNG: &[u8] = include_bytes!("../../../assets/icons/pult/png/pult-64.png");

fn decode(png: &[u8]) -> Result<IconData, String> {
    from_png_bytes(png).map_err(|e| e.to_string())
}

/// The icon of the window.
///
/// # Errors
///
/// When the file that is built in is no valid PNG.
pub fn window_icon() -> Result<IconData, String> {
    decode(WINDOW_PNG)
}

/// What closing the window does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Close {
    /// Ends the program.
    Quit,
    /// Hides the window in the tray; the route keeps playing.
    Hide,
    /// Asks first, the first time: the user may not know pult keeps playing.
    Ask,
}

/// What closing does with a `tray` or not, a `desk` open or not, and whether the user was told
/// about the tray already (`told`). Quitting from the tray menu always quits.
#[must_use]
pub fn on_close(tray: bool, desk: bool, told: bool) -> Close {
    match (tray && desk, told) {
        (false, _) => Close::Quit,
        (true, false) => Close::Ask,
        (true, true) => Close::Hide,
    }
}

/// The answer to the hint on the first close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    ToTray,
    Quit,
    /// Neither: the window stays, the hint comes again on the next close.
    Cancel,
}

/// The hint on the first close: pult keeps playing in the tray. `None` while it waits;
/// Escape or a click beside it cancels.
pub fn hint(ui: &mut Ui) -> Option<Answer> {
    let theme = Theme::current();
    let p = &theme.palette;
    let mut answer = None;
    let modal = Modal::new(Id::unique("tray-hint")).show(ui.ctx(), |ui| {
        ui.set_width(380.0);
        ui.spacing_mut().item_spacing.y = 10.0;
        ui.label(
            RichText::new("pult keeps playing in the tray")
                .size(16.0)
                .color(p.text),
        );
        ui.label(
            RichText::new(
                "Closing the window does not stop the route. Click the icon in the \
                 notification area to bring the window back, or quit pult from its menu.",
            )
            .size(12.5)
            .color(p.text_dim),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let size = vec2(110.0, 28.0);
            if ui
                .add(Button::new(RichText::new("To the tray").color(p.text)).min_size(size))
                .clicked()
            {
                answer = Some(Answer::ToTray);
            }
            if ui
                .add(Button::new(RichText::new("Quit pult").color(p.text)).min_size(size))
                .clicked()
            {
                answer = Some(Answer::Quit);
            }
            if ui
                .add(Button::new(RichText::new("Cancel").color(p.text_dim)).min_size(size))
                .clicked()
            {
                answer = Some(Answer::Cancel);
            }
        });
    });
    if answer.is_none() && modal.should_close() {
        answer = Some(Answer::Cancel);
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_hides_only_while_something_plays() {
        assert_eq!(on_close(false, true, true), Close::Quit);
        assert_eq!(on_close(true, false, false), Close::Quit);
        assert_eq!(on_close(true, true, false), Close::Ask);
        assert_eq!(on_close(true, true, true), Close::Hide);
    }

    #[test]
    fn the_icons_are_full_squares_of_rgba_in_their_size() {
        for (png, size) in [(TRAY_PNG, 32), (WINDOW_PNG, 64)] {
            let icon = decode(png).unwrap();
            assert_eq!((icon.width, icon.height), (size, size));
            assert_eq!(icon.rgba.len(), (size * size * 4) as usize);
            assert_eq!(icon.rgba[3], 0, "the corners are round");
            let middle = ((size / 2 * size + size / 2) * 4 + 3) as usize;
            assert_eq!(icon.rgba[middle], 255, "the tile is opaque");
        }
    }

    #[test]
    fn a_broken_icon_is_an_error() {
        assert!(decode(b"no png").is_err());
    }
}
