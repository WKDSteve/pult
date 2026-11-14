//! `pult-plughost`: the host process of the spike, and the measurement that starts it.
//!
//! ```text
//! pult-plughost bench [--frames 64,128,256] [--seconds 60] [--kill-at <s>] [--gain 0.5]
//! pult-plughost host --name <mapping> --channels <n> --frames <n> --blocks <n> [--state <bits>]
//! ```
//!
//! `bench` plays a tone through an insert whose other side is a second process of this program
//! (`host`), and prints misses, wake delays, CPU and clicks as a Markdown table; with `--kill-at`
//! it kills the host that many seconds in and reports how the insert took it. `host` is started
//! by `bench` and by the tests, never by hand.

use std::process::ExitCode;

const USAGE: &str = "usage:
  pult-plughost bench [--frames 64,128,256] [--seconds 60] [--kill-at <s>] [--gain 0.5]
  pult-plughost host --name <mapping> --channels <n> --frames <n> --blocks <n> [--state <bits>]";

/// The value behind the option `name` in `args`, parsed; `None` when the option is not there.
fn option<T: std::str::FromStr>(args: &[String], name: &str) -> Result<Option<T>, String> {
    let Some(at) = args.iter().position(|arg| arg == name) else {
        return Ok(None);
    };
    let value = args
        .get(at + 1)
        .ok_or_else(|| format!("{name} needs a value"))?;
    value
        .parse()
        .map(Some)
        .map_err(|_| format!("{name} does not take {value}"))
}

/// Refuses an option `args` has that is none of `known`.
fn only(args: &[String], known: &[&str]) -> Result<(), String> {
    match args
        .iter()
        .step_by(2)
        .find(|arg| !known.contains(&arg.as_str()))
    {
        Some(unknown) => Err(format!("unknown option {unknown}")),
        None => Ok(()),
    }
}

#[cfg(windows)]
mod run {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use pult_plughost::bench::{self, Plan};
    use pult_plughost::{Event, Geometry, Host, ProAudio, Region, now_ns};

    use super::{only, option};

    /// A host whose engine sent nothing for this long ends: the engine is gone.
    const ENGINE_GONE: Duration = Duration::from_secs(2);

    pub(super) fn host(args: &[String]) -> Result<(), String> {
        only(
            args,
            &["--name", "--channels", "--frames", "--blocks", "--state"],
        )?;
        let need =
            |name: &str| option::<usize>(args, name)?.ok_or_else(|| format!("host needs {name}"));
        let name: String = option(args, "--name")?.ok_or("host needs --name")?;
        let geometry = Geometry::new(need("--channels")?, need("--frames")?, need("--blocks")?)?;
        let state = option::<u32>(args, "--state")?.map(f32::from_bits);
        let region = Region::open(&name, geometry).map_err(|e| format!("{name}: {e}"))?;
        let event = Event::open(&bench::wake_name(&name)).map_err(|e| format!("{name}: {e}"))?;
        let mut host = Host::attach(Arc::new(region), geometry, state)?;
        let _realtime = ProAudio::join();
        let mut beat = host.engine_beat();
        let mut moved = Instant::now();
        loop {
            let _ = event.wait(Duration::from_millis(100));
            host.work(now_ns());
            if host.stop_requested() {
                return Ok(());
            }
            let now = host.engine_beat();
            if now != beat {
                beat = now;
                moved = Instant::now();
            } else if moved.elapsed() > ENGINE_GONE {
                return Ok(());
            }
        }
    }

    pub(super) fn bench(args: &[String]) -> Result<(), String> {
        only(args, &["--frames", "--seconds", "--kill-at", "--gain"])?;
        let frames: String = option(args, "--frames")?.unwrap_or_else(|| "64,128,256".to_owned());
        let frames: Vec<usize> = frames
            .split(',')
            .map(|size| {
                size.trim()
                    .parse()
                    .map_err(|_| format!("--frames does not take {size}"))
            })
            .collect::<Result<_, _>>()?;
        let seconds: f64 = option(args, "--seconds")?.unwrap_or(60.0);
        let kill_at: Option<f64> = option(args, "--kill-at")?;
        let gain: f32 = option(args, "--gain")?.unwrap_or(0.5);
        if !(seconds > 0.0 && seconds <= 3600.0 && gain.is_finite()) {
            return Err("--seconds takes up to 3600, --gain a number".to_owned());
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut reports = Vec::with_capacity(frames.len());
        for frames in frames {
            eprintln!("{frames} frames for {seconds} s ...");
            let plan = Plan {
                frames,
                seconds,
                kill_at,
                gain,
            };
            reports.push(bench::run(&exe, &plan).map_err(|e| format!("{frames} frames: {e}"))?);
        }
        print!("{}", bench::table(&reports));
        Ok(())
    }
}

#[cfg(not(windows))]
mod run {
    const NOT_YET: &str = "shared memory between two processes exists for Windows only so far";

    pub(super) fn host(_args: &[String]) -> Result<(), String> {
        let _ = (super::only, super::option::<u32>);
        Err(NOT_YET.to_owned())
    }

    pub(super) fn bench(_args: &[String]) -> Result<(), String> {
        Err(NOT_YET.to_owned())
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let done = match args.split_first() {
        Some((command, rest)) if command == "host" => run::host(rest),
        Some((command, rest)) if command == "bench" => run::bench(rest),
        _ => Err(USAGE.to_owned()),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("pult-plughost: {message}");
            ExitCode::from(2)
        }
    }
}
