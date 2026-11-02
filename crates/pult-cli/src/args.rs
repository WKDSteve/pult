//! The command line, parsed by hand: three subcommands do not need a parser crate.

use std::fmt;

use pult_engine::{RouteSpec, SourceSpec};

/// Buffering per input when `--latency-ms` is not given.
pub const DEFAULT_LATENCY_MS: u32 = 40;

/// Test tone level when `--gain-db` is not given: quiet, for a chain ending at an amplifier.
pub const DEFAULT_TONE_DB: f32 = -30.0;

/// Test tone frequency when `--freq` is not given.
pub const DEFAULT_TONE_HZ: f32 = 440.0;

/// The usage text of `pult help`.
pub const USAGE: &str = "\
pult - route and mix audio devices

USAGE:
    pult devices
    pult run --to <OUTPUT> --from <INPUT>[@GAIN_DB] [--from ...] [--latency-ms <MS>]
    pult tone --to <OUTPUT> [--gain-db <DB>] [--freq <HZ>]
    pult help | --help
    pult --version

Device names may be a unique part of the name, ignoring case (see `pult devices`).
GAIN_DB is a gain in dB, for example \"CABLE Output@-6\". --latency-ms is the buffering per
input (default 40); raise it if you hear dropouts. `tone` plays a sine (default 440 Hz at
-30 dB) to check a chain of devices. Both run until Enter is pressed.

EXAMPLE:
    pult run --from \"CABLE Output\" --to Focusrite";

/// A test tone to play.
#[derive(Debug, Clone, PartialEq)]
pub struct ToneSpec {
    /// The output device name, or a unique part of it.
    pub sink: String,
    /// Level in dB.
    pub gain_db: f32,
    /// Frequency in Hz.
    pub frequency: f32,
}

/// What the user asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// List the devices.
    Devices,
    /// Run a route until stopped.
    Run(RouteSpec),
    /// Play a test tone until stopped.
    Tone(ToneSpec),
    /// Print the usage.
    Help,
    /// Print the version.
    Version,
}

/// Why the command line is not valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageError(pub String);

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn usage(message: impl Into<String>) -> UsageError {
    UsageError(message.into())
}

/// Parses the arguments after the program name.
///
/// # Errors
///
/// [`UsageError`] for an unknown subcommand or option, a missing value or an invalid number.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Command, UsageError> {
    let mut args = args.into_iter();
    let Some(sub) = args.next() else {
        return Ok(Command::Help);
    };
    match sub.as_str() {
        "devices" => no_more(args).map(|()| Command::Devices),
        "run" => parse_run(args).map(Command::Run),
        "tone" => parse_tone(args).map(Command::Tone),
        "help" | "--help" | "-h" => Ok(Command::Help),
        "--version" | "-V" => Ok(Command::Version),
        other => Err(usage(format!("unknown command \"{other}\""))),
    }
}

fn no_more(mut args: impl Iterator<Item = String>) -> Result<(), UsageError> {
    match args.next() {
        Some(extra) => Err(usage(format!("unexpected argument \"{extra}\""))),
        None => Ok(()),
    }
}

fn parse_run(mut args: impl Iterator<Item = String>) -> Result<RouteSpec, UsageError> {
    let mut sources = Vec::new();
    let mut sink = None;
    let mut latency_ms = DEFAULT_LATENCY_MS;
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .ok_or_else(|| usage(format!("{name} needs a value")))
        };
        match arg.as_str() {
            "--from" => sources.push(parse_source(&value("--from")?)?),
            "--to" => {
                if sink.replace(value("--to")?).is_some() {
                    return Err(usage("--to may be given only once"));
                }
            }
            "--latency-ms" => {
                let raw = value("--latency-ms")?;
                latency_ms = raw
                    .parse()
                    .ok()
                    .filter(|ms| (1..=2000).contains(ms))
                    .ok_or_else(|| {
                        usage(format!("--latency-ms must be 1 to 2000, not \"{raw}\""))
                    })?;
            }
            other => return Err(usage(format!("unknown option \"{other}\""))),
        }
    }
    let sink = sink.ok_or_else(|| usage("run needs an output (--to)"))?;
    if sources.is_empty() {
        return Err(usage("run needs at least one input (--from)"));
    }
    Ok(RouteSpec {
        sources,
        sink,
        latency_ms,
    })
}

fn parse_tone(mut args: impl Iterator<Item = String>) -> Result<ToneSpec, UsageError> {
    let mut sink = None;
    let mut gain_db = DEFAULT_TONE_DB;
    let mut frequency = DEFAULT_TONE_HZ;
    while let Some(arg) = args.next() {
        let mut number = |name: &str, range: std::ops::RangeInclusive<f32>| {
            let raw = args
                .next()
                .ok_or_else(|| usage(format!("{name} needs a value")))?;
            raw.trim()
                .parse::<f32>()
                .ok()
                .filter(|v| range.contains(v))
                .ok_or_else(|| {
                    let (lo, hi) = range.into_inner();
                    usage(format!("{name} must be {lo} to {hi}, not \"{raw}\""))
                })
        };
        match arg.as_str() {
            "--to" => {
                let name = args.next().ok_or_else(|| usage("--to needs a value"))?;
                if sink.replace(name).is_some() {
                    return Err(usage("--to may be given only once"));
                }
            }
            "--gain-db" => gain_db = number("--gain-db", -96.0..=0.0)?,
            "--freq" => frequency = number("--freq", 20.0..=20_000.0)?,
            other => return Err(usage(format!("unknown option \"{other}\""))),
        }
    }
    Ok(ToneSpec {
        sink: sink.ok_or_else(|| usage("tone needs an output (--to)"))?,
        gain_db,
        frequency,
    })
}

