//! The messages of the control interface: what a client reads of a running mixer and what it
//! may ask of it. Plain data without I/O, so a client (takt, a test) can use it without any
//! device code. `docs/CONTROL.md` describes it for readers.
//!
//! Parameters go by their addresses ([`Binding`]). There is no way to name a file here: a
//! client can save the profile the route came from, but not write anywhere else.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::binding::{Binding, BusParam, InputParam, Value};
use crate::control::{Command, Target};
use crate::view::{Action, BusView, EventKind, EventView, GAIN_RANGE_DB, InputView, MeterPoint};
use crate::view::{MixerView, SourceKind};

/// The version of these messages; a change that an old client would misread raises it.
pub const VERSION: u32 = 1;

/// The whole mixer as a client reads it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// [`VERSION`].
    pub version: u32,
    /// Seconds since the route started.
    pub elapsed_s: f64,
    /// Dropouts so far per hour, as events.
    pub dropouts_per_hour: f64,
    /// Glitches the devices reported so far.
    pub glitches: u64,
    /// Where the input meters measure.
    pub meter_point: MeterPlace,
    /// The file name of the profile the route came from, without its folder.
    pub profile: Option<String>,
    /// The inputs, in route order.
    pub inputs: Vec<InputState>,
    /// The buses, in route order.
    pub buses: Vec<BusState>,
    /// The latest events, oldest first; `seq` tells a client which it has seen.
    pub events: Vec<EventState>,
    /// The scenes it can change to, in profile order; none from a run older than scenes.
    #[serde(default)]
    pub scenes: Vec<String>,
    /// The MIDI controller, where one runs; none from a run older than that.
    #[serde(default)]
    pub midi: Option<crate::midi::MidiStatus>,
}

/// Where the input meters measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MeterPlace {
    /// Before gain and mute.
    Pre,
    /// After gain and mute.
    Post,
}

/// What an input captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputSource {
    /// A capture device.
    Device,
    /// The sound of one program.
    Program,
}

/// One input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputState {
    /// Its place in the route from 1, as in the addresses (`input/2/gain`).
    pub number: usize,
    /// The device's or program's full name.
    pub name: String,
    /// What the profile calls it, if it does.
    pub label: Option<String>,
    /// Device or program.
    pub source: InputSource,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Gain in dB.
    pub gain_db: f32,
    /// Muted.
    pub muted: bool,
    /// Soloed.
    pub solo: bool,
    /// -1 (left) to 1 (right).
    pub pan: f32,
    /// The names of the buses it feeds.
    pub buses: Vec<String>,
    /// Whether the device or program is there.
    pub present: bool,
    /// The highest peak since the previous state in dBFS; `None` for silence.
    pub peak_db: Option<f32>,
    /// The resampler's correction against clock drift in ppm.
    pub drift_ppm: f32,
    /// Times it ran dry or resynced so far.
    pub dropouts: u64,
}

/// One bus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BusState {
    /// The short name, as in the addresses (`bus/A1/mute`).
    pub name: String,
    /// What the profile calls it, if it does.
    pub label: Option<String>,
    /// The output device's full name.
    pub device: String,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Gain in dB.
    pub gain_db: f32,
    /// Muted.
    pub muted: bool,
    /// Whether the device is there.
    pub present: bool,
    /// The highest peak since the previous state in dBFS; `None` for silence.
    pub peak_db: Option<f32>,
    /// Samples clipped so far.
    pub clipped: u64,
}

/// What kind of thing happened to a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventType {
    /// It stopped; the rest plays on.
    Lost,
    /// It plays again.
    Back,
    /// It is there again but does not open yet.
    Refused,
    /// It plays, but Windows refused it realtime priority.
    Slow,
}

/// Something that happened to a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventState {
    /// Counts up from 1 during a run.
    pub seq: u64,
    /// Seconds since the route started.
    pub elapsed_s: u64,
    /// What kind of thing.
    #[serde(rename = "type")]
    pub event_type: EventType,
    /// What, as the console says it.
    pub text: String,
}

impl EventState {
    /// The event `seq` of a run.
    #[must_use]
    pub fn new(seq: u64, event: &EventView) -> Self {
        Self {
            seq,
            elapsed_s: event.elapsed.as_secs(),
            event_type: match event.kind {
                EventKind::Lost => EventType::Lost,
                EventKind::Back => EventType::Back,
                EventKind::Refused => EventType::Refused,
                EventKind::Slow => EventType::Slow,
            },
            text: event.text.clone(),
        }
    }
}

