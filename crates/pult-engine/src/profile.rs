//! Profiles: a route written down as a TOML file.
//!
//! ```toml
//! latency_ms = 40              # optional, default 40
//!
//! [[bus]]
//! name = "A1"                  # 1 to 8 letters, digits, - or _
//! device = "Focusrite"         # a unique part of the output's name
//! label = "Boxen"              # optional, what the window calls it
//! gain_db = 0                  # optional
//! mute = false                 # optional
//!
//! [[input]]
//! device = "CABLE Output"
//! buses = ["A1"]               # the matrix: which buses this input feeds
//! label = "Musik"              # optional, what the window calls it
//! gain_db = -3                 # optional
//! pan = 0                      # optional, -1 left to 1 right
//! mute = false                 # optional
//!
//! [[input]]
//! process = "Spotify"          # instead of device: the sound of a program
//! buses = ["A1"]
//!
//! [[scene]]
//! name = "abend"               # `scene abend` changes to it
//! blend_ms = 500               # optional, how long the change takes
//!
//! [[scene.input]]
//! input = 2                    # its place in the route, as in input/2/gain
//! gain_db = -12                # only what differs; also pan, mute, buses
//!
//! [[scene.bus]]
//! bus = "A1"
//! mute = false                 # also gain_db
//! ```
//!
//! Unknown keys are errors, so a typo does not silently fall back to a default.

use std::fmt;
use std::ops::RangeInclusive;
use std::path::Path;

use serde::Deserialize;

use std::fmt::Write as _;

use pult_core::MAX_RAMP_MS;
use toml_edit::{Array, DocumentMut, Item, Table, Value};

use crate::live::quoted;
use crate::{BusSpec, Error, InputKind, InputSpec, RouteSpec, SceneBus, SceneInput, SceneSpec};

/// Buffering per input when a profile or the command line names none.
pub const DEFAULT_LATENCY_MS: u32 = 40;

/// The buffering a route may ask for, in milliseconds.
pub const LATENCY_MS: RangeInclusive<u32> = 1..=2000;

