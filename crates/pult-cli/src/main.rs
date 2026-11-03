//! `pult`: lists audio devices and runs a route from the command line.

#![forbid(unsafe_code)]

mod args;
mod monitor;

use std::io::{self, BufRead, IsTerminal, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use args::{Command, Routing, RunSpec, ToneSpec, USAGE};
use monitor::{LOG_INTERVAL, Log, Snapshot};
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
    run(&mut running, log, bus_names);
    Ok(())
}

/// Shows levels until Enter is pressed. A device that disappears is reported and taken
/// back when it returns; the run goes on.
fn run(running: &mut Running, log: Option<Log>, bus_names: Vec<String>) {
    println!(
        "Routing at {} Hz; press Enter to stop.",
        running.sample_rate
    );
    for bus in &running.buses {
        println!("  {} -> {}", bus.name, bus.device);
    }
    for (i, input) in running.inputs.iter().enumerate() {
        let buses: Vec<&str> = input
            .routes
            .iter()
            .map(|r| running.buses[r.bus].name.as_str())
            .collect();
        println!("  [{}] {} -> {}", i + 1, input.name, buses.join(", "));
    }
    let mut recorder = Recorder::new(log, bus_names);
    // Nothing ends a run but Enter: lost devices are the supervisor's business.
    let _ = wait_for_enter(|| None, Some(&mut || recorder.tick(running)));
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
}

impl Recorder {
    fn new(log: Option<Log>, bus_names: Vec<String>) -> Self {
        Self {
            start: Instant::now(),
            log,
            bus_names,
            interval: None,
            next_row: LOG_INTERVAL,
        }
    }

    /// Checks the devices, takes a snapshot, logs when an interval is full, and returns
    /// what changed (one line each) and the status line.
    fn tick(&mut self, running: &mut Running) -> Tick {
        let elapsed = self.start.elapsed();
        let notes = running
            .supervise()
            .iter()
            .map(|change| format!("{} {change}", monitor::clock(elapsed)))
            .collect();
        let snap = Snapshot::take(running, elapsed);
        let merged = self
            .interval
            .as_ref()
            .map_or_else(|| snap.clone(), |earlier| earlier.merge(&snap));
        if snap.elapsed >= self.next_row {
            self.write(&merged);
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

    /// Writes the rest of the last interval.
    fn finish(&mut self, running: &Running) {
        let snap = Snapshot::take(running, self.start.elapsed());
        let last = self
            .interval
            .take()
            .map_or_else(|| snap.clone(), |earlier| earlier.merge(&snap));
        self.write(&last);
    }

    /// A failed write is reported once and ends logging; the route keeps running.
    fn write(&mut self, snap: &Snapshot) {
        if let Some(log) = &mut self.log
            && let Err(err) = log.row(snap)
        {
            eprintln!("\npult: log stopped, {err}");
            self.log = None;
        }
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
    wait_for_enter(|| playing.fault(), None)
}

/// Blocks until a line arrives on stdin or `fault` reports an error. `status` runs every
/// interval; its notes are printed as lines of their own, its status line is shown live
/// when stdout is a terminal, and the last one is printed at the end.
fn wait_for_enter(
    fault: impl Fn() -> Option<String>,
    mut status: Option<&mut dyn FnMut() -> Tick>,
) -> Result<(), String> {
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        // Only a line stops the route; a closed stdin (started without a console) does not.
        thread::spawn(move || {
            if matches!(io::stdin().lock().read_line(&mut String::new()), Ok(n) if n > 0) {
                stop.store(true, Ordering::Relaxed);
            }
        });
    }
    let live = io::stdout().is_terminal();
    let mut last = String::new();
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(STATUS_INTERVAL);
        if let Some(fault) = fault() {
            println!("\n{last}");
            return Err(format!("stopped, {fault}"));
        }
        if let Some(status) = status.as_mut() {
            let tick = status();
            last = tick.line;
            // A line wider than the terminal wraps, and `\r` then returns only to the start
            // of its last row: every refresh would stack up. Fit it to the width instead.
            let width = terminal_size::terminal_size().map_or(80, |(w, _)| usize::from(w.0));
            for note in &tick.notes {
                if live {
                    // Padded to cover the status line it replaces; a long note may wrap.
                    println!("\r{note:<columns$}", columns = width.saturating_sub(1));
                } else {
                    println!("{note}");
                }
            }
            if live {
                print!("\r{}", fit(&last, width));
                let _ = io::stdout().flush();
            }
        }
    }
    if status.is_some() {
        println!("\n{last}");
    }
    Ok(())
}

/// `line` cut or padded to one column less than `width`, so that it never wraps and fully
/// covers the previous refresh.
fn fit(line: &str, width: usize) -> String {
    let columns = width.saturating_sub(1);
    let mut fitted: String = line.chars().take(columns).collect();
    let used = fitted.chars().count();
    fitted.extend(std::iter::repeat_n(' ', columns - used));
    fitted
}

#[cfg(test)]
mod tests {
    use super::fit;

    #[test]
    fn fit_cuts_long_lines_and_pads_short_ones() {
        assert_eq!(fit("abcdef", 4), "abc");
        assert_eq!(fit("ab", 5), "ab  ");
        assert_eq!(fit("äöü-x", 4), "äöü");
        assert_eq!(fit("abc", 0), "");
    }
}
