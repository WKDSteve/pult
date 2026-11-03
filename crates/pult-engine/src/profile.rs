//! Profiles: a route written down as a TOML file.
//!
//! ```toml
//! latency_ms = 40              # optional, default 40
//!
//! [[bus]]
//! name = "A1"                  # 1 to 8 letters, digits, - or _
//! device = "Focusrite"         # a unique part of the output's name
//! gain_db = 0                  # optional
//! mute = false                 # optional
//!
//! [[input]]
//! device = "CABLE Output"
//! buses = ["A1"]               # the matrix: which buses this input feeds
//! gain_db = -3                 # optional
//! pan = 0                      # optional, -1 left to 1 right
//! mute = false                 # optional
//! ```
//!
//! Unknown keys are errors, so a typo does not silently fall back to a default.

use std::fmt;
use std::ops::RangeInclusive;
use std::path::Path;

use serde::Deserialize;

use crate::{BusSpec, Error, InputSpec, RouteSpec};

/// Buffering per input when a profile or the command line names none.
pub const DEFAULT_LATENCY_MS: u32 = 40;

/// The buffering a route may ask for, in milliseconds.
pub const LATENCY_MS: RangeInclusive<u32> = 1..=2000;

/// The gain an input or a bus may have, in dB.
pub const GAIN_DB: RangeInclusive<f32> = -96.0..=24.0;

/// Longest bus name: it is a column of the status line.
const MAX_BUS_NAME: usize = 8;