impl State {
    /// What a client reads of `view`, with the events the caller kept.
    #[must_use]
    pub fn new(view: &MixerView, events: Vec<EventState>) -> Self {
        let bus_name = |&bus: &usize| view.buses.get(bus).map(|b| b.name.clone());
        Self {
            version: VERSION,
            elapsed_s: view.elapsed.as_secs_f64(),
            dropouts_per_hour: view.dropouts_per_hour,
            glitches: view.glitches,
            meter_point: match view.meter_point {
                MeterPoint::PreFader => MeterPlace::Pre,
                MeterPoint::PostFader => MeterPlace::Post,
            },
            profile: view
                .profile
                .as_deref()
                .and_then(Path::file_name)
                .map(|name| name.to_string_lossy().into_owned()),
            inputs: view
                .inputs
                .iter()
                .enumerate()
                .map(|(index, input)| InputState::new(index, input, &bus_name))
                .collect(),
            buses: view.buses.iter().map(BusState::new).collect(),
            events,
            scenes: view.scenes.clone(),
            midi: view.midi.clone(),
        }
    }
}

impl InputState {
    fn new(index: usize, input: &InputView, bus_name: &impl Fn(&usize) -> Option<String>) -> Self {
        Self {
            number: index + 1,
            name: input.name.clone(),
            label: input.label.clone(),
            source: match input.kind {
                SourceKind::Device => InputSource::Device,
                SourceKind::Program => InputSource::Program,
            },
            sample_rate: input.sample_rate,
            gain_db: input.gain_db,
            muted: input.muted,
            solo: input.solo,
            pan: input.pan,
            buses: input.buses.iter().filter_map(bus_name).collect(),
            present: input.present,
            peak_db: input.peak_db,
            drift_ppm: input.drift_ppm,
            dropouts: input.dropouts,
        }
    }
}

impl BusState {
    fn new(bus: &BusView) -> Self {
        Self {
            name: bus.name.clone(),
            label: bus.label.clone(),
            device: bus.device.clone(),
            sample_rate: bus.sample_rate,
            gain_db: bus.gain_db,
            muted: bus.muted,
            present: bus.present,
            peak_db: bus.peak_db,
            clipped: bus.clipped,
        }
    }
}

/// A new value for a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OpValue {
    /// For a mute, a solo or a crosspoint.
    Switch(bool),
    /// For a gain in dB or a pan.
    Number(f32),
}

/// What a client asks of the mixer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Op {
    /// Sets the parameter at `address`.
    Set {
        /// For example `input/2/gain`, `bus/A1/mute` or `matrix/2/A1`.
        address: String,
        /// The new value.
        value: OpValue,
        /// Milliseconds to get there, for a gain or a pan.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ramp_ms: Option<u32>,
    },
    /// Changes to a scene of the route.
    Scene {
        /// Its name, ignoring case.
        name: String,
    },
    /// Moves the input meters before or after the fader.
    MeterPoint {
        /// Where to.
        value: MeterPlace,
    },
    /// Saves the current state to the profile the route came from. A variant with fields,
    /// empty as they are: serde would let anything pass beside a unit variant.
    Save {},
    /// Ends the route, as the person at it could. The one who runs it does that, not the desk.
    Stop {},
    /// The next movement on the MIDI controller binds this. For the one who runs it, not the desk.
    Learn {
        /// An address, or `scene/<name>`.
        target: String,
    },
    /// Takes a MIDI binding away. For the one who runs it, not the desk.
    Unlearn {
        /// An address, or `scene/<name>`.
        target: String,
    },
}

