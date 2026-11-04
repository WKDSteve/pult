//! `pult`: lists audio devices and runs a route from the command line.

#![forbid(unsafe_code)]

mod args;
mod console;
mod monitor;
mod remote;

use std::io::{self, BufRead};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use args::{Command, Routing, RunSpec, ToneSpec, USAGE};
use console::{Console, Input};
use monitor::{LOG_INTERVAL, Log, Snapshot};
use pult_core::{Desk, Dropouts, control};
use pult_engine::{Devices, EngineDesk, Running, Tone};
use pult_hub::{Control, Pump};

// Debug builds abort on any allocation inside an audio callback (see pult-engine).
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

/// How often the status line is refreshed.
const STATUS_INTERVAL: Duration = Duration::from_millis(200);

fn main() -> ExitCode {
    let command = match args::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(err) => {
            eprintln!("pult: {err}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let result = match command {
        Command::Help => {
            println!("{USAGE}");
            Ok(())
        }
        Command::Version => {
            println!("pult {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Devices => pult_engine::list_devices()
            .map(|d| print_devices(&d))
            .map_err(|e| e.to_string()),
        Command::Programs => pult_engine::program_names()
            .map(|names| {
                for name in names {
                    println!("{name}");
                }
                println!(
                    "(capture one with --from app:<NAME>, or process = \"<NAME>\" in a profile)"
                );
            })
            .map_err(|e| e.to_string()),
        Command::Run(spec) => start_run(&spec),
        Command::Remote(request) => return remote::run(&request),
        Command::Tone(spec) => pult_engine::play_tone(&spec.sink, spec.frequency, spec.gain_db)
            .map_err(|e| e.to_string())
            .and_then(|playing| tone(&playing, &spec)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("pult: {err}");
            ExitCode::FAILURE
        }
    }
}

fn print_devices(devices: &Devices) {
    for (title, list) in [("Inputs", &devices.inputs), ("Outputs", &devices.outputs)] {
        println!("{title}:");
        if list.is_empty() {
            println!("  (none)");
        }
        for d in list {
            let mark = if d.is_default { '*' } else { ' ' };
            println!(
                "{mark} {}  [{} ch, {} Hz, {}]",
                d.name, d.channels, d.sample_rate, d.sample_format
            );
        }
    }
    println!("(* = system default)");
}

/// Reads the profile and opens the log (before any device, so a bad file costs nothing),
/// then runs the route.
fn start_run(spec: &RunSpec) -> Result<(), String> {
    let route = match &spec.routing {
        Routing::Inline(route) => route.clone(),
        Routing::Profile(path) => pult_engine::profile::load(path).map_err(|e| e.to_string())?,
    };
    let bus_names: Vec<String> = route.buses.iter().map(|b| b.name.clone()).collect();
    let log = match &spec.log {
        Some(path) => Some(
            Log::create(path, route.inputs.len(), &bus_names)
                .map_err(|e| format!("cannot write the log {}: {e}", path.display()))?,
        ),
        None => None,
    };
    let running = pult_engine::start(&route).map_err(|e| e.to_string())?;
    let presence = pult_engine::Presence::announce();
    if let Some(note) = presence.note() {
        println!("{note}");
    }
    let profile = match &spec.routing {
        Routing::Inline(_) => None,
        Routing::Profile(path) => Some(path.clone()),
    };
    let mut desk = EngineDesk::new(running, profile);
    // The route plays with or without it.
    let hub = match Control::start() {
        Ok((control, pump)) => {
            println!("Control interface on port {}.", control.port());
            Some((control, pump))
        }
        Err(why) => {
            println!("note: no control interface: {why}");
            None
        }
    };
    let (_control, pump) = hub.unzip();
    run(&mut desk, log, bus_names, pump);
    Ok(())
}

/// Shows levels and takes commands until an empty line, `quit` or Ctrl+C. A device that
/// disappears is reported and taken back when it returns; the run goes on.
fn run(desk: &mut EngineDesk, log: Option<Log>, bus_names: Vec<String>, pump: Option<Pump>) {
    println!("Routing; type help for commands, Enter on an empty line stops.");
    let running = desk.running();
    for bus in &running.buses {
        println!("  {} -> {} at {} Hz", bus.name, bus.device, bus.sample_rate);
    }
    for (i, input) in running.inputs.iter().enumerate() {
        let buses: Vec<&str> = input
            .routes
            .iter()
            .filter(|r| r.is_routed())
            .map(|r| running.buses[r.bus].name.as_str())
            .collect();
        println!(
            "  [{}] {} at {} Hz -> {}",
            i + 1,
            input.name,
            input.sample_rate,
            buses.join(", ")
        );
    }
    let mut recorder = Recorder::new(log, bus_names, pump);
    let mut console = Console::open();
    let mut next = Instant::now();
    // Nothing but the user ends a run: lost devices are the supervisor's business.
    loop {
        match console.wait(next) {
            Some(Input::Interrupt) => break,
            Some(Input::Line(line)) => {
                let running = desk.running_mut();
                let parsed = control::parse(&line, &running.input_names(), &running.bus_names());
                let answer = match parsed {
                    Ok(control::Command::Stop) => break,
                    Ok(command) => running.apply(&command),
                    Err(err) => Err(err.to_string()),
                };
                let lines = match answer {
                    Ok(lines) => lines.into_iter().map(|l| format!("  {l}")).collect(),
                    Err(err) => vec![format!("  not done: {err}")],
                };
                console.notes(&lines);
            }
            None => {
                let tick = recorder.tick(desk);
                console.notes(&tick.notes);
                console.status(tick.line);
                if tick.stop {
                    break;
                }
                next = Instant::now() + STATUS_INTERVAL;
            }
        }
    }
    console.close();
    recorder.finish(desk.running());
}

/// Takes the snapshots of a run and writes every [`LOG_INTERVAL`] a log row with the
/// highest peaks of that interval.
struct Recorder {
    start: Instant,
    log: Option<Log>,
    bus_names: Vec<String>,
    interval: Option<Snapshot>,
    next_row: Duration,
    dropouts: Dropouts,
    /// The way to the clients of the control interface, if it runs.
    pump: Option<Pump>,
}

impl Recorder {
    fn new(log: Option<Log>, bus_names: Vec<String>, pump: Option<Pump>) -> Self {
        Self {
            start: Instant::now(),
            log,
            bus_names,
            interval: None,
            next_row: LOG_INTERVAL,
            dropouts: Dropouts::default(),
            pump,
        }
    }

    /// Takes a snapshot, checks the devices, serves the clients, logs when an interval is
    /// full, and returns what changed (one line each) and the status line.
    fn tick(&mut self, desk: &mut EngineDesk) -> Tick {
        let elapsed = self.start.elapsed();
        let snap = Snapshot::take(desk.running(), elapsed, &mut self.dropouts);
        // The view looks for lost devices; its peaks are what the snapshot left.
        let mut view = desk.view();
        let mut notes: Vec<String> = view
            .events
            .iter()
            .map(|event| format!("{} {}", monitor::clock(elapsed), event.text))
            .collect();
        if let Some(pump) = &mut self.pump {
            monitor::with_peaks_of(&mut view, &snap);
            notes.extend(
                pump.pump(desk, &view)
                    .into_iter()
                    .map(|line| format!("{} remote: {line}", monitor::clock(elapsed))),
            );
        }
        let merged = self
            .interval
            .as_ref()
            .map_or_else(|| snap.clone(), |earlier| earlier.merge(&snap));
        if snap.elapsed >= self.next_row {
            notes.extend(self.write(&merged));
            self.interval = None;
            while self.next_row <= snap.elapsed {
                self.next_row += LOG_INTERVAL;
            }
        } else {
            self.interval = Some(merged);
        }
        Tick {
            notes,
            line: monitor::status_line(&snap, &self.bus_names),
            stop: self.pump.as_ref().is_some_and(Pump::stop_asked),
        }
    }

    /// Writes the rest of the last interval and prints the summary of the run.
    fn finish(&mut self, running: &Running) {
        let snap = Snapshot::take(running, self.start.elapsed(), &mut self.dropouts);
        println!("{}", monitor::summary(&snap));
        let last = self
            .interval
            .take()
            .map_or_else(|| snap.clone(), |earlier| earlier.merge(&snap));
        if let Some(note) = self.write(&last) {
            eprintln!("pult: {note}");
        }
    }

    /// A failed write ends logging, and its note says so once; the route keeps running.
    fn write(&mut self, snap: &Snapshot) -> Option<String> {
        if let Some(log) = &mut self.log
            && let Err(err) = log.row(snap)
        {
            self.log = None;
            return Some(format!("log stopped, {err}"));
        }
        None
    }
}

/// What one refresh shows: lines that stay (devices lost or back) and the live status line.
struct Tick {
    notes: Vec<String>,
    line: String,
    /// A client asked for the route to end.
    stop: bool,
}

/// Keeps the tone playing until Enter is pressed or the device fails.
fn tone(playing: &Tone, spec: &ToneSpec) -> Result<(), String> {
    println!(
        "Playing {} Hz at {} dB on {} ({} Hz); press Enter to stop.",
        spec.frequency, spec.gain_db, playing.sink, playing.sample_rate
    );
    wait_for_enter(|| playing.fault())
}

/// Blocks until a line arrives on stdin or `fault` reports an error.
fn wait_for_enter(fault: impl Fn() -> Option<String>) -> Result<(), String> {
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        // Only a line stops; a closed stdin (started without a console) does not.
        thread::spawn(move || {
            if matches!(io::stdin().lock().read_line(&mut String::new()), Ok(n) if n > 0) {
                stop.store(true, Ordering::Relaxed);
            }
        });
    }
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(STATUS_INTERVAL);
        if let Some(fault) = fault() {
            return Err(format!("stopped, {fault}"));
        }
    }
    Ok(())
}
