//! `pult`: lists audio devices and runs a route from the command line.

#![forbid(unsafe_code)]

mod args;
mod console;
mod monitor;

use std::io::{self, BufRead};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use args::{Command, Routing, RunSpec, ToneSpec, USAGE};
use console::{Console, Input};
use monitor::{LOG_INTERVAL, Log, Snapshot};
use pult_core::{Dropouts, control};
use pult_engine::{Devices, Running, Tone};

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
    let mut running = pult_engine::start(&route).map_err(|e| e.to_string())?;
    let presence = pult_engine::Presence::announce();
    if let Some(note) = presence.note() {
        println!("{note}");
    }
    run(&mut running, log, bus_names);
    Ok(())
}

/// Shows levels and takes commands until an empty line, `quit` or Ctrl+C. A device that
/// disappears is reported and taken back when it returns; the run goes on.
fn run(running: &mut Running, log: Option<Log>, bus_names: Vec<String>) {
    println!("Routing; type help for commands, Enter on an empty line stops.");
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
    let mut recorder = Recorder::new(log, bus_names);
    let mut console = Console::open();
    let mut next = Instant::now();
    // Nothing but the user ends a run: lost devices are the supervisor's business.
    loop {
        match console.wait(next) {
            Some(Input::Interrupt) => break,
            Some(Input::Line(line)) => {
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
                let tick = recorder.tick(running);
                console.notes(&tick.notes);
                console.status(tick.line);
                next = Instant::now() + STATUS_INTERVAL;
            }
        }
    }
    console.close();
    recorder.finish(running);
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
}

impl Recorder {
    fn new(log: Option<Log>, bus_names: Vec<String>) -> Self {
        Self {
            start: Instant::now(),
            log,
            bus_names,
            interval: None,
            next_row: LOG_INTERVAL,
            dropouts: Dropouts::default(),
        }
    }

    /// Checks the devices, takes a snapshot, logs when an interval is full, and returns
    /// what changed (one line each) and the status line.
    fn tick(&mut self, running: &mut Running) -> Tick {
        let elapsed = self.start.elapsed();
        let mut notes: Vec<String> = running
            .supervise()
            .iter()
            .map(|change| format!("{} {change}", monitor::clock(elapsed)))
            .collect();
        let snap = Snapshot::take(running, elapsed, &mut self.dropouts);
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
