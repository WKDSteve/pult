//! The command line, parsed by hand: a few subcommands do not need a parser crate.

use std::fmt;
use std::path::PathBuf;

use pult_engine::profile::{DEFAULT_LATENCY_MS, GAIN_DB, LATENCY_MS};
use pult_engine::{BusSpec, InputKind, InputSpec, RouteSpec};

/// Test tone level when `--gain-db` is not given: quiet, for a chain ending at an amplifier.
pub const DEFAULT_TONE_DB: f32 = -30.0;

/// Test tone frequency when `--freq` is not given.
pub const DEFAULT_TONE_HZ: f32 = 440.0;

/// How long `pult record` records when not told.
const DEFAULT_RECORD_SECONDS: f64 = 10.0;

/// How many bursts `pult roundtrip` plays when not told.
pub const DEFAULT_ROUNDTRIP_COUNT: u32 = 20;

/// Milliseconds between two bursts of `pult roundtrip` when not told.
pub const DEFAULT_ROUNDTRIP_GAP_MS: u32 = 500;

/// The level of the bursts of `pult roundtrip` when not told: clear above a converter's noise,
/// not painful on a speaker left on by mistake.
pub const DEFAULT_ROUNDTRIP_DB: f32 = -12.0;

/// The tone `pult clicks` expects when not told: the one of the click tests, low, so that a
/// jump stands far above its steepest step.
pub const DEFAULT_CLICK_TONE_HZ: f32 = 103.0;

/// The bus name of a route given with `--to`.
pub const INLINE_BUS: &str = "A1";

/// How long `pult midi listen` listens when not told.
const DEFAULT_LISTEN_SECONDS: u64 = 20;

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
              (a test tone: it ends at once, without a fade, and may click there)
    pult record --from <INPUT|out:OUTPUT> --to <FILE.wav> [--seconds <S>]
    pult clicks <FILE.wav> [--freq <HZ>] [--quiet <S>] [--skip <S>] [--until <S>]
    pult roundtrip --to <OUTPUT> --from <INPUT|out:OUTPUT> [--count <N>] [--gap-ms <MS>]
                   [--gain-db <DB>] [--keep <FILE.wav>]
    pult help | --help
    pult --version

  To the pult that is running:
    pult gain <TARGET> <DB> [over <MS>]
    pult mute <TARGET>      pult unmute <TARGET>
    pult solo <INPUT>       pult unsolo <INPUT>
    pult pan <INPUT> <-1..1> [over <MS>]
    pult gate <INPUT> <0..10|off>   pult comp <INPUT> <0..10|off>
    pult eq <BUS> on|off    pult eq <BUS> <BAND> gain|freq|q|kind <VALUE>
    pult limit <BUS> on|off    pult limit <BUS> <DB>    pult limit <BUS> look <MS>
    pult route <INPUT> <BUS>      pult unroute <INPUT> <BUS>
    pult scene <NAME>       pult scenes
    pult show [--json]
    pult save
    pult open [--print]     the remote control page in the browser, or its link
    pult stop               end the route, as Enter in it would
    pult learn <ADDRESS>    the next movement on the MIDI controller binds it; waits for it
    pult unlearn <ADDRESS>  take a binding off; an address is input/1/gain, bus/A1/mute,
                            matrix/1/A1 or scene/abend
    pult midi               the device and what is bound

  Without a running pult:
    pult midi devices       the MIDI ports of this machine
    pult midi listen [S]    print what they send for S seconds (default 20)

  The window:
    pult start [--open]     start it unless a route runs; --open shows the page too

A target is an input number (1), a unique part of its name or label (cable), a bus name (A1)
or a unique part of a bus label (boxen); what fits more than one is refused. `save` writes to the profile the route came from. These end with status 1 when no pult
runs or it refuses, and with 2 when the words are no command for it.