/// `NAME` or `NAME@GAIN_DB`. The suffix only counts when it is a number, so a device name
/// with an `@` in it still works.
fn parse_source(raw: &str) -> Result<SourceSpec, UsageError> {
    let (device, gain_db) = match raw.rsplit_once('@') {
        Some((name, gain)) => match gain.trim().parse::<f32>() {
            Ok(db) if db.is_finite() && (-96.0..=24.0).contains(&db) => (name, db),
            Ok(_) => return Err(usage(format!("gain in \"{raw}\" must be -96 to 24 dB"))),
            Err(_) => (raw, 0.0),
        },
        None => (raw, 0.0),
    };
    let device = device.trim();
    if device.is_empty() {
        return Err(usage("--from needs a device name"));
    }
    Ok(SourceSpec {
        device: device.to_owned(),
        gain_db,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Command, UsageError> {
        parse(
            std::iter::once("run")
                .chain(args.iter().copied())
                .map(String::from),
        )
    }

    fn source(device: &str, gain_db: f32) -> SourceSpec {
        SourceSpec {
            device: device.into(),
            gain_db,
        }
    }

    #[test]
    fn subcommands() {
        assert_eq!(parse(Vec::new()), Ok(Command::Help));
        assert_eq!(parse(["devices".to_owned()]), Ok(Command::Devices));
        assert_eq!(parse(["--version".to_owned()]), Ok(Command::Version));
        assert!(parse(["devices".to_owned(), "x".to_owned()]).is_err());
        assert!(parse(["mix".to_owned()]).is_err());
    }

    #[test]
    fn run_with_sources_gains_and_latency() {
        let parsed = run(&[
            "--from",
            "CABLE Output",
            "--from",
            "Analogue 1 + 2@-6.5",
            "--to",
            "Focusrite",
            "--latency-ms",
            "60",
        ]);
        assert_eq!(
            parsed,
            Ok(Command::Run(RouteSpec {
                sources: vec![source("CABLE Output", 0.0), source("Analogue 1 + 2", -6.5)],
                sink: "Focusrite".into(),
                latency_ms: 60,
            }))
        );
    }

    #[test]
    fn an_at_sign_without_a_number_stays_in_the_name() {
        let Ok(Command::Run(spec)) = run(&["--from", "Mic @ Desk", "--to", "x"]) else {
            panic!("expected a route");
        };
        assert_eq!(spec.sources, vec![source("Mic @ Desk", 0.0)]);
        assert_eq!(spec.latency_ms, DEFAULT_LATENCY_MS);
    }

    fn tone(args: &[&str]) -> Result<Command, UsageError> {
        parse(
            std::iter::once("tone")
                .chain(args.iter().copied())
                .map(String::from),
        )
    }

    #[test]
    fn tone_defaults_quiet_and_takes_level_and_frequency() {
        assert_eq!(
            tone(&["--to", "Focusrite"]),
            Ok(Command::Tone(ToneSpec {
                sink: "Focusrite".into(),
                gain_db: DEFAULT_TONE_DB,
                frequency: DEFAULT_TONE_HZ,
            }))
        );
        let Ok(Command::Tone(spec)) = tone(&["--freq", "1000", "--to", "x", "--gain-db", "-12"])
        else {
            panic!("expected a tone");
        };
        assert_eq!((spec.gain_db, spec.frequency), (-12.0, 1000.0));
    }

    #[test]
    fn invalid_tones_are_refused() {
        assert!(tone(&[]).is_err());
        assert!(tone(&["--to", "x", "--gain-db", "3"]).is_err());
        assert!(tone(&["--to", "x", "--freq", "5"]).is_err());
        assert!(tone(&["--to", "x", "--freq"]).is_err());
        assert!(tone(&["--to", "x", "--to", "y"]).is_err());
        assert!(tone(&["--to", "x", "--loud"]).is_err());
    }

    #[test]
    fn invalid_runs_are_refused() {
        assert!(run(&["--to", "x"]).is_err());
        assert!(run(&["--from", "a"]).is_err());
        assert!(run(&["--from", "a", "--to", "x", "--to", "y"]).is_err());
        assert!(run(&["--from", "a", "--to"]).is_err());
        assert!(run(&["--from", "a@40", "--to", "x"]).is_err());
        assert!(run(&["--from", "@-3", "--to", "x"]).is_err());
        assert!(run(&["--from", "a", "--to", "x", "--latency-ms", "0"]).is_err());
        assert!(run(&["--from", "a", "--to", "x", "--latency-ms", "fast"]).is_err());
        assert!(run(&["--from", "a", "--to", "x", "--loud"]).is_err());
    }
}