/// Why a profile cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileError(pub String);

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProfileError {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    latency_ms: Option<u32>,
    #[serde(default)]
    bus: Vec<BusEntry>,
    #[serde(default)]
    input: Vec<InputEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BusEntry {
    name: String,
    device: String,
    #[serde(default)]
    gain_db: f32,
    #[serde(default)]
    mute: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputEntry {
    device: String,
    buses: Vec<String>,
    #[serde(default)]
    gain_db: f32,
    #[serde(default)]
    pan: f32,
    #[serde(default)]
    mute: bool,
}

fn fail<T>(message: String) -> Result<T, ProfileError> {
    Err(ProfileError(message))
}

fn check_gain(what: &str, gain_db: f32) -> Result<(), ProfileError> {
    if GAIN_DB.contains(&gain_db) {
        Ok(())
    } else {
        fail(format!(
            "{what}: gain_db must be {} to {} dB, not {gain_db}",
            GAIN_DB.start(),
            GAIN_DB.end()
        ))
    }
}

fn valid_bus_name(name: &str) -> bool {
    (1..=MAX_BUS_NAME).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Reads a profile from TOML text.
///
/// # Errors
///
/// [`ProfileError`] for a syntax error (with line and column), an unknown key, a value out
/// of range, a bad or repeated bus name, an input feeding an unknown bus, or a route that
/// [`RouteSpec::validate`] refuses.
pub fn parse(text: &str) -> Result<RouteSpec, ProfileError> {
    let file: File = toml::from_str(text).map_err(|e| ProfileError(e.to_string()))?;
    let latency_ms = file.latency_ms.unwrap_or(DEFAULT_LATENCY_MS);
    if !LATENCY_MS.contains(&latency_ms) {
        return fail(format!(
            "latency_ms must be {} to {}, not {latency_ms}",
            LATENCY_MS.start(),
            LATENCY_MS.end()
        ));
    }

    let mut buses: Vec<BusSpec> = Vec::with_capacity(file.bus.len());
    for entry in file.bus {
        if !valid_bus_name(&entry.name) {
            return fail(format!(
                "bus name \"{}\" must be 1 to {MAX_BUS_NAME} letters, digits, - or _",
                entry.name
            ));
        }
        if buses.iter().any(|b| b.name == entry.name) {
            return fail(format!("bus \"{}\" is defined twice", entry.name));
        }
        check_gain(&format!("bus {}", entry.name), entry.gain_db)?;
        buses.push(BusSpec {
            name: entry.name,
            device: entry.device,
            gain_db: entry.gain_db,
            muted: entry.mute,
        });
    }

    let mut inputs = Vec::with_capacity(file.input.len());
    for entry in file.input {
        let what = format!("input \"{}\"", entry.device);
        check_gain(&what, entry.gain_db)?;
        if !(-1.0..=1.0).contains(&entry.pan) {
            return fail(format!("{what}: pan must be -1 to 1, not {}", entry.pan));
        }
        let mut routed = Vec::with_capacity(entry.buses.len());
        for name in &entry.buses {
            let Some(index) = buses.iter().position(|b| &b.name == name) else {
                let known: Vec<&str> = buses.iter().map(|b| b.name.as_str()).collect();
                return fail(format!(
                    "{what} feeds bus \"{name}\", which is not defined (buses: {})",
                    known.join(", ")
                ));
            };
            routed.push(index);
        }
        inputs.push(InputSpec {
            device: entry.device,
            gain_db: entry.gain_db,
            pan: entry.pan,
            muted: entry.mute,
            buses: routed,
        });
    }

    let spec = RouteSpec {
        inputs,
        buses,
        latency_ms,
    };
    spec.validate().map_err(|err| match err {
        Error::InvalidRoute(why) => ProfileError(why),
        other => ProfileError(other.to_string()),
    })?;
    Ok(spec)
}

/// Reads the profile at `path`.
///
/// # Errors
///
/// [`ProfileError`] when the file cannot be read or [`parse`] refuses it; the message
/// names the file.
pub fn load(path: &Path) -> Result<RouteSpec, ProfileError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ProfileError(format!("cannot read {}: {e}", path.display())))?;
    parse(&text).map_err(|e| ProfileError(format!("{}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(text: &str) -> String {
        match parse(text) {
            Err(ProfileError(message)) => message,
            Ok(spec) => panic!("expected an error, got {spec:?}"),
        }
    }

    const TWO_BUSES: &str = r#"
        [[bus]]
        name = "A1"
        device = "Focusrite"

        [[bus]]
        name = "A2"
        device = "Headset"
        gain_db = -6
        mute = true

        [[input]]
        device = "CABLE Output"
        buses = ["A1", "A2"]

        [[input]]
        device = "Analogue 1 + 2"
        buses = ["A2"]
        gain_db = -3.5
        pan = -0.25
        mute = true
    "#;

    #[test]
    fn reads_buses_inputs_and_the_matrix() {
        let spec = parse(TWO_BUSES).expect("valid profile");
        assert_eq!(spec.latency_ms, DEFAULT_LATENCY_MS);
        assert_eq!(
            spec.buses,
            [
                BusSpec::new("A1", "Focusrite"),
                BusSpec {
                    gain_db: -6.0,
                    muted: true,
                    ..BusSpec::new("A2", "Headset")
                }
            ]
        );
        assert_eq!(
            spec.inputs,
            [
                InputSpec::new("CABLE Output", 0.0, vec![0, 1]),
                InputSpec {
                    pan: -0.25,
                    muted: true,
                    ..InputSpec::new("Analogue 1 + 2", -3.5, vec![1])
                }
            ]
        );
    }

    #[test]
    fn the_example_profile_is_valid() {
        let spec = parse(include_str!("../../../examples/pult.toml")).expect("example");
        assert!(!spec.inputs.is_empty() && !spec.buses.is_empty());
    }

    #[test]
    fn refuses_typos_and_values_out_of_range() {
        assert!(error("latency = 40").contains("unknown field"));
        assert!(error("[[bus]]\nname = \"A1\"\ndevice = \"x\"\nmuted = true").contains("muted"));
        assert!(error("latency_ms = 0").contains("latency_ms must be 1 to 2000"));
        assert!(error("[[bus]]\nname = \"A1\"\ndevice = \"x\"\ngain_db = 30").contains("bus A1"));
        let input = |extra: &str| {
            format!(
                "[[bus]]\nname = \"A1\"\ndevice = \"x\"\n[[input]]\ndevice = \"m\"\nbuses = [\"A1\"]\n{extra}"
            )
        };
        assert!(error(&input("pan = 2")).contains("pan must be -1 to 1"));
        assert!(error(&input("gain_db = -100")).contains("input \"m\""));
        assert!(error("[[bus]]\nname = 1").contains("line 2"));
    }

    #[test]
    fn refuses_bad_bus_names_and_routes() {
        assert!(error("[[bus]]\nname = \"A 1\"\ndevice = \"x\"").contains("letters, digits"));
        assert!(error("[[bus]]\nname = \"Lautsprecher\"\ndevice = \"x\"").contains("1 to 8"));
        assert!(
            error("[[bus]]\nname = \"A1\"\ndevice = \"x\"\n[[bus]]\nname = \"A1\"\ndevice = \"y\"")
                .contains("defined twice")
        );
        let feeds = |buses: &str| {
            format!(
                "[[bus]]\nname = \"A1\"\ndevice = \"x\"\n[[input]]\ndevice = \"m\"\nbuses = {buses}"
            )
        };
        assert!(error(&feeds("[\"B1\"]")).contains("\"B1\", which is not defined (buses: A1)"));
        assert!(error(&feeds("[]")).contains("feeds no bus"));
        assert!(error(&feeds("[\"A1\", \"A1\"]")).contains("twice"));
        assert!(error("[[bus]]\nname = \"A1\"\ndevice = \"x\"").contains("at least one input"));
    }

    #[test]
    fn load_names_the_file() {
        let path = Path::new("no/such/pult.toml");
        assert!(load(path).unwrap_err().0.contains("cannot read no"));
    }
}