Device names may be a unique part of the name, ignoring case (see `pult devices`).
--from app:<PROGRAM> captures the sound of a running program instead of a device, for
example \"app:Spotify\" (see `pult programs`); with * or ? it is a pattern for the whole
program name, for example \"app:discord*\" (quote it, so the shell leaves the star alone).
GAIN_DB is a gain in dB, for example \"CABLE Output@-6\". --latency-ms is the buffering per
input (default 40); raise it if you hear dropouts. --profile reads inputs, buses (several
outputs) and which input feeds which bus from a TOML file (see examples/pult.toml).
--log writes a CSV row with the levels and counters every 10 s (the file is overwritten; a
run on a file that another run has open is refused).
`tone` plays a sine (default 440 Hz at -30 dB) to check a chain of devices and stops on
Enter. `record` writes what an input captures, or with out: before its name what an
output plays, to a WAV file for --seconds (default 10) or until Enter. `clicks` looks
for clicks in a recording of a steady tone of --freq Hz (default 103): steps steeper than
that tone can make at its level. --quiet says how many seconds at its start hold no tone
yet (their noise sets the floor), --skip how many to leave out at the start, --until
where to stop (a route that ends cuts its sound off); its status is 0
for no click, 1 for clicks and 2 when the recording cannot be judged. `roundtrip` measures
how long a way takes: it plays --count short bursts (default 20, one every --gap-ms, default
500, at --gain-db, default -12) on an output, records an input (or with out: what an output
plays) meanwhile and prints per burst the time between both, then median, minimum, maximum
and spread. The time contains the buffers of its own two streams: measure once straight and
once with the way in question in between, the difference is that way. --keep writes the
recording to a WAV file. Its status is 0 when every burst arrived, 1 when some were lost and
2 when none arrived. `run` takes commands while it runs (type help: gain, mute, solo, pan, show, save) and
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

/// A recording to make.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordSpec {
    /// An input device, or `out:` and an output device for what it plays.
    pub source: String,
    /// The WAV file to write.
    pub file: PathBuf,
    /// How long to record.
    pub seconds: f64,
}

/// A way to measure: bursts on an output, heard on an input.
#[derive(Debug, Clone, PartialEq)]
pub struct RoundTripSpec {
    /// What to play and where to listen.
    pub trip: pult_engine::RoundTripSpec,
    /// A WAV file for the recording, if it is to be kept.
    pub keep: Option<PathBuf>,
}

/// A recording of a test tone to look through for clicks.
#[derive(Debug, Clone, PartialEq)]
pub struct ClicksSpec {
    /// The WAV file to read.
    pub file: PathBuf,
    /// The frequency of the tone in Hz.
    pub frequency: f32,
    /// Seconds at the start that hold no tone yet: their noise sets the floor.
    pub quiet: f64,
    /// Seconds at the start that are not judged; at least `quiet`.
    pub skip: f64,
    /// The second up to which the recording is judged; to its end without one.
    pub until: Option<f64>,
}

