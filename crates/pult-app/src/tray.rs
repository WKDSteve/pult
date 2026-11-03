//! The icon in the notification area: the window hides there while the route keeps playing.
//!
//! A click on the icon or "Show pult" brings the window back; "Quit pult" ends the route and
//! the program. Closing the window only hides it while a desk is open, so nothing stops
//! playing by accident; with nothing open, closing quits as before.

use std::sync::mpsc::Sender;

use eframe::egui::{Align, Button, Context, Id, Layout, Modal, RichText, Ui, vec2};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::theme::Theme;

const SHOW: &str = "show";
const QUIT: &str = "quit";

/// What the tray, or a second start, asks the window for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Show,
    Quit,
}

/// The tray icon.
pub struct Tray {
    icon: TrayIcon,
}

impl std::fmt::Debug for Tray {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tray").finish_non_exhaustive()
    }
}

impl Tray {
    /// Puts the icon into the notification area; what it is asked goes to `commands` and
    /// repaints `ctx`, also while the window is hidden. Call it once, on the thread of the
    /// window.
    ///
    /// # Errors
    ///
    /// When the shell has no notification area or refuses the icon.
    pub fn new(ctx: &Context, tooltip: &str, commands: &Sender<Command>) -> Result<Self, String> {
        let menu = Menu::new();
        let show = MenuItem::with_id(SHOW, "Show pult", true, None);
        let quit = MenuItem::with_id(QUIT, "Quit pult", true, None);
        menu.append(&show).map_err(|e| e.to_string())?;
        menu.append(&quit).map_err(|e| e.to_string())?;
        let (to_window, repaint) = (commands.clone(), ctx.clone());
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let command = match event.id.as_ref() {
                SHOW => Command::Show,
                QUIT => Command::Quit,
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

        let (rgba, size) = icon_pixels();
        let image = Icon::from_rgba(rgba, size, size).map_err(|e| e.to_string())?;
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip(tooltip)
            .with_icon(image)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { icon })
    }

    /// Sets the text shown over the icon (the window title).
    pub fn set_tooltip(&self, text: &str) {
        let _ = self.icon.set_tooltip(Some(text));
    }
}

/// The icon: the lit square of the word mark on a dark tile, as RGBA rows.
#[must_use]
pub fn icon_pixels() -> (Vec<u8>, u32) {
    const SIZE: u32 = 32;
    let p = &Theme::current().palette;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            // A tile with its corners cut by one pixel, the square in its middle.
            let edge = |v: u32| v == 0 || v == SIZE - 1;
            let corner = edge(x) && edge(y);
            let lit = (9..23).contains(&x) && (9..23).contains(&y);
            let color = if lit { p.accent } else { p.surface };
            let alpha = if corner { 0 } else { 255 };
            rgba.extend_from_slice(&[color.r(), color.g(), color.b(), alpha]);
        }
    }
    (rgba, SIZE)
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
    fn the_icon_is_a_full_square_of_rgba() {
        let (rgba, size) = icon_pixels();
        assert_eq!(rgba.len(), (size * size * 4) as usize);
        let accent = Theme::current().palette.accent;
        let middle = ((size / 2 * size + size / 2) * 4) as usize;
        assert_eq!(
            &rgba[middle..middle + 4],
            &[accent.r(), accent.g(), accent.b(), 255]
        );
        assert_eq!(rgba[3], 0, "the corners are cut");
    }
}
