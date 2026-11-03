//! The command line, parsed by hand: three subcommands do not need a parser crate.

use std::fmt;
use std::path::PathBuf;

use pult_engine::profile::{DEFAULT_LATENCY_MS, GAIN_DB, LATENCY_MS};
use pult_engine::{BusSpec, InputKind, InputSpec, RouteSpec};

/// Test tone level when `--gain-db` is not given: quiet, for a chain ending at an amplifier.
pub const DEFAULT_TONE_DB: f32 = -30.0;

/// Test tone frequency when `--freq` is not given.
pub const DEFAULT_TONE_HZ: f32 = 440.0;

/// The bus name of a route given with `--to`.
pub const INLINE_BUS: &str = "A1";

/// The usage text of `pult help`.
pub const USAGE: &str = "\
pult - route and mix audio devices

USAGE:
    pult devices
    pult programs
    pult run --to <OUTPUT> --from <INPUT>[@GAIN_DB] [--from ...] [--latency-ms <MS>]
             [--log <FILE.csv>]
    pult run --profile <FILE.toml> [--log <FILE.csv>]
    pult tone --to <OUTPUT> [--gain-db <DB>] [--freq <HZ>]
    pult help | --help
    pult --version

Device names may be a unique part of the name, ignoring case (see `pult devices`).
--from app:<PROGRAM> captures the sound of a running program instead of a device, for
example \"app:Spotify\" (see `pult programs`).
GAIN_DB is a gain in dB, for example \"CABLE Output@-6\". --latency-ms is the buffering per
input (default 40); raise it if you hear dropouts. --profile reads inputs, buses (several
outputs) and which input feeds which bus from a TOML file (see examples/pult.toml).
--log writes a CSV row with the levels and counters every 10 s (the file is overwritten).
`tone` plays a sine (default 440 Hz at -30 dB) to check a chain of devices and stops on
Enter. `run` takes commands while it runs (type help: gain, mute, pan, show, save) and
stops on an empty line, quit or Ctrl+C.

EXAMPLE:
    pult run --from \"CABLE Output\" --to Focusrite";

/// Where a run's route comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Routing {
    /// Given on the command line: inputs onto one bus.
    Inline(RouteSpec),
    /// A profile file, read when the run starts.
    Profile(PathBuf),
}

/// A route to run and how to watch it.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSpec {
    /// The devices, buses and buffering.
    pub routing: Routing,
    /// Where `--log` writes its CSV, if given.
    pub log: Option<PathBuf>,
}

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
    /// List the running programs.
    Programs,
    /// Run a route until stopped.
    Run(RunSpec),
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
        "programs" => no_more(args).map(|()| Command::Programs),
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

fn once<T>(slot: &mut Option<T>, value: T, name: &str) -> Result<(), UsageError> {
    if slot.replace(value).is_some() {
        Err(usage(format!("{name} may be given only once")))
    } else {
        Ok(())
    }
}