/// What to ask of the pult that is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Remote {
    /// Its state; as JSON for programs.
    Show {
        /// As JSON.
        json: bool,
    },
    /// The names of its scenes, one per line.
    Scenes,
    /// Saves to the profile the route came from.
    Save,
    /// The words of a command of its input line, understood against its inputs and buses.
    Line(String),
    /// The remote control page, with a new pairing code.
    Open {
        /// Only print the link.
        print: bool,
    },
    /// Ends the route.
    Stop,
    /// The next movement binds this; waits for it.
    Learn(String),
    /// Takes a binding off.
    Unlearn(String),
    /// The device and what is bound.
    Midi,
    /// The ports of this machine.
    MidiDevices,
    /// Prints what the ports say for so many seconds.
    MidiListen(u64),
    /// Starts the window unless a route runs.
    Start {
        /// Opens the page too.
        open: bool,
    },
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
    /// Record a device to a file.
    Record(RecordSpec),
    /// Look for clicks in a recording.
    Clicks(ClicksSpec),
    /// Measure how long a way takes.
    RoundTrip(RoundTripSpec),
    /// Ask the pult that is running.
    Remote(Remote),
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
        "record" => parse_record(args).map(Command::Record),
        "clicks" => parse_clicks(args).map(Command::Clicks),
        "roundtrip" => parse_roundtrip(args).map(Command::RoundTrip),
        "scenes" => no_more(args).map(|()| Command::Remote(Remote::Scenes)),
        "show" => match args.next().as_deref() {
            None => Ok(Command::Remote(Remote::Show { json: false })),
            Some("--json") => no_more(args).map(|()| Command::Remote(Remote::Show { json: true })),
            Some(other) => Err(usage(format!("unexpected argument \"{other}\""))),
        },
        "save" => match args.next() {
            None => Ok(Command::Remote(Remote::Save)),
            Some(_) => Err(usage(
                "a running pult saves to the profile it came from; save takes no file here",
            )),
        },
        "stop" => no_more(args).map(|()| Command::Remote(Remote::Stop)),
        "midi" => match args.next().as_deref() {
            None => Ok(Command::Remote(Remote::Midi)),
            Some("devices") => no_more(args).map(|()| Command::Remote(Remote::MidiDevices)),
            Some("listen") => match args.next() {
                None => Ok(Command::Remote(Remote::MidiListen(DEFAULT_LISTEN_SECONDS))),
                Some(seconds) => seconds
                    .parse()
                    .map(|s| Command::Remote(Remote::MidiListen(s)))
                    .map_err(|_| usage(format!("\"{seconds}\" is no number of seconds"))),
            },
            Some(other) => Err(usage(format!("unexpected argument \"{other}\""))),
        },
        "learn" | "unlearn" => {
            let target = args.collect::<Vec<String>>().join(" ");
            if target.trim().is_empty() {
                return Err(usage(format!(
                    "{sub} needs an address (input/1/gain, scene/abend)"
                )));
            }
            Ok(Command::Remote(if sub == "learn" {
                Remote::Learn(target)
            } else {
                Remote::Unlearn(target)
            }))
        }
        "open" => match args.next().as_deref() {
            None => Ok(Command::Remote(Remote::Open { print: false })),
            Some("--print") => {
                no_more(args).map(|()| Command::Remote(Remote::Open { print: true }))
            }
            Some(other) => Err(usage(format!("unexpected argument \"{other}\""))),
        },
        "start" => match args.next().as_deref() {
            None => Ok(Command::Remote(Remote::Start { open: false })),
            Some("--open") => no_more(args).map(|()| Command::Remote(Remote::Start { open: true })),
            Some(other) => Err(usage(format!("unexpected argument \"{other}\""))),
        },
        "gain" | "mute" | "unmute" | "solo" | "unsolo" | "pan" | "gate" | "comp" | "compressor"
        | "eq" | "limit" | "route" | "unroute" | "send" | "scene" => {
            // Quoted or not: the input line reads a name of several words either way.
            let rest: Vec<String> = args.collect();
            Ok(Command::Remote(Remote::Line(format!(
                "{sub} {}",
                rest.join(" ")
            ))))
        }
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
            scenes: Vec::new(),
            pfl: false,
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

/// The value of the option `name`, a number of seconds from 0 to an hour.
fn seconds(name: &str, raw: Option<String>) -> Result<f64, UsageError> {
    let raw = raw.ok_or_else(|| usage(format!("{name} needs a value")))?;
    raw.trim()
        .parse::<f64>()
        .ok()
        .filter(|s| (0.0..=3600.0).contains(s))
        .ok_or_else(|| usage(format!("{name} must be 0 to 3600 seconds, not \"{raw}\"")))
}

fn parse_record(mut args: impl Iterator<Item = String>) -> Result<RecordSpec, UsageError> {
    let mut source = None;
    let mut file = None;
    let mut length = DEFAULT_RECORD_SECONDS;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--from" => {
                let name = args.next().ok_or_else(|| usage("--from needs a value"))?;
                once(&mut source, name, "--from")?;
            }
            "--to" => {
                let name = args.next().ok_or_else(|| usage("--to needs a value"))?;
                once(&mut file, PathBuf::from(name), "--to")?;
            }
            "--seconds" => length = seconds("--seconds", args.next())?,
            other => return Err(usage(format!("unknown option \"{other}\""))),
        }
    }
    Ok(RecordSpec {
        source: source.ok_or_else(|| usage("record needs a device (--from)"))?,
        file: file.ok_or_else(|| usage("record needs a file (--to)"))?,
        seconds: length,
    })
}