/// The gain an input or a bus may have, in dB: the range of the view contract, so the
/// interface and the engine accept the same values.
pub const GAIN_DB: RangeInclusive<f32> =
    pult_core::view::GAIN_RANGE_DB.0..=pult_core::view::GAIN_RANGE_DB.1;

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
    #[serde(default)]
    scene: Vec<SceneEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SceneEntry {
    name: String,
    #[serde(default)]
    blend_ms: u32,
    #[serde(default)]
    input: Vec<SceneInputEntry>,
    #[serde(default)]
    bus: Vec<SceneBusEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SceneInputEntry {
    input: usize,
    gain_db: Option<f32>,
    pan: Option<f32>,
    mute: Option<bool>,
    buses: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SceneBusEntry {
    bus: String,
    gain_db: Option<f32>,
    mute: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BusEntry {
    name: String,
    device: String,
    label: Option<String>,
    #[serde(default)]
    gain_db: f32,
    #[serde(default)]
    mute: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputEntry {
    device: Option<String>,
    process: Option<String>,
    buses: Vec<String>,
    label: Option<String>,
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

fn check_label(what: &str, label: Option<&String>) -> Result<(), ProfileError> {
    if label.is_some_and(|label| label.trim().is_empty()) {
        return fail(format!("{what}: label must not be empty"));
    }
    Ok(())
}

fn check_pan(what: &str, pan: f32) -> Result<(), ProfileError> {
    if (-1.0..=1.0).contains(&pan) {
        Ok(())
    } else {
        fail(format!("{what}: pan must be -1 to 1, not {pan}"))
    }
}

/// The index of the bus called `name`, or an error for `what` listing the buses there are.
fn bus_index(what: &str, name: &str, buses: &[BusSpec]) -> Result<usize, ProfileError> {
    buses.iter().position(|b| b.name == name).ok_or_else(|| {
        let known: Vec<&str> = buses.iter().map(|b| b.name.as_str()).collect();
        ProfileError(format!(
            "{what} names bus \"{name}\", which is not defined (buses: {})",
            known.join(", ")
        ))
    })
}

/// The scene `entry` against what the route has; `count` is its number of inputs.
fn scene(entry: SceneEntry, count: usize, buses: &[BusSpec]) -> Result<SceneSpec, ProfileError> {
    if entry.name.is_empty() || entry.name != entry.name.trim() {
        return fail(format!(
            "scene \"{}\": a name must not be empty or start or end with a space",
            entry.name
        ));
    }
    let what = format!("scene \"{}\"", entry.name);
    if entry.blend_ms > MAX_RAMP_MS {
        return fail(format!(
            "{what}: blend_ms must be at most {MAX_RAMP_MS}, not {}",
            entry.blend_ms
        ));
    }
    if entry.input.is_empty() && entry.bus.is_empty() {
        return fail(format!("{what} changes nothing"));
    }
    let mut inputs: Vec<SceneInput> = Vec::with_capacity(entry.input.len());
    for input in entry.input {
        let Some(index) = input.input.checked_sub(1).filter(|&index| index < count) else {
            return fail(format!(
                "{what} names input {}; inputs are 1 to {count}",
                input.input
            ));
        };
        if inputs.iter().any(|known| known.input == index) {
            return fail(format!("{what} names input {} twice", input.input));
        }
        let place = format!("{what}, input {}", input.input);
        if let Some(gain_db) = input.gain_db {
            check_gain(&place, gain_db)?;
        }
        if let Some(pan) = input.pan {
            check_pan(&place, pan)?;
        }
        let routed = match input.buses {
            Some(names) => {
                if names.is_empty() {
                    return fail(format!("{place} would feed no bus; mute it instead"));
                }
                let mut routed = Vec::with_capacity(names.len());
                for name in &names {
                    let bus = bus_index(&place, name, buses)?;
                    if routed.contains(&bus) {
                        return fail(format!("{place} names bus {name} twice"));
                    }
                    routed.push(bus);
                }
                routed.sort_unstable();
                Some(routed)
            }
            None => None,
        };
        if input.gain_db.is_none()
            && input.pan.is_none()
            && input.mute.is_none()
            && routed.is_none()
        {
            return fail(format!("{place} changes nothing"));
        }
        inputs.push(SceneInput {
            input: index,
            gain_db: input.gain_db,
            pan: input.pan,
            muted: input.mute,
            buses: routed,
        });
    }
    let mut scene_buses: Vec<SceneBus> = Vec::with_capacity(entry.bus.len());
    for bus in entry.bus {
        let index = bus_index(&what, &bus.bus, buses)?;
        if scene_buses.iter().any(|known| known.bus == index) {
            return fail(format!("{what} names bus {} twice", bus.bus));
        }
        let place = format!("{what}, bus {}", bus.bus);
        if let Some(gain_db) = bus.gain_db {
            check_gain(&place, gain_db)?;
        }
        if bus.gain_db.is_none() && bus.mute.is_none() {
            return fail(format!("{place} changes nothing"));
        }
        scene_buses.push(SceneBus {
            bus: index,
            gain_db: bus.gain_db,
            muted: bus.mute,
        });
    }
    Ok(SceneSpec {
        name: entry.name,
        blend_ms: entry.blend_ms,
        inputs,
        buses: scene_buses,
    })
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
/// of range, a bad or repeated bus name, an input feeding an unknown bus, a scene naming what
/// the route does not have, or a route that [`RouteSpec::validate`] refuses.
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
        check_label(&format!("bus {}", entry.name), entry.label.as_ref())?;
        buses.push(BusSpec {
            name: entry.name,
            device: entry.device,
            label: entry.label,
            gain_db: entry.gain_db,
            muted: entry.mute,
        });
    }

    let mut inputs = Vec::with_capacity(file.input.len());
    for (number, entry) in file.input.into_iter().enumerate() {
        let (name, kind) = match (entry.device, entry.process) {
            (Some(device), None) => (device, InputKind::Device),
            (None, Some(program)) => (program, InputKind::Process),
            (Some(_), Some(_)) => {
                return fail(format!(
                    "input {}: give either device or process, not both",
                    number + 1
                ));
            }
            (None, None) => {
                return fail(format!("input {}: needs a device or a process", number + 1));
            }
        };
        let what = format!("input \"{name}\"");
        check_gain(&what, entry.gain_db)?;
        check_label(&what, entry.label.as_ref())?;
        check_pan(&what, entry.pan)?;
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
            device: name,
            kind,
            label: entry.label,
            gain_db: entry.gain_db,
            pan: entry.pan,
            muted: entry.mute,
            buses: routed,
        });
    }

    let mut scenes: Vec<SceneSpec> = Vec::with_capacity(file.scene.len());
    for entry in file.scene {
        let scene = scene(entry, inputs.len(), &buses)?;
        if scenes
            .iter()
            .any(|known| known.name.eq_ignore_ascii_case(&scene.name))
        {
            return fail(format!("scene \"{}\" is defined twice", scene.name));
        }
        scenes.push(scene);
    }

    let spec = RouteSpec {
        inputs,
        buses,
        latency_ms,
        scenes,
    };
    spec.validate().map_err(|err| match err {
        Error::InvalidRoute(why) => ProfileError(why),
        other => ProfileError(other.to_string()),
    })?;
    Ok(spec)
}

/// Writes `spec` as a profile that [`parse`] reads back to the same spec.
#[must_use]
pub fn render(spec: &RouteSpec) -> String {
    // Writing into a String cannot fail.
    let mut text = String::from("# pult profile, saved from a running route.\n\n");
    let _ = writeln!(text, "latency_ms = {}", spec.latency_ms);
    for bus in &spec.buses {
        let _ = write!(
            text,
            "\n[[bus]]\nname = {}\ndevice = {}\n{}gain_db = {}\nmute = {}\n",
            quoted(&bus.name),
            quoted(&bus.device),
            label_line(bus.label.as_ref()),
            bus.gain_db,
            bus.muted
        );
    }
    for input in &spec.inputs {
        let key = match input.kind {
            InputKind::Device => "device",
            InputKind::Process => "process",
        };
        let buses: Vec<String> = input
            .buses
            .iter()
            .map(|&bus| quoted(&spec.buses[bus].name))
            .collect();
        let _ = write!(
            text,
            "\n[[input]]\n{key} = {}\nbuses = [{}]\n{}gain_db = {}\npan = {}\nmute = {}\n",
            quoted(&input.device),
            buses.join(", "),
            label_line(input.label.as_ref()),
            input.gain_db,
            input.pan,
            input.muted
        );
    }
    for scene in &spec.scenes {
        let _ = write!(text, "\n[[scene]]\nname = {}\n", quoted(&scene.name));
        if scene.blend_ms > 0 {
            let _ = writeln!(text, "blend_ms = {}", scene.blend_ms);
        }
        for input in &scene.inputs {
            let _ = write!(text, "\n[[scene.input]]\ninput = {}\n", input.input + 1);
            render_changes(&mut text, input.gain_db, input.pan, input.muted);
            if let Some(buses) = &input.buses {
                let names: Vec<String> = buses
                    .iter()
                    .map(|&bus| quoted(&spec.buses[bus].name))
                    .collect();
                let _ = writeln!(text, "buses = [{}]", names.join(", "));
            }
        }
        for bus in &scene.buses {
            let name = quoted(&spec.buses[bus.bus].name);
            let _ = write!(text, "\n[[scene.bus]]\nbus = {name}\n");
            render_changes(&mut text, bus.gain_db, None, bus.muted);
        }
    }
    text
}

/// The lines of what a scene changes; what it leaves alone has none.
fn render_changes(text: &mut String, gain_db: Option<f32>, pan: Option<f32>, muted: Option<bool>) {
    if let Some(gain_db) = gain_db {
        let _ = writeln!(text, "gain_db = {gain_db}");
    }
    if let Some(pan) = pan {
        let _ = writeln!(text, "pan = {pan}");
    }
    if let Some(muted) = muted {
        let _ = writeln!(text, "mute = {muted}");
    }
}

/// Writes `spec` into `existing`, the text of a profile with the same buses and inputs: only
/// values that differ change, so comments, order, commented-out examples and scenes stay. Any
/// other text, or one that would not read back to `spec`, gives way to [`render`].
#[must_use]
pub fn merge(existing: &str, spec: &RouteSpec) -> String {
    merged(existing, spec)
        .filter(|text| parse(text).as_ref() == Ok(spec))
        .unwrap_or_else(|| render(spec))
}

/// The merge itself; `None` when `existing` is not a profile of these buses and inputs.
fn merged(existing: &str, spec: &RouteSpec) -> Option<String> {
    let mut file: DocumentMut = existing.parse().ok()?;

    if file.get("latency_ms").is_some() || spec.latency_ms != DEFAULT_LATENCY_MS {
        let same =
            file.get("latency_ms").and_then(Item::as_integer) == Some(i64::from(spec.latency_ms));
        if !same {
            set(
                file.as_table_mut(),
                "latency_ms",
                i64::from(spec.latency_ms).into(),
            );
        }
    }

    let buses = file.get_mut("bus")?.as_array_of_tables_mut()?;
    if buses.len() != spec.buses.len() {
        return None;
    }
    for (table, bus) in buses.iter_mut().zip(&spec.buses) {
        if text_of(table, "name") != Some(&bus.name)
            || text_of(table, "device") != Some(&bus.device)
        {
            return None;
        }
        set_label(table, bus.label.as_ref());
        set_number(table, "gain_db", bus.gain_db);
        set_switch(table, "mute", bus.muted);
    }

    let inputs = file.get_mut("input")?.as_array_of_tables_mut()?;
    if inputs.len() != spec.inputs.len() {
        return None;
    }
    for (table, input) in inputs.iter_mut().zip(&spec.inputs) {
        let key = match input.kind {
            InputKind::Device => "device",
            InputKind::Process => "process",
        };
        if text_of(table, key) != Some(&input.device) {
            return None;
        }
        let routed: Vec<&str> = input
            .buses
            .iter()
            .map(|&bus| spec.buses[bus].name.as_str())
            .collect();
        let listed: Option<Vec<&str>> = table
            .get("buses")
            .and_then(Item::as_array)
            .map(|list| list.iter().filter_map(Value::as_str).collect());
        if listed.as_ref() != Some(&routed) {
            set(table, "buses", Array::from_iter(routed).into());
        }
        set_label(table, input.label.as_ref());
        set_number(table, "gain_db", input.gain_db);
        set_number(table, "pan", input.pan);
        set_switch(table, "mute", input.muted);
    }
    // The parser reads line ends as \n; a Windows file keeps its own.
    let text = file.to_string();
    Some(if existing.contains("\r\n") {
        text.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        text
    })
}

fn text_of<'a>(table: &'a Table, key: &str) -> Option<&'a str> {
    table.get(key).and_then(Item::as_str)
}

