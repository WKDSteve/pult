//! `pult-app`: the console window over a running route (`pult-app studio.toml`) or the demo
//! desk (`pult-app --demo`). Without an argument it opens the profile opened last again
//! (`settings`); `--tray` does so with the window in the tray, which is how Windows starts it.
//!
//! A release build is a window program: no console window opens with it. Text for the
//! command line goes to the terminal it was started from (`ffi`).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![deny(unsafe_code)]

#[allow(unsafe_code)]
mod ffi;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{self, CentralPanel, Frame, ViewportCommand};
use pult_app::console::{self, ConsoleState};
use pult_app::settings::{self, Settings, Stored};
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
    /// The control interface, once a real route plays; never for the demo.
    hub: Option<(pult_hub::Control, pult_hub::Pump)>,
    /// The MIDI controller, which runs with the hub: one owner per machine.
    midi: Option<pult_midi::Midi>,
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
    /// What the window keeps between runs: the profile opened last, the start with Windows,
    /// and whether the user saw the hint on the first close.
    settings: Stored,
    /// Problems with the settings, for the log once a desk is open.
    notes: Vec<String>,
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
    fn new(start: &Start, ctx: &egui::Context, settings: Stored, note: Option<String>) -> Self {
        let (send, commands) = mpsc::channel();
        listen_for_later_starts(ctx, &send);
        let autostart = settings.settings.autostart;
        let mut app = Self {
            profile: None,
            desk: None,
            hub: None,
            midi: None,
            opening: None,
            problem: None,
            state: ConsoleState::default(),
            clock: Instant::now(),
            title: String::new(),
            tray: Tray::new(ctx, "pult", autostart, &send).ok(),
            commands,
            window: Window::Shown,
            settings,
            notes: note.into_iter().collect(),
        };
        let last = app.settings.settings.last_profile.as_deref();
        if *start == Start::Demo {
            app.use_desk(Box::new(FakeDesk::new()));
        } else if let Some(path) = start::profile_to_open(start, last) {
            // Kept whole, so it opens again from wherever the next start runs.
            app.profile = Some(std::path::absolute(&path).unwrap_or(path));
            app.open_profile(ctx);
        }
        if *start == Start::Tray {
            // The window starts hidden (see `main`); it shows when there is no tray to come
            // back from or nothing to play.
            app.window = Window::Hidden;
            if app.tray.is_none() || app.profile.is_none() {
                app.show(ctx);
            }
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
    fn check_opening(&mut self, ctx: &egui::Context) {
        let Some(result) = self.opening.as_ref().and_then(Opening::poll) else {
            return;
        };
        self.opening = None;
        match result {
            Ok(desk) => {
                self.use_desk(Box::new(desk));
                if self.hub.is_none() {
                    match pult_hub::Control::start() {
                        Ok(hub) => {
                            self.hub = Some(hub);
                            self.midi = Some(pult_midi::Midi::start());
                        }
                        Err(why) => self.notes.push(format!("no control interface: {why}")),
                    }
                }
                let profile = self.profile.clone();
                self.change_settings(|s| s.last_profile = profile);
            }
            Err(problem) => {
                self.problem = Some(problem);
                // From the tray, nobody would see why nothing plays.
                if self.window == Window::Hidden {
                    self.show(ctx);
                }
            }
        }
    }

    /// Changes the settings and keeps them; a problem goes to the log.
    fn change_settings(&mut self, edit: impl FnOnce(&mut Settings)) {
        if let Err(problem) = self.settings.change(edit) {
            self.notes.push(problem);
        }
    }

    /// Turns the start with Windows over: the Run value first, then the settings, and the
    /// check in the tray shows what holds.
    fn turn_autostart(&mut self) {
        let on = !self.settings.settings.autostart;
        match set_autostart(on) {
            Ok(()) => self.change_settings(|s| s.autostart = on),
            Err(problem) => self.notes.push(problem),
        }
        if let Some(tray) = &self.tray {
            tray.set_autostart(self.settings.settings.autostart);
        }
    }

    fn use_desk(&mut self, desk: Box<dyn Desk>) {
        self.desk = Some(desk);
        self.problem = None;
        self.state = ConsoleState::default();
    }

    fn show(&mut self, ctx: &egui::Context) {
        self.window = Window::Shown;
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    fn hide(&mut self, ctx: &egui::Context) {
        self.window = Window::Hidden;
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.window = Window::Quitting;
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    /// Ends the program when a client of the control interface asked for that.
    fn quit_if_asked(&mut self, ctx: &egui::Context) {
        let asked = self.hub.as_ref().is_some_and(|(_, pump)| pump.stop_asked());
        if asked && self.window != Window::Quitting {
            self.quit(ctx);
        }
    }

    /// Does what the tray asked for and what closing the window means.
    fn tray_and_close(&mut self, ctx: &egui::Context) {
        while let Ok(command) = self.commands.try_recv() {
            match command {
                Command::Show => self.show(ctx),
                Command::Quit => self.quit(ctx),
                Command::Autostart => self.turn_autostart(),
            }
        }
        if !ctx.input(|i| i.viewport().close_requested()) || self.window == Window::Quitting {
            return;
        }
        let told = self.settings.settings.told_about_tray;
        match tray::on_close(self.tray.is_some(), self.desk.is_some(), told) {
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
    /// What happens whether or not the window shows: eframe calls only this while the window
    /// is hidden, so the tray, later starts and the opening profile are handled here.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let title = self.wanted_title();
        if title != self.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            if let Some(tray) = &self.tray {
                tray.set_tooltip(&title);
            }
            self.title = title;
        }
        self.tray_and_close(ctx);
        self.check_opening(ctx);
        if self.window == Window::Hidden {
            // Still asked for views: the engine looks for lost devices then, and the events
            // view keeps what happened meanwhile.
            if let Some(desk) = &mut self.desk {
                let mut view = desk.view();
                self.state.take(&view);
                if let (Some((_, pump)), Some(midi)) = (&mut self.hub, &mut self.midi) {
                    for line in pult_midi::serve(pump, midi, desk.as_mut(), &mut view) {
                        self.state.remote(view.elapsed, &line);
                    }
                }
                for note in self.notes.drain(..) {
                    self.state.settings_problem(view.elapsed, &note);
                }
            }
            self.quit_if_asked(ctx);
            ctx.request_repaint_after(FRAME);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.window == Window::Hidden {
            // The last pass before the window goes; `logic` takes over.
            return;
        }
        if let Some(desk) = &mut self.desk {
            let now = self.clock.elapsed().as_secs_f64();
            let mut view = desk.view();
            for note in self.notes.drain(..) {
                self.state.settings_problem(view.elapsed, &note);
            }
            if let (Some((_, pump)), Some(midi)) = (&mut self.hub, &mut self.midi) {
                for line in pult_midi::serve(pump, midi, desk.as_mut(), &mut view) {
                    self.state.remote(view.elapsed, &line);
                }
            }
            for action in console::show(ui, &view, &mut self.state, now) {
                self.state.act(desk.as_mut(), view.elapsed, action);
            }
            if self.window == Window::Asking {
                match tray::hint(ui) {
                    Some(Answer::ToTray) => {
                        self.change_settings(|s| s.told_about_tray = true);
                        self.hide(&ctx);
                    }
                    Some(Answer::Quit) => self.quit(&ctx),
                    Some(Answer::Cancel) => self.window = Window::Shown,
                    None => {}
                }
            }
            self.quit_if_asked(&ctx);
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

/// The command Windows runs on signing in: this program, into the tray.
fn autostart_command(exe: &Path) -> String {
    format!("\"{}\" --tray", exe.display())
}

/// Lets Windows start pult on signing in, or no longer.
fn set_autostart(on: bool) -> Result<(), String> {
    let command = if on {
        let exe = std::env::current_exe().map_err(|e| format!("where is pult-app? {e}"))?;
        Some(autostart_command(&exe))
    } else {
        None
    };
    ffi::set_run(ffi::RUN_KEY, ffi::RUN_VALUE, command.as_deref())
        .map_err(|e| format!("could not change the start with Windows: {e}"))
}

/// Shows the window when another start of pult-app asks for it (see `main`).
fn listen_for_later_starts(ctx: &egui::Context, commands: &Sender<Command>) {
    let (send, ctx) = (commands.clone(), ctx.clone());
    ffi::listen_for_show(ffi::SHOW, move || {
        let _ = send.send(Command::Show);
        ctx.request_repaint();
    });
}

/// Whether a start that finds pult-app running brings its window up: all but the start with
/// Windows (`--tray`), which then has nothing to do and leaves the window where it is.
fn brings_up_the_window(start: &Start) -> bool {
    *start != Start::Tray
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
    if !matches!(
        start,
        Ok(Start::Profile(_) | Start::Demo | Start::Nothing | Start::Tray)
    ) {
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
        if !brings_up_the_window(&start) {
            return ExitCode::SUCCESS;
        }
        let shown = ffi::ask_to_show(ffi::SHOW, Duration::from_secs(3));
        let (note, code) = later_start(&start, shown);
        if let Some(note) = note {
            ffi::attach_parent_console();
            eprintln!("pult-app: {note}");
        }
        return code;
    };
    let (settings, note) = Stored::load(settings::default_path());
    let viewport = egui::ViewportBuilder::default()
        .with_title("pult")
        // eframe shows the window after its first painted frame, and paints none while it
        // is hidden: from the tray, nothing flashes up.
        .with_visible(start != Start::Tray)
        .with_inner_size([1280.0, 760.0])
        .with_min_inner_size([640.0, 560.0]);
    let options = eframe::NativeOptions {
        // Without its icon the window still opens, with the default of the system.
        viewport: match tray::window_icon() {
            Ok(icon) => viewport.with_icon(icon),
            Err(_) => viewport,
        },
        ..Default::default()
    };
    let result = eframe::run_native(
        "pult",
        options,
        Box::new(|cc| {
            Theme::current().apply(&cc.egui_ctx);
            Ok(Box::new(App::new(&start, &cc.egui_ctx, settings, note)))
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

    #[test]
    fn only_the_start_with_windows_leaves_a_running_window_alone() {
        assert!(!brings_up_the_window(&Start::Tray));
        assert!(brings_up_the_window(&Start::Nothing));
        assert!(brings_up_the_window(&Start::Demo));
        assert!(brings_up_the_window(&Start::Profile(PathBuf::from(
            "a.toml"
        ))));
    }

    #[test]
    fn windows_starts_this_program_into_the_tray() {
        assert_eq!(
            autostart_command(Path::new(r"C:\Program Files\pult\pult-app.exe")),
            r#""C:\Program Files\pult\pult-app.exe" --tray"#
        );
    }
}