fn parse_roundtrip(mut args: impl Iterator<Item = String>) -> Result<RoundTripSpec, UsageError> {
    let mut sink = None;
    let mut source = None;
    let mut keep = None;
    let mut count = DEFAULT_ROUNDTRIP_COUNT;
    let mut gap_ms = DEFAULT_ROUNDTRIP_GAP_MS;
    let mut gain_db = DEFAULT_ROUNDTRIP_DB;
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .ok_or_else(|| usage(format!("{name} needs a value")))
        };
        let whole = |name: &str, raw: String, range: std::ops::RangeInclusive<u32>| {
            raw.trim()
                .parse::<u32>()
                .ok()
                .filter(|v| range.contains(v))
                .ok_or_else(|| {
                    let (lo, hi) = range.into_inner();
                    usage(format!("{name} must be {lo} to {hi}, not \"{raw}\""))
                })
        };
        match arg.as_str() {
            "--to" => once(&mut sink, value("--to")?, "--to")?,
            "--from" => once(&mut source, value("--from")?, "--from")?,
            "--keep" => once(&mut keep, PathBuf::from(value("--keep")?), "--keep")?,
            "--count" => count = whole("--count", value("--count")?, 1..=1000)?,
            "--gap-ms" => {
                let shortest = pult_engine::SHORTEST_GAP_MS;
                gap_ms = whole("--gap-ms", value("--gap-ms")?, shortest..=10_000)?;
            }
            "--gain-db" => {
                let raw = value("--gain-db")?;
                gain_db = raw
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|v| (-96.0..=0.0).contains(v))
                    .ok_or_else(|| usage(format!("--gain-db must be -96 to 0, not \"{raw}\"")))?;
            }
            other => return Err(usage(format!("unknown option \"{other}\""))),
        }
    }
    Ok(RoundTripSpec {
        trip: pult_engine::RoundTripSpec {
            sink: sink.ok_or_else(|| usage("roundtrip needs an output (--to)"))?,
            source: source.ok_or_else(|| usage("roundtrip needs a device to hear (--from)"))?,
            count,
            gap_ms,
            gain_db,
        },
        keep,
    })
}

