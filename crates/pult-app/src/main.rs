//! `pult-app`: the console window over a running route (`pult-app studio.toml`) or the demo
//! desk (`pult-app --demo`).
//!
//! A release build is a window program: no console window opens with it. Text for the
//! command line goes to the terminal it was started from (`ffi`).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![deny(unsafe_code)]

#[allow(unsafe_code)]
mod ffi;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use eframe::egui::{self, CentralPanel, Frame, ViewportCommand};
use pult_app::console::{self, ConsoleState};
use pult_app::start::{self, Choice, Opening, Start, USAGE};
use pult_app::theme::Theme;
use pult_core::view::{Desk, FakeDesk};
use pult_engine::EngineDesk;

// Debug builds abort on any allocation inside an audio callback, as the command line does
// (see pult-engine).
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

/// The desk is asked for a view about 30 times a second, as the contract expects; a view also
/// lets the engine look for lost devices.
const FRAME: Duration = Duration::from_millis(33);

struct App {
    /// The profile from the command line, opened again on "Try again".
    profile: Option<PathBuf>,
    desk: Option<Box<dyn Desk>>,
    /// The profile while it opens in the background.
    opening: Option<Opening<EngineDesk>>,
    /// Why the profile did not open; shown on the start screen.
    problem: Option<String>,
    state: ConsoleState,
    clock: Instant,
    title: String,
}

impl App {
    fn new(start: Start, ctx: &egui::Context) -> Self {
        let mut app = Self {
            profile: None,
            desk: None,
            opening: None,
            problem: None,
            state: ConsoleState::default(),
            clock: Instant::now(),
            title: String::new(),
        };
        match start {
            Start::Profile(path) => {
                app.profile = Some(path);
                app.open_profile(ctx);
            }
            Start::Demo => app.use_desk(Box::new(FakeDesk::new())),
            Start::Nothing | Start::Help | Start::Version => {}
        }
        app
    }

    /// Opens the profile in the background; the start screen says so meanwhile.
    fn open_profile(&mut self, ctx: &egui::Context) {
        let Some(path) = self.profile.clone() else {
            return;
        };
        self.problem = None;
        self.opening = Some(Opening::spawn(ctx, move || EngineDesk::open(&path)));
    }

    /// Takes the desk or the problem once the profile is open.
    fn check_opening(&mut self) {
        let Some(result) = self.opening.as_ref().and_then(Opening::poll) else {
            return;
        };
        self.opening = None;
        match result {
            Ok(desk) => self.use_desk(Box::new(desk)),
            Err(problem) => self.problem = Some(problem),
        }
    }

    fn use_desk(&mut self, desk: Box<dyn Desk>) {
        self.desk = Some(desk);
        self.problem = None;
        self.state = ConsoleState::default();
    }

    /// The window title: the profile's file name, or what is shown instead.
    fn wanted_title(&self) -> String {
        let name = match (&self.desk, &self.profile) {
            (Some(_), Some(path)) => path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into(),
            ),
            (Some(_), _) => "demo".to_owned(),
            (None, _) => return "pult".to_owned(),
        };
        format!("pult · {name}")
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let title = self.wanted_title();
        if title != self.title {
            ui.ctx()
                .send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        self.check_opening();
        if let Some(desk) = &mut self.desk {
            let now = self.clock.elapsed().as_secs_f64();
            let view = desk.view();
            for action in console::show(ui, &view, &mut self.state, now) {
                if let Err(reason) = desk.act(action) {
                    self.state.refused(view.elapsed, &reason);
                }
            }
            ui.ctx().request_repaint_after(FRAME);
            return;
        }
        let base = Theme::current().palette.base;
        let opening = self.opening.is_some();
        let choice = CentralPanel::default()
            .frame(Frame::new().fill(base))
            .show(ui, |ui| {
                if opening {
                    start::starting(ui, self.profile.as_deref());
                    None
                } else {
                    start::screen(ui, self.profile.as_deref(), self.problem.as_deref())
                }
            })
            .inner;
        match choice {
            Some(Choice::Retry) => self.open_profile(ui.ctx()),
            Some(Choice::Demo) => {
                self.profile = None;
                self.use_desk(Box::new(FakeDesk::new()));
            }
            None => {}
        }
    }
}

fn main() -> ExitCode {
    let start = start::parse(std::env::args().skip(1));
    if !matches!(start, Ok(Start::Profile(_) | Start::Demo | Start::Nothing)) {
        ffi::attach_parent_console();
    }
    let start = match start {
        Ok(Start::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(Start::Version) => {
            println!("pult-app {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Ok(start) => start,
        Err(err) => {
            eprintln!("pult-app: {err}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("pult")
            .with_inner_size([1280.0, 760.0])
            .with_min_inner_size([640.0, 560.0]),
        ..Default::default()
    };
    let result = eframe::run_native(
        "pult",
        options,
        Box::new(|cc| {
            Theme::current().apply(&cc.egui_ctx);
            Ok(Box::new(App::new(start, &cc.egui_ctx)))
        }),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            ffi::attach_parent_console();
            eprintln!("pult-app: {err}");
            ExitCode::FAILURE
        }
    }
}
