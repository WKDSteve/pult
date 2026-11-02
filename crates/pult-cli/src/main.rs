//! `pult`: lists audio devices and runs a route from the command line.

mod args;

use std::fmt::Write as _;
use std::io::{self, BufRead, IsTerminal, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use args::{Command, ToneSpec, USAGE};
use pult_core::linear_to_db;
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
        Command::Run(spec) => pult_engine::start(&spec)
            .map_err(|e| e.to_string())
            .and_then(|running| run(&running)),
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

fn db(linear: f32) -> String {
    let value = linear_to_db(linear);
    if value < -90.0 {
        "  -inf".to_owned()
    } else {
        format!("{value:6.1}")
    }
}

/// Shows levels until Enter is pressed or a device fails.
fn run(running: &Running) -> Result<(), String> {
    println!(
        "Routing to {} at {} Hz; press Enter to stop.",
        running.sink, running.sample_rate
    );
    for (i, input) in running.inputs.iter().enumerate() {
        println!("  [{}] {}", i + 1, input.name);
    }
    wait_for_enter(|| running.fault(), Some(&|| status_line(running)))
}

/// Keeps the tone playing until Enter is pressed or the device fails.
fn tone(playing: &Tone, spec: &ToneSpec) -> Result<(), String> {
    println!(
        "Playing {} Hz at {} dB on {} ({} Hz); press Enter to stop.",
        spec.frequency, spec.gain_db, playing.sink, playing.sample_rate
    );
    wait_for_enter(|| playing.fault(), None)
}

/// Blocks until a line arrives on stdin or `fault` reports an error, refreshing the
/// `status` line meanwhile when stdout is a terminal and printing it once at the end.
fn wait_for_enter(
    fault: impl Fn() -> Option<String>,
    status: Option<&dyn Fn() -> String>,
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
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(STATUS_INTERVAL);
        if let Some(fault) = fault() {
            println!();
            return Err(format!("stopped, {fault}"));
        }
        if let (true, Some(status)) = (live, status) {
            // A line wider than the terminal wraps, and `\r` then returns only to the start of
            // its last row: every refresh would stack up. Fit it to the current width instead.
            let width = terminal_size::terminal_size().map_or(80, |(w, _)| usize::from(w.0));
            print!("\r{}", fit(&status(), width));
            let _ = io::stdout().flush();
        }
    }
    if let Some(status) = status {
        println!("\n{}", status());
    }
    Ok(())
}

fn status_line(running: &Running) -> String {
    let mut line = String::new();
    // Writing into a String cannot fail. Per input: level, dropouts, frames lost at capture
    // (the ring was full), and the drift fixes as frames dropped, frames repeated and hard
    // resyncs.
    for (i, input) in running.inputs.iter().enumerate() {
        let stats = &input.handle.stats;
        let _ = write!(
            line,
            "[{}] {} dB out {} lost {} fix -{}/+{}/{} ",
            i + 1,
            db(stats.peak.take()),
            stats.underruns.get(),
            stats.dropped_at_capture.get(),
            stats.drops.get(),
            stats.repeats.get(),
            stats.resyncs.get()
        );
    }
    let _ = write!(
        line,
        "| mix {} dB clipped {} | glitches {}",
        db(running.master.peak.take()),
        running.master.clipped.get(),
        running.xruns()
    );
    line.trim_end().to_owned()
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