fn parse_clicks(mut args: impl Iterator<Item = String>) -> Result<ClicksSpec, UsageError> {
    let mut file = None;
    let mut frequency = DEFAULT_CLICK_TONE_HZ;
    let mut quiet = 0.0;
    let mut skip = 0.0;
    let mut until = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--freq" => {
                let raw = args.next().ok_or_else(|| usage("--freq needs a value"))?;
                frequency = raw
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|hz| (20.0..=20_000.0).contains(hz))
                    .ok_or_else(|| usage(format!("--freq must be 20 to 20000, not \"{raw}\"")))?;
            }
            "--quiet" => quiet = seconds("--quiet", args.next())?,
            "--skip" => skip = seconds("--skip", args.next())?,
            "--until" => until = Some(seconds("--until", args.next())?),
            option if option.starts_with("--") => {
                return Err(usage(format!("unknown option \"{option}\"")));
            }
            _ => once(&mut file, PathBuf::from(arg), "the file")?,
        }
    }
    Ok(ClicksSpec {
        file: file.ok_or_else(|| usage("clicks needs a recording (a WAV file)"))?,
        frequency,
        quiet,
        skip,
        until,
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
    fn solo_goes_to_the_running_pult_in_the_words_of_its_input_line() {
        for word in ["solo", "unsolo"] {
            assert_eq!(
                parse([word.to_owned(), "Analogue".to_owned(), "1".to_owned()]),
                Ok(Command::Remote(Remote::Line(format!("{word} Analogue 1"))))
            );
        }
    }

    #[test]
    fn gate_and_comp_go_to_the_running_pult_too() {
        assert_eq!(
            parse(["gate".to_owned(), "2".to_owned(), "3".to_owned()]),
            Ok(Command::Remote(Remote::Line("gate 2 3".to_owned())))
        );
        assert_eq!(
            parse(["comp".to_owned(), "Analogue 1".to_owned(), "off".to_owned()]),
            Ok(Command::Remote(Remote::Line(
                "comp Analogue 1 off".to_owned()
            )))
        );
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
                    scenes: Vec::new(),
                    pfl: false,
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

    fn words(line: &str) -> Result<Command, UsageError> {
        parse(line.split_whitespace().map(String::from))
    }

    #[test]
    fn roundtrip_takes_two_devices_and_how_to_play() {
        let Ok(Command::RoundTrip(spec)) = words("roundtrip --to Focusrite --from out:In5") else {
            panic!("a roundtrip command");
        };
        assert_eq!(spec.trip.sink, "Focusrite");
        assert_eq!(spec.trip.source, "out:In5");
        assert_eq!(spec.trip.count, DEFAULT_ROUNDTRIP_COUNT);
        assert_eq!(spec.trip.gap_ms, DEFAULT_ROUNDTRIP_GAP_MS);
        assert!((spec.trip.gain_db - DEFAULT_ROUNDTRIP_DB).abs() < f32::EPSILON);
        assert_eq!(spec.keep, None);
        let line =
            "roundtrip --from mic --to out --count 5 --gap-ms 250 --gain-db -20 --keep a.wav";
        let Ok(Command::RoundTrip(spec)) = words(line) else {
            panic!("a roundtrip command");
        };
        assert_eq!((spec.trip.count, spec.trip.gap_ms), (5, 250));
        assert!((spec.trip.gain_db + 20.0).abs() < f32::EPSILON);
        assert_eq!(spec.keep, Some(PathBuf::from("a.wav")));
        for (line, why) in [
            ("roundtrip --from mic", "--to"),
            ("roundtrip --to out", "--from"),
            ("roundtrip --to out --from mic --count 0", "1 to 1000"),
            ("roundtrip --to out --from mic --gap-ms 10", "50 to 10000"),
            ("roundtrip --to out --from mic --gain-db 3", "-96 to 0"),
            ("roundtrip --to out --from mic --count", "needs a value"),
            ("roundtrip --to out --to out --from mic", "only once"),
            ("roundtrip --to out --from mic --loud", "unknown option"),
        ] {
            let error = words(line).expect_err(line);
            assert!(error.0.contains(why), "{line}: {error}");
        }
    }

    #[test]
    fn record_takes_a_device_a_file_and_a_time() {
        assert_eq!(
            words("record --from out:In5 --to a.wav"),
            Ok(Command::Record(RecordSpec {
                source: "out:In5".to_owned(),
                file: PathBuf::from("a.wav"),
                seconds: DEFAULT_RECORD_SECONDS,
            }))
        );
        let Ok(Command::Record(spec)) = words("record --to a.wav --seconds 2.5 --from mic") else {
            panic!("a record command");
        };
        assert!((spec.seconds - 2.5).abs() < 1e-9);
        for (line, why) in [
            ("record --to a.wav", "--from"),
            ("record --from mic", "--to"),
            ("record --from mic --to a.wav --seconds -1", "3600"),
            ("record --from mic --to a.wav --seconds", "needs a value"),
            ("record --from mic --from mic --to a.wav", "only once"),
            ("record --from mic --to a.wav --loud", "unknown option"),
        ] {
            let error = words(line).expect_err(line);
            assert!(error.0.contains(why), "{line}: {error}");
        }
    }

    #[test]
    fn clicks_takes_a_file_and_what_to_expect_in_it() {
        assert_eq!(
            words("clicks a.wav"),
            Ok(Command::Clicks(ClicksSpec {
                file: PathBuf::from("a.wav"),
                frequency: DEFAULT_CLICK_TONE_HZ,
                quiet: 0.0,
                skip: 0.0,
                until: None,
            }))
        );
        let Ok(Command::Clicks(spec)) =
            words("clicks --freq 440 --quiet 1 a.wav --skip 3 --until 9")
        else {
            panic!("a clicks command");
        };
        assert!((spec.frequency - 440.0).abs() < 1e-6);
        assert!((spec.quiet - 1.0).abs() < 1e-9 && (spec.skip - 3.0).abs() < 1e-9);
        assert_eq!(spec.until, Some(9.0));
        for (line, why) in [
            ("clicks", "a recording"),
            ("clicks a.wav b.wav", "only once"),
            ("clicks a.wav --freq 5", "20 to 20000"),
            ("clicks a.wav --quiet x", "3600"),
            ("clicks a.wav --floor 1", "unknown option"),
        ] {
            let error = words(line).expect_err(line);
            assert!(error.0.contains(why), "{line}: {error}");
        }
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
