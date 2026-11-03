//! `pult-app`: the console window. Until the engine side of `pult_core::view` exists, it
//! shows the in-memory `FakeDesk`.

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use eframe::egui;
use pult_app::console::{self, ConsoleState};
use pult_app::theme::Theme;
use pult_core::view::{Desk, FakeDesk};

/// The desk is asked for a view about 30 times a second, as the contract expects.
const FRAME: Duration = Duration::from_millis(33);

struct App {
    desk: Box<dyn Desk>,
    state: ConsoleState,
    start: Instant,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = self.start.elapsed().as_secs_f64();
        let view = self.desk.view();
        for action in console::show(ui, &view, &mut self.state, now) {
            if let Err(error) = self.desk.act(action) {
                eprintln!("pult-app: {error}");
            }
        }
        ui.ctx().request_repaint_after(FRAME);
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("pult")
            .with_inner_size([1280.0, 760.0])
            .with_min_inner_size([640.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "pult",
        options,
        Box::new(|cc| {
            Theme::current().apply(&cc.egui_ctx);
            Ok(Box::new(App {
                desk: Box::new(FakeDesk::new()),
                state: ConsoleState::default(),
                start: Instant::now(),
            }))
        }),
    )
}