/// Sets `key`, keeping the comment behind an old value.
fn set(table: &mut Table, key: &str, value: Value) {
    match table.get_mut(key).and_then(Item::as_value_mut) {
        Some(old) => {
            let decor = old.decor().clone();
            *old = value;
            *old.decor_mut() = decor;
        }
        None => {
            table.insert(key, Item::Value(value));
        }
    }
}

fn set_label(table: &mut Table, label: Option<&String>) {
    match label {
        Some(label) if text_of(table, "label") != Some(label) => {
            set(table, "label", label.as_str().into());
        }
        Some(_) => {}
        None => {
            table.remove("label");
        }
    }
}

/// Sets a number that defaults to 0, unless it reads as `number` already.
fn set_number(table: &mut Table, key: &str, number: f32) {
    let old = table.get(key).and_then(|item| {
        item.as_float()
            .or_else(|| item.as_integer().map(|whole| whole as f64))
    });
    // Exactly: parse reads the file's number to this very f32 if it did not change.
    #[allow(clippy::cast_possible_truncation, clippy::float_cmp)]
    let same = old.map_or(number == 0.0, |old| old as f32 == number);
    if !same {
        // Through the text, so -3.7 stays -3.7 and does not grow the digits of an f32.
        let shown = number.to_string();
        let value = shown.parse::<i64>().map_or_else(
            |_| shown.parse::<f64>().map_or(Value::from(0.0), Value::from),
            Value::from,
        );
        set(table, key, value);
    }
}