impl Op {
    /// What `command` asks of a mixer in `state`; `None` for what is no request to it (`show`, a
    /// file to save to, the end of a run). The indices come from parsing against `state`.
    #[must_use]
    pub fn of_command(command: &Command, state: &State) -> Option<Self> {
        let bus = |index: usize| state.buses.get(index).map(|bus| bus.name.as_str());
        let target = |target: Target, param: &str| match target {
            Target::Input(index) => Some(format!("input/{}/{param}", index + 1)),
            Target::Bus(index) => Some(format!("bus/{}/{param}", bus(index)?)),
        };
        let set = |address: String, value: OpValue, ramp_ms: u32| Self::Set {
            address,
            value,
            ramp_ms: (ramp_ms > 0).then_some(ramp_ms),
        };
        Some(match command {
            Command::Gain(to, db, ramp_ms) => {
                set(target(*to, "gain")?, OpValue::Number(*db), *ramp_ms)
            }
            Command::Mute(to, muted) => set(target(*to, "mute")?, OpValue::Switch(*muted), 0),
            Command::Pan(input, pan, ramp_ms) => set(
                target(Target::Input(*input), "pan")?,
                OpValue::Number(*pan),
                *ramp_ms,
            ),
            Command::Route(input, to, on) => set(
                format!("matrix/{}/{}", input + 1, bus(*to)?),
                OpValue::Switch(*on),
                0,
            ),
            Command::Scene(name) => Self::Scene { name: name.clone() },
            Command::Learn(target) => Self::Learn {
                target: target.clone(),
            },
            Command::Unlearn(target) => Self::Unlearn {
                target: target.clone(),
            },
            Command::Scenes | Command::Show | Command::Save(_) | Command::Help | Command::Stop => {
                return None;
            }
        })
    }

    /// The action this asks for in `view`.
    ///
    /// # Errors
    ///
    /// A message when the address names nothing in `view`, its parameter is only read, the
    /// value is of the wrong kind, a gain is out of range, or a ramp is too long or asked of
    /// what does not ramp. Elsewhere a fader's off is sent as the lowest gain; a client's number
    /// that is too high or too low is a mistake.
    pub fn action(&self, view: &MixerView) -> Result<Action, String> {
        match self {
            Self::Set {
                address,
                value,
                ramp_ms,
            } => {
                let binding = Binding::parse(address, view).map_err(|e| e.to_string())?;
                let value = value_for(binding, *value, address)?;
                let action = binding
                    .write(value)
                    .ok_or_else(|| format!("{address} can only be read"))?;
                match ramp_ms {
                    Some(ms) => action.over(*ms).map_err(|why| format!("{address}: {why}")),
                    None => Ok(action),
                }
            }
            Self::Scene { name } => Ok(Action::Scene { name: name.clone() }),
            Self::MeterPoint { value } => Ok(Action::SetMeterPoint(match value {
                MeterPlace::Pre => MeterPoint::PreFader,
                MeterPlace::Post => MeterPoint::PostFader,
            })),
            Self::Save {} => Ok(Action::SaveProfile { path: None }),
            Self::Stop {} => Err("ending the route is not for the desk".to_owned()),
            Self::Learn { .. } | Self::Unlearn { .. } => {
                Err("MIDI bindings are not for the desk".to_owned())
            }
        }
    }
}