fn parse_run(mut args: impl Iterator<Item = String>) -> Result<RunSpec, UsageError> {
    let mut sources = Vec::new();
    let mut sink = None;
    let mut log = None;
    let mut profile = None;
    let mut latency_ms = None;
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .ok_or_else(|| usage(format!("{name} needs a value")))
        };
        match arg.as_str() {
            "--from" => sources.push(parse_source(&value("--from")?)?),
            "--to" => once(&mut sink, value("--to")?, "--to")?,
            "--log" => once(&mut log, PathBuf::from(value("--log")?), "--log")?,
            "--profile" => once(
                &mut profile,
                PathBuf::from(value("--profile")?),
                "--profile",
            )?,
            "--latency-ms" => {
                let raw = value("--latency-ms")?;
                let ms = raw
                    .parse()
                    .ok()
                    .filter(|ms| LATENCY_MS.contains(ms))
                    .ok_or_else(|| {
                        usage(format!(
                            "--latency-ms must be {} to {}, not \"{raw}\"",
                            LATENCY_MS.start(),
                            LATENCY_MS.end()
                        ))
                    })?;
                once(&mut latency_ms, ms, "--latency-ms")?;
            }
            other => return Err(usage(format!("unknown option \"{other}\""))),
        }
    }
    if let Some(profile) = profile {
        if !sources.is_empty() || sink.is_some() || latency_ms.is_some() {
            return Err(usage(
                "--profile cannot be combined with --from, --to or --latency-ms",
            ));
        }
        return Ok(RunSpec {
            routing: Routing::Profile(profile),
            log,
        });
    }
    let sink = sink.ok_or_else(|| usage("run needs an output (--to) or a --profile"))?;
    if sources.is_empty() {
        return Err(usage("run needs at least one input (--from)"));
    }
    Ok(RunSpec {
        routing: Routing::Inline(RouteSpec {
            inputs: sources,
            buses: vec![BusSpec::new(INLINE_BUS, sink)],
            latency_ms: latency_ms.unwrap_or(DEFAULT_LATENCY_MS),
        }),
        log,
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
                once(&mut sink, name, "--to")?;
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

/// `NAME` or `NAME@GAIN_DB`, feeding the one bus of an inline route. The suffix only counts
/// when it is a number, so a device name with an `@` in it still works.
fn parse_source(raw: &str) -> Result<InputSpec, UsageError> {
    let (device, gain_db) = match raw.rsplit_once('@') {
        Some((name, gain)) => match gain.trim().parse::<f32>() {
            Ok(db) if GAIN_DB.contains(&db) => (name, db),
            Ok(_) => {
                return Err(usage(format!(
                    "gain in \"{raw}\" must be {} to {} dB",
                    GAIN_DB.start(),
                    GAIN_DB.end()
                )));
            }
            Err(_) => (raw, 0.0),
        },
        None => (raw, 0.0),
    };
    let device = device.trim();
    let (device, kind) = match device.get(..4) {
        Some(prefix) if prefix.eq_ignore_ascii_case("app:") => {
            (device[4..].trim(), InputKind::Process)
        }
        _ => (device, InputKind::Device),
    };
    if device.is_empty() {
        return Err(usage("--from needs a device or program name"));
    }
    Ok(InputSpec {
        kind,
        ..InputSpec::new(device, gain_db, vec![0])
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

    fn inline(args: &[&str]) -> RouteSpec {
        match run(args) {
            Ok(Command::Run(RunSpec {
                routing: Routing::Inline(route),
                ..
            })) => route,
            other => panic!("expected an inline route, got {other:?}"),
        }
    }

    #[test]
    fn subcommands() {
        assert_eq!(parse(Vec::new()), Ok(Command::Help));
        assert_eq!(parse(["devices".to_owned()]), Ok(Command::Devices));
        assert_eq!(parse(["programs".to_owned()]), Ok(Command::Programs));
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
            "--log",
            "abend.csv",
        ]);
        assert_eq!(
            parsed,
            Ok(Command::Run(RunSpec {
                routing: Routing::Inline(RouteSpec {
                    inputs: vec![
                        InputSpec::new("CABLE Output", 0.0, vec![0]),
                        InputSpec::new("Analogue 1 + 2", -6.5, vec![0]),
                    ],
                    buses: vec![BusSpec::new("A1", "Focusrite")],
                    latency_ms: 60,
                }),
                log: Some(PathBuf::from("abend.csv")),
            }))
        );
    }

    #[test]
    fn app_prefix_captures_a_program() {
        let route = inline(&[
            "--from",
            "APP: Spotify@-3",
            "--from",
            "Appliance",
            "--to",
            "x",
        ]);
        assert_eq!(
            route.inputs,
            vec![
                InputSpec {
                    kind: InputKind::Process,
                    ..InputSpec::new("Spotify", -3.0, vec![0])
                },
                InputSpec::new("Appliance", 0.0, vec![0]),
            ]
        );
        assert!(run(&["--from", "app:", "--to", "x"]).is_err());
    }

    #[test]
    fn an_at_sign_without_a_number_stays_in_the_name() {
        let route = inline(&["--from", "Mic @ Desk", "--to", "x"]);
        assert_eq!(
            route.inputs,
            vec![InputSpec::new("Mic @ Desk", 0.0, vec![0])]
        );
        assert_eq!(route.latency_ms, DEFAULT_LATENCY_MS);
    }

    #[test]
    fn run_from_a_profile() {
        assert_eq!(
            run(&["--profile", "pult.toml", "--log", "a.csv"]),
            Ok(Command::Run(RunSpec {
                routing: Routing::Profile(PathBuf::from("pult.toml")),
                log: Some(PathBuf::from("a.csv")),
            }))
        );
        for extra in [["--to", "x"], ["--from", "a"], ["--latency-ms", "20"]] {
            let mut args = vec!["--profile", "p.toml"];
            args.extend(extra);
            assert!(run(&args).is_err(), "{extra:?}");
        }
        assert!(run(&["--profile", "a", "--profile", "b"]).is_err());
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
        assert!(
            run(&[
                "--from",
                "a",
                "--to",
                "x",
                "--latency-ms",
                "9",
                "--latency-ms",
                "9"
            ])
            .is_err()
        );
        assert!(run(&["--from", "a", "--to", "x", "--loud"]).is_err());
        assert!(run(&["--from", "a", "--to", "x", "--log"]).is_err());
        assert!(run(&["--from", "a", "--to", "x", "--log", "a", "--log", "b"]).is_err());
    }
}