/// Sets a switch that defaults to off.
fn set_switch(table: &mut Table, key: &str, on: bool) {
    let same = table
        .get(key)
        .and_then(Item::as_bool)
        .map_or(!on, |old| old == on);
    if !same {
        set(table, key, on.into());
    }
}

/// The line of a label in a rendered profile; empty without one.
fn label_line(label: Option<&String>) -> String {
    label.map_or_else(String::new, |label| format!("label = {}\n", quoted(label)))
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
    fn an_input_is_a_device_or_a_program() {
        let bus = "[[bus]]\nname = \"A1\"\ndevice = \"x\"\n[[input]]\nbuses = [\"A1\"]\n";
        let spec = parse(&format!("{bus}process = \"Spotify\"")).expect("a program input");
        assert_eq!(spec.inputs[0].kind, InputKind::Process);
        assert_eq!(spec.inputs[0].device, "Spotify");
        assert!(error(&format!("{bus}process = \"a\"\ndevice = \"b\"")).contains("not both"));
        assert!(error(bus).contains("input 1: needs a device or a process"));
    }

    #[test]
    fn render_reads_back_to_the_same_route() {
        let mut spec = parse(TWO_BUSES).expect("valid profile");
        spec.inputs[0].kind = InputKind::Process;
        spec.inputs[0].device = "Spo\"ti\\fy".into();
        spec.inputs[1].gain_db = -96.0;
        spec.inputs[1].label = Some("Mu\"sik".into());
        spec.buses[0].gain_db = 24.0;
        spec.buses[1].label = Some("Kopfhörer".into());
        spec.latency_ms = 80;
        let text = render(&spec);
        assert_eq!(parse(&text), Ok(spec), "{text}");
    }

    #[test]
    fn a_label_is_optional_but_not_empty() {
        let spec = parse(
            "[[bus]]\nname = \"A1\"\ndevice = \"x\"\nlabel = \"Boxen\"\n\
             [[input]]\ndevice = \"m\"\nbuses = [\"A1\"]\nlabel = \"Musik\"",
        )
        .expect("labels");
        assert_eq!(spec.buses[0].label.as_deref(), Some("Boxen"));
        assert_eq!(spec.inputs[0].label.as_deref(), Some("Musik"));
        assert_eq!(parse(TWO_BUSES).expect("no labels").inputs[0].label, None);
        assert!(
            error("[[bus]]\nname = \"A1\"\ndevice = \"x\"\nlabel = \" \"")
                .contains("bus A1: label must not be empty")
        );
        assert!(
            error(
                "[[bus]]\nname = \"A1\"\ndevice = \"x\"\n\
                 [[input]]\ndevice = \"m\"\nbuses = [\"A1\"]\nlabel = \"\""
            )
            .contains("input \"m\": label must not be empty")
        );
    }

    const EXAMPLE: &str = include_str!("../../../examples/pult.toml");

    #[test]
    fn merge_changes_only_what_differs_and_keeps_the_rest() {
        let mut spec = parse(EXAMPLE).expect("example");
        assert_eq!(merge(EXAMPLE, &spec), EXAMPLE, "nothing changed");

        spec.inputs[0].gain_db = -3.7;
        spec.inputs[0].pan = 0.5;
        spec.inputs[0].label = Some("Musik".into());
        spec.inputs[1].muted = false;
        spec.buses[0].gain_db = -6.0;
        spec.latency_ms = 80;
        let text = merge(EXAMPLE, &spec);
        assert_eq!(parse(&text).as_ref(), Ok(&spec), "{text}");

        // Every comment line of the example is still there, in its order.
        let comments = |text: &str| -> Vec<String> {
            text.lines()
                .filter(|line| line.starts_with('#'))
                .map(str::to_owned)
                .collect()
        };
        assert_eq!(comments(&text), comments(EXAMPLE));
        assert!(text.contains("# process = \"Spotify\""));
        assert!(text.contains("gain_db = -3.7\n"), "{text}");
        assert!(text.contains("latency_ms = 80\n"), "{text}");
    }

    #[test]
    fn merge_keeps_the_comment_behind_a_value_and_changes_routes() {
        let existing = "[[bus]]\nname = \"A1\"\ndevice = \"x\"\n\
             [[bus]]\nname = \"A2\"\ndevice = \"y\"\n\
             [[input]]\ndevice = \"m\"\nbuses = [\"A1\"] # the matrix\n\
             gain_db = 0 # loud enough\n";
        let mut spec = parse(existing).expect("profile");
        spec.inputs[0].gain_db = -6.0;
        spec.inputs[0].buses = vec![0, 1];
        let text = merge(existing, &spec);
        assert!(text.contains("gain_db = -6 # loud enough"), "{text}");
        assert!(
            text.contains("buses = [\"A1\", \"A2\"] # the matrix"),
            "{text}"
        );
        assert_eq!(parse(&text), Ok(spec));
    }

    #[test]
    fn merge_keeps_the_line_ends_of_a_windows_file() {
        let existing = EXAMPLE.replace("\r\n", "\n").replace('\n', "\r\n");
        let mut spec = parse(&existing).expect("example");
        assert_eq!(merge(&existing, &spec), existing, "nothing changed");
        spec.inputs[0].gain_db = -6.0;
        let text = merge(&existing, &spec);
        assert_eq!(parse(&text), Ok(spec));
        assert_eq!(text.matches('\n').count(), text.matches("\r\n").count());
    }

    const SCENES: &str = r#"
        [[bus]]
        name = "A1"
        device = "Focusrite"

        [[bus]]
        name = "A2"
        device = "Headset"

        [[input]]
        device = "CABLE Output"
        buses = ["A1"]

        [[input]]
        device = "Analogue 1 + 2"
        buses = ["A1"]

        # For the evening.
        [[scene]]
        name = "Abend"     # the quiet one
        blend_ms = 500

        [[scene.input]]
        input = 2
        gain_db = -12.5    # just the voice
        pan = -0.5
        mute = false
        buses = ["A2", "A1"]

        [[scene.bus]]
        bus = "A2"
        gain_db = -6
        mute = true

        [[scene]]
        name = "Laut"

        [[scene.bus]]
        bus = "A1"
        gain_db = 3
    "#;

    /// `SCENES` with `from` replaced, which has to be there.
    fn scenes_with(from: &str, to: &str) -> String {
        assert!(SCENES.contains(from), "{from}");
        SCENES.replacen(from, to, 1)
    }

    #[test]
    fn reads_scenes_with_only_what_differs() {
        let spec = parse(SCENES).expect("valid profile");
        assert_eq!(
            spec.scenes,
            [
                SceneSpec {
                    name: "Abend".into(),
                    blend_ms: 500,
                    inputs: vec![SceneInput {
                        input: 1,
                        gain_db: Some(-12.5),
                        pan: Some(-0.5),
                        muted: Some(false),
                        buses: Some(vec![0, 1]),
                    }],
                    buses: vec![SceneBus {
                        bus: 1,
                        gain_db: Some(-6.0),
                        muted: Some(true),
                    }],
                },
                SceneSpec {
                    name: "Laut".into(),
                    blend_ms: 0,
                    inputs: Vec::new(),
                    buses: vec![SceneBus {
                        bus: 0,
                        gain_db: Some(3.0),
                        muted: None,
                    }],
                },
            ]
        );
    }

    #[test]
    fn refuses_scenes_that_do_not_fit_the_route() {
        for (from, to, why) in [
            ("input = 2", "input = 3", "names input 3; inputs are 1 to 2"),
            ("input = 2", "input = 0", "names input 0"),
            ("input = 2", "input = -1", "input"),
            (
                "buses = [\"A2\", \"A1\"]",
                "buses = [\"B9\"]",
                "bus \"B9\", which is not defined (buses: A1, A2)",
            ),
            (
                "buses = [\"A2\", \"A1\"]",
                "buses = []",
                "would feed no bus",
            ),
            (
                "buses = [\"A2\", \"A1\"]",
                "buses = [\"A1\", \"A1\"]",
                "names bus A1 twice",
            ),
            (
                "bus = \"A2\"",
                "bus = \"B9\"",
                "bus \"B9\", which is not defined",
            ),
            (
                "gain_db = -12.5",
                "gain_db = 30",
                "input 2: gain_db must be",
            ),
            ("gain_db = -6", "gain_db = -200", "bus A2: gain_db must be"),
            ("gain_db = -12.5", "gain_db = nan", "gain_db must be"),
            ("pan = -0.5", "pan = 2", "pan must be -1 to 1"),
            (
                "blend_ms = 500",
                "blend_ms = 60001",
                "blend_ms must be at most 60000",
            ),
            ("blend_ms = 500", "blend_ms = -1", "blend_ms"),
            ("blend_ms = 500", "blend = 500", "unknown field"),
            ("mute = false", "muted = false", "unknown field"),
            (
                "name = \"Laut\"",
                "name = \"abend\"",
                "scene \"abend\" is defined twice",
            ),
            ("name = \"Laut\"", "name = \"\"", "must not be empty"),
            (
                "name = \"Laut\"",
                "name = \" Laut\"",
                "start or end with a space",
            ),
            ("name = \"Laut\"", "", "missing field"),
        ] {
            let message = error(&scenes_with(from, to));
            assert!(message.contains(why), "{from} -> {to}: {message}");
        }
        let nothing = "[[bus]]\nname = \"A1\"\ndevice = \"x\"\n\
                       [[input]]\ndevice = \"m\"\nbuses = [\"A1\"]\n\
                       [[scene]]\nname = \"leer\"\n";
        assert!(error(nothing).contains("scene \"leer\" changes nothing"));
        assert!(
            error(&format!("{nothing}[[scene.input]]\ninput = 1\n"))
                .contains("input 1 changes nothing")
        );
        assert!(
            error(&format!("{nothing}[[scene.bus]]\nbus = \"A1\"\n"))
                .contains("bus A1 changes nothing")
        );
        assert!(
            error(&format!(
                "{nothing}[[scene.input]]\ninput = 1\nmute = true\n\
                 [[scene.input]]\ninput = 1\nmute = false\n"
            ))
            .contains("names input 1 twice")
        );
        assert!(
            error(&format!(
                "{nothing}[[scene.bus]]\nbus = \"A1\"\nmute = true\n\
                 [[scene.bus]]\nbus = \"A1\"\nmute = false\n"
            ))
            .contains("names bus A1 twice")
        );
    }

    #[test]
    fn render_reads_scenes_back() {
        let mut spec = parse(SCENES).expect("valid profile");
        spec.scenes[1].name = "La\"ut".into();
        let text = render(&spec);
        assert_eq!(parse(&text), Ok(spec), "{text}");
        assert!(!text.contains("blend_ms = 0"), "{text}");
    }

    #[test]
    fn merge_leaves_scenes_and_their_comments_alone() {
        let mut spec = parse(SCENES).expect("valid profile");
        assert_eq!(merge(SCENES, &spec), SCENES, "nothing changed");
        spec.inputs[1].gain_db = -3.0;
        spec.buses[1].muted = true;
        let text = merge(SCENES, &spec);
        assert_eq!(parse(&text).as_ref(), Ok(&spec), "{text}");
        let at = |text: &str| text.find("# For the evening.").expect("the scenes");
        assert_eq!(text[at(&text)..], SCENES[at(SCENES)..], "{text}");
    }

    #[test]
    fn merge_writes_anew_what_is_no_profile_of_this_route() {
        let spec = parse(TWO_BUSES).expect("profile");
        let fresh = render(&spec);
        assert_eq!(merge("not = [toml", &spec), fresh);
        assert_eq!(merge("# just a comment\n", &spec), fresh);
        // Another device, another bus count, another input.
        assert_eq!(merge(&TWO_BUSES.replace("Headset", "Boxen"), &spec), fresh);
        assert_eq!(merge(EXAMPLE, &spec), fresh);
        assert_eq!(merge(&TWO_BUSES.replace("CABLE", "VAIO"), &spec), fresh);
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