/// `value` as the kind of value `binding` takes.
fn value_for(binding: Binding, value: OpValue, address: &str) -> Result<Value, String> {
    let wants_number = |number: f32| {
        if number.is_finite() {
            Ok(number)
        } else {
            Err(format!("{address} takes a finite number"))
        }
    };
    match (binding, value) {
        (
            Binding::Input(_, InputParam::Gain) | Binding::Bus(_, BusParam::Gain),
            OpValue::Number(db),
        ) => {
            let (low, high) = GAIN_RANGE_DB;
            let db = wants_number(db)?;
            if (low..=high).contains(&db) {
                Ok(Value::Gain(db))
            } else {
                Err(format!("gain {db} dB is outside {low} to {high} dB"))
            }
        }
        (Binding::Input(_, InputParam::Pan), OpValue::Number(pan)) => {
            Ok(Value::Pan(wants_number(pan)?))
        }
        (
            Binding::Input(_, InputParam::Mute | InputParam::Solo)
            | Binding::Bus(_, BusParam::Mute)
            | Binding::Route { .. },
            OpValue::Switch(on),
        ) => Ok(Value::Switch(on)),
        (Binding::Input(_, InputParam::Meter) | Binding::Bus(_, BusParam::Meter), _) => {
            Err(format!("{address} can only be read"))
        }
        (_, OpValue::Number(_)) => Err(format!("{address} takes true or false")),
        (_, OpValue::Switch(_)) => Err(format!("{address} takes a number")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{Desk, FakeDesk};

    fn view() -> MixerView {
        FakeDesk::new().view()
    }

    fn op(json: &str) -> Op {
        serde_json::from_str(json).expect(json)
    }

    fn action(json: &str) -> Result<Action, String> {
        op(json).action(&view())
    }

    #[test]
    fn a_state_names_things_as_the_addresses_do() {
        let view = view();
        let state = State::new(&view, Vec::new());
        assert_eq!(state.version, VERSION);
        assert_eq!(state.inputs[1].number, 2);
        assert_eq!(state.inputs[1].buses, ["A1", "A2"]);
        assert_eq!(state.buses[0].name, "A1");
        assert_eq!(state.meter_point, MeterPlace::Post);
        assert_eq!(state.profile.as_deref(), Some("pult.toml"));
        let text = serde_json::to_string(&state).expect("to JSON");
        let back: State = serde_json::from_str(&text).expect("from JSON");
        assert_eq!(back, state);
    }

    #[test]
    fn a_state_tells_no_folder_of_the_profile() {
        let mut view = view();
        view.profile = Some(r"C:\Users\someone\secret\abend.toml".into());
        let state = State::new(&view, Vec::new());
        // On Windows a path splits at the backslash; elsewhere it stays one name, so a slash.
        view.profile = Some("/home/someone/abend.toml".into());
        let other = State::new(&view, Vec::new());
        assert_eq!(other.profile.as_deref(), Some("abend.toml"));
        #[cfg(windows)]
        assert_eq!(state.profile.as_deref(), Some("abend.toml"));
        let _ = state;
    }

    #[test]
    fn an_event_says_its_type_in_a_word() {
        let event = EventView {
            elapsed: std::time::Duration::from_millis(5_900),
            kind: EventKind::Back,
            text: "A1 Focusrite is back".to_owned(),
        };
        let text = serde_json::to_string(&EventState::new(7, &event)).expect("to JSON");
        assert_eq!(
            text,
            r#"{"seq":7,"elapsed_s":5,"type":"back","text":"A1 Focusrite is back"}"#
        );
    }

    #[test]
    fn ops_become_the_actions_of_the_contract() {
        assert_eq!(
            action(r#"{"op":"set","address":"input/2/gain","value":-6}"#),
            Ok(Action::InputGain {
                input: 1,
                db: -6.0,
                ramp_ms: 0
            })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"input/2/pan","value":0.5}"#),
            Ok(Action::InputPan {
                input: 1,
                pan: 0.5,
                ramp_ms: 0
            })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"bus/A1/mute","value":true}"#),
            Ok(Action::BusMute {
                bus: 0,
                muted: true
            })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"matrix/2/A1","value":false}"#),
            Ok(Action::Route {
                input: 1,
                bus: 0,
                on: false
            })
        );
        assert_eq!(
            action(r#"{"op":"meter_point","value":"pre"}"#),
            Ok(Action::SetMeterPoint(MeterPoint::PreFader))
        );
        assert_eq!(
            action(r#"{"op":"save"}"#),
            Ok(Action::SaveProfile { path: None })
        );
    }

    #[test]
    fn a_command_becomes_the_op_for_its_address() {
        let view = view();
        let state = State::new(&view, Vec::new());
        // Every op goes through its action, so a wrong address would show here.
        let action = |command: &Command| {
            Op::of_command(command, &state)
                .expect("an op")
                .action(&view)
        };
        assert_eq!(
            action(&Command::Gain(Target::Bus(1), -6.0, 500)),
            Ok(Action::BusGain {
                bus: 1,
                db: -6.0,
                ramp_ms: 500
            })
        );
        assert_eq!(
            action(&Command::Gain(Target::Input(1), -3.0, 0)),
            Ok(Action::InputGain {
                input: 1,
                db: -3.0,
                ramp_ms: 0
            })
        );
        assert_eq!(
            action(&Command::Mute(Target::Input(2), true)),
            Ok(Action::InputMute {
                input: 2,
                muted: true
            })
        );
        assert_eq!(
            action(&Command::Mute(Target::Bus(0), false)),
            Ok(Action::BusMute {
                bus: 0,
                muted: false
            })
        );
        assert_eq!(
            action(&Command::Pan(0, 0.5, 20)),
            Ok(Action::InputPan {
                input: 0,
                pan: 0.5,
                ramp_ms: 20
            })
        );
        assert_eq!(
            action(&Command::Route(2, 0, true)),
            Ok(Action::Route {
                input: 2,
                bus: 0,
                on: true
            })
        );
        assert_eq!(
            action(&Command::Scene("Abend".to_owned())),
            Ok(Action::Scene {
                name: "Abend".to_owned()
            })
        );
    }

    #[test]
    fn what_is_no_request_to_a_mixer_becomes_no_op() {
        let state = State::new(&view(), Vec::new());
        for command in [
            Command::Show,
            Command::Scenes,
            Command::Help,
            Command::Stop,
            Command::Save("x.toml".to_owned()),
        ] {
            assert_eq!(Op::of_command(&command, &state), None, "{command:?}");
        }
        assert_eq!(
            Op::of_command(&Command::Route(0, 99, true), &state),
            None,
            "a bus past the state"
        );
    }

    #[test]
    fn a_gain_or_pan_can_take_its_time_and_a_scene_is_named() {
        assert_eq!(
            action(r#"{"op":"set","address":"input/2/gain","value":-6,"ramp_ms":500}"#),
            Ok(Action::InputGain {
                input: 1,
                db: -6.0,
                ramp_ms: 500
            })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"bus/A1/gain","value":0,"ramp_ms":60000}"#),
            Ok(Action::BusGain {
                bus: 0,
                db: 0.0,
                ramp_ms: 60_000
            })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"input/1/pan","value":1,"ramp_ms":20}"#),
            Ok(Action::InputPan {
                input: 0,
                pan: 1.0,
                ramp_ms: 20
            })
        );
        assert_eq!(
            action(r#"{"op":"scene","name":"Abend"}"#),
            Ok(Action::Scene {
                name: "Abend".to_owned()
            })
        );
        let refused = |json| action(json).expect_err(json);
        assert!(
            refused(r#"{"op":"set","address":"input/1/gain","value":0,"ramp_ms":60001}"#)
                .contains("at most 60000 ms")
        );
        assert!(
            refused(r#"{"op":"set","address":"input/1/mute","value":true,"ramp_ms":100}"#)
                .contains("only a gain or a pan")
        );
        assert!(
            refused(r#"{"op":"set","address":"matrix/1/A1","value":true,"ramp_ms":100}"#)
                .contains("only a gain or a pan")
        );
    }

    #[test]
    fn a_state_lists_the_scenes_and_reads_without_them() {
        let state = State::new(&view(), Vec::new());
        assert_eq!(state.scenes, ["Abend", "Laut"]);

        let mut older = serde_json::to_value(&state).expect("to JSON");
        older.as_object_mut().expect("an object").remove("scenes");
        let read: State = serde_json::from_value(older).expect("from JSON");
        assert_eq!(read.scenes, Vec::<String>::new());
    }

    #[test]
    fn a_bad_op_says_why_and_a_gain_is_not_clamped() {
        let refused = |json| action(json).expect_err(json);
        assert!(
            refused(r#"{"op":"set","address":"input/9/gain","value":0}"#).contains("no input 9")
        );
        assert!(
            refused(r#"{"op":"set","address":"nothing","value":0}"#).contains("not an address")
        );
        assert!(
            refused(r#"{"op":"set","address":"input/1/meter","value":0}"#).contains("only be read")
        );
        assert!(
            refused(r#"{"op":"set","address":"input/1/gain","value":true}"#)
                .contains("takes a number")
        );
        assert!(
            refused(r#"{"op":"set","address":"input/1/mute","value":1}"#).contains("true or false")
        );
        assert!(refused(r#"{"op":"set","address":"input/1/gain","value":60}"#).contains("outside"));
        assert!(
            refused(r#"{"op":"set","address":"input/1/gain","value":-200}"#).contains("outside")
        );
    }

    #[test]
    fn nothing_but_the_known_ops_reads() {
        for json in [
            r#"{"op":"save","path":"C:\\evil.toml"}"#,
            r#"{"op":"delete"}"#,
            r#"{"op":"set","address":"input/1/gain"}"#,
            r#"{"op":"set","address":"input/1/gain","value":"-6"}"#,
            r#"{"op":"set","address":"input/1/gain","value":0,"extra":1}"#,
            r#"{"op":"set","address":"input/1/gain","value":0,"ramp_ms":-1}"#,
            r#"{"op":"set","address":"input/1/gain","value":0,"ramp_ms":"soon"}"#,
            r#"{"op":"scene"}"#,
            r#"{"op":"stop","now":true}"#,
            r#"{"op":"learn"}"#,
            r#"{"op":"unlearn","target":"x","y":1}"#,
            r#"{"op":"scene","name":"Abend","path":"x"}"#,
        ] {
            assert!(serde_json::from_str::<Op>(json).is_err(), "{json}");
        }
    }
}
