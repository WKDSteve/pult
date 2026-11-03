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
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{self, CentralPanel, Frame, ViewportCommand};
use pult_app::console::{self, ConsoleState};
use pult_app::start::{self, Choice, Opening, Start, USAGE};
use pult_app::theme::Theme;
use pult_app::tray::{self, Answer, Close, Command, Tray};
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
    /// The icon in the notification area; `None` when the shell refused it, then closing quits.
    tray: Option<Tray>,
    /// What the tray and later starts ask for.
    commands: Receiver<Command>,
    window: Window,
    /// The user saw the hint on the first close; later closes hide at once.
    told: bool,
}

/// Where the window stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Window {
    Shown,
    /// The hint on the first close is up.
    Asking,
    /// In the tray: nothing is drawn, the desk is still asked for views.
    Hidden,
    /// Quitting from the tray menu or the hint: the next close is not stopped.
    Quitting,
}

impl App {
    fn new(start: Start, ctx: &egui::Context) -> Self {
        let (send, commands) = mpsc::channel();
        listen_for_later_starts(ctx, &send);
        let mut app = Self {
            profile: None,
            desk: None,
            opening: None,
            problem: None,
            state: ConsoleState::default(),
            clock: Instant::now(),
            title: String::new(),
            tray: Tray::new(ctx, "pult", &send).ok(),
            commands,
            window: Window::Shown,
            told: false,
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

    fn hide(&mut self, ctx: &egui::Context) {
        self.window = Window::Hidden;
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.window = Window::Quitting;
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    /// Does what the tray asked for and what closing the window means.
    fn tray_and_close(&mut self, ctx: &egui::Context) {
        while let Ok(command) = self.commands.try_recv() {
            match command {
                Command::Show => {
                    self.window = Window::Shown;
                    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
                Command::Quit => self.quit(ctx),
            }
        }
        if !ctx.input(|i| i.viewport().close_requested()) || self.window == Window::Quitting {
            return;
        }
        match tray::on_close(self.tray.is_some(), self.desk.is_some(), self.told) {
            Close::Quit => {}
            Close::Hide => {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.hide(ctx);
            }
            Close::Ask => {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.window = Window::Asking;
            }
        }
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
            if let Some(tray) = &self.tray {
                tray.set_tooltip(&title);
            }
            self.title = title;
        }
        let ctx = ui.ctx().clone();
        self.tray_and_close(&ctx);
        self.check_opening();
        if self.window == Window::Hidden {
            // Still asked for views: the engine looks for lost devices then, and the events
            // view keeps what happened meanwhile.
            if let Some(desk) = &mut self.desk {
                let view = desk.view();
                self.state.take(&view);
            }
            ctx.request_repaint_after(FRAME);
            return;
        }
        if let Some(desk) = &mut self.desk {
            let now = self.clock.elapsed().as_secs_f64();
            let view = desk.view();
            for action in console::show(ui, &view, &mut self.state, now) {
                self.state.act(desk.as_mut(), view.elapsed, action);
            }
            if self.window == Window::Asking {
                match tray::hint(ui) {
                    Some(Answer::ToTray) => {
                        self.told = true;
                        self.hide(&ctx);
                    }
                    Some(Answer::Quit) => self.quit(&ctx),
                    Some(Answer::Cancel) => self.window = Window::Shown,
                    None => {}
                }
            }
            ctx.request_repaint_after(FRAME);
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

/// Shows the window when another start of pult-app asks for it (see `main`).
fn listen_for_later_starts(ctx: &egui::Context, commands: &Sender<Command>) {
    let (send, ctx) = (commands.clone(), ctx.clone());
    ffi::listen_for_show(ffi::SHOW, move || {
        let _ = send.send(Command::Show);
        ctx.request_repaint();
    });
}

/// What a start says and how it ends when another pult-app runs already, which `shown` its
/// window or did not answer. Quiet when it only meant to bring the window up.
fn later_start(start: &Start, shown: bool) -> (Option<&'static str>, ExitCode) {
    match (shown, start) {
        (true, Start::Nothing) => (None, ExitCode::SUCCESS),
        (true, _) => (
            Some("already running; showed its window and left this start out"),
            ExitCode::SUCCESS,
        ),
        (false, _) => (
            Some("already running, but its window did not answer"),
            ExitCode::FAILURE,
        ),
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
    // One instance per session: a second start shows the running window and ends, instead of
    // a second route on the same devices.
    let Some(_instance) = ffi::claim(ffi::INSTANCE) else {
        let shown = ffi::ask_to_show(ffi::SHOW, Duration::from_secs(3));
        let (note, code) = later_start(&start, shown);
        if let Some(note) = note {
            ffi::attach_parent_console();
            eprintln!("pult-app: {note}");
        }
        return code;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_later_start_says_what_happened() {
        assert_eq!(
            later_start(&Start::Nothing, true),
            (None, ExitCode::SUCCESS)
        );
        let (note, code) = later_start(&Start::Demo, true);
        assert!(note.unwrap().contains("showed its window"));
        assert_eq!(code, ExitCode::SUCCESS);
        let (note, code) = later_start(&Start::Nothing, false);
        assert!(note.unwrap().contains("did not answer"));
        assert_eq!(code, ExitCode::FAILURE);
    }
}
