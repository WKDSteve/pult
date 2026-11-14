//! The messages of the control interface: what a client reads of a running mixer and what it
//! may ask of it. Plain data without I/O, so a client (takt, a test) can use it without any
//! device code. `docs/CONTROL.md` describes it for readers.
//!
//! Parameters go by their addresses ([`Binding`]). There is no way to name a file here: a
//! client can save the profile the route came from, but not write anywhere else.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::binding::{BandParam, Binding, BusParam, InputParam, Value};
use crate::control::{Command, Target};
use crate::eq::{Band, BandChange, BandKind};
use crate::limiter::{LimitChange, LimitSettings};
use crate::multiband::MultibandParam;
use crate::strip::StripChange;
use crate::view::{Action, BusView, EventKind, EventView, GAIN_RANGE_DB, InputView, MeterPoint};
use crate::view::{MixerView, SourceKind, check_amount};

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
    /// Someone talks: the talkback microphones are open and the monitor buses dimmed.
    #[serde(default)]
    pub talk: bool,
    /// Solo on a monitor bus listens before the fader (PFL) instead of after it (AFL).
    #[serde(default)]
    pub pfl: bool,
}

/// Where the input meters measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MeterPlace {
    /// Before gate, compressor, gain and mute.
    Pre,
    /// After all of those.
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
#[allow(clippy::struct_excessive_bools)] // Switches and states of a channel strip, each on its own.
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
    /// Its gate as one knob, 0 (off) to 10; 0 from a run older than gates.
    #[serde(default)]
    pub gate: f32,
    /// Its compressor as one knob, 0 (off) to 10; 0 from a run older than compressors.
    #[serde(default)]
    pub comp: f32,
    /// In the automixer.
    #[serde(default)]
    pub automix: bool,
    /// Its priority in the automixer in dB.
    #[serde(default)]
    pub automix_weight_db: f32,
    /// What the automixer gives it now in dB (0 outside it).
    #[serde(default)]
    pub automix_gain_db: f32,
    /// The most its gate took away since the previous state, in dB; 0 when open or off.
    #[serde(default)]
    pub gate_reduction_db: f32,
    /// Whether its gate is open (or off); true from a run older than this.
    #[serde(default = "open")]
    pub gate_open: bool,
    /// The most its compressor took away since the previous state, in dB.
    #[serde(default)]
    pub comp_reduction_db: f32,
    /// Its channel strip; neutral from a run older than this.
    #[serde(default)]
    pub strip: crate::strip::StripSettings,
    /// A talkback microphone: silent until someone talks.
    #[serde(default)]
    pub talkback: bool,
    /// Whether its equalizer is on; on from a run older than this, where it has no bands.
    #[serde(default = "open")]
    pub eq_on: bool,
    /// The bands of its equalizer; none from a run older than this.
    #[serde(default)]
    pub eq: Vec<Band>,
    /// Its equalizer comes before gate and compressor instead of after them.
    #[serde(default)]
    pub eq_pre: bool,
    /// Whether its limiter is on.
    #[serde(default)]
    pub limit_on: bool,
    /// The ceiling of its limiter in dB.
    #[serde(default)]
    pub limit_db: f32,
    /// How far its limiter turned down at most since the previous state, in dB.
    #[serde(default)]
    pub limited_db: f32,
    /// Its sidechain; neutral from a run older than it. `key` is the number of the input its
    /// gate and compressor listen to, 0 for itself.
    #[serde(default)]
    pub sidechain: crate::sidechain::SidechainSettings,
    /// Its de-esser; off from a run older than it.
    #[serde(default)]
    pub deess: crate::deess::DeessSettings,
    /// The most its de-esser took from the band of the sibilants since the previous state, in dB.
    #[serde(default)]
    pub deess_reduction_db: f32,
    /// Its ducker; off from a run older than ducking. `key` is the number of the input that
    /// ducks it, 0 for none.
    #[serde(default)]
    pub duck: crate::duck::DuckSettings,
    /// The most its ducker took away since the previous state, in dB.
    #[serde(default)]
    pub duck_reduction_db: f32,
    /// The names of the buses it feeds.
    pub buses: Vec<String>,
    /// Its send to every bus, in bus order; empty from a run older than sends.
    #[serde(default)]
    pub sends: Vec<SendState>,
    /// Whether the device or program is there.
    pub present: bool,
    /// The highest peak since the previous state in dBFS; `None` for silence.
    pub peak_db: Option<f32>,
    /// The resampler's correction against clock drift in ppm.
    pub drift_ppm: f32,
    /// Times it ran dry or resynced so far.
    pub dropouts: u64,
}

/// The loudness of a bus after EBU R128; each `null` until measured.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct LoudnessState {
    /// Momentary loudness (400 ms) in LUFS.
    pub momentary_lufs: Option<f32>,
    /// Short-term loudness (3 s) in LUFS.
    pub short_term_lufs: Option<f32>,
    /// Integrated loudness since the start in LUFS.
    pub integrated_lufs: Option<f32>,
    /// Loudness range in LU.
    pub range_lu: Option<f32>,
    /// Highest true peak since the start in dBTP.
    pub true_peak_db: Option<f32>,
}

/// The send of an input to one bus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendState {
    /// The name of the bus.
    pub bus: String,
    /// Level in dB.
    pub gain_db: f32,
    /// Taken before the input's fader.
    pub pre: bool,
}

/// What of one input waits before a bus takes it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RingState {
    /// The number of the input, from 1, as in the addresses (`input/2/gain`).
    pub input: usize,
    /// What waits in its ring to this bus, in milliseconds, averaged.
    pub ms: f32,
}

/// One bus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // Switches of a console, each on its own.
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
    /// Whether its equalizer is switched on; off from a run older than equalizers.
    #[serde(default)]
    pub eq_on: bool,
    /// The bands of its equalizer, as in the addresses from 1 (`bus/A1/eq/2/gain`); none from
    /// a run older than equalizers.
    #[serde(default)]
    pub eq: Vec<Band>,
    /// Its limiter, as the fields `limit_on`, `limit_db` and `limit_lookahead_ms`; off and zero
    /// from a run older than limiters ([`LimitSettings::OLDER_RUN`]).
    #[serde(flatten)]
    pub limit: LimitSettings,
    /// The monitor bus, the one solo acts on; false from a run older than this.
    #[serde(default)]
    pub monitor: bool,
    /// A mix-minus bus: the number of the input it is for (from 1), which never feeds it.
    #[serde(default)]
    pub minus: Option<usize>,
    /// Its loudness after EBU R128.
    #[serde(default)]
    pub loudness: LoudnessState,
    /// Its compressor as one knob, 0 (off) to 10.
    #[serde(default)]
    pub comp: f32,
    /// The mix of its compressor in percent.
    #[serde(default = "full_mix")]
    pub comp_mix: f32,
    /// The corner of the sidechain high-pass of its compressor in Hz.
    #[serde(default = "lowest_hpf")]
    pub comp_hpf_hz: f32,
    /// Its delay in milliseconds; none from a run older than bus delays.
    #[serde(default)]
    pub delay_ms: f32,
    /// How much later sound leaves the bus than it was mixed, in milliseconds: the lookahead
    /// of its limiter while that is on, plus `delay_ms`. 0 from a run older than this.
    #[serde(default)]
    pub latency_ms: f32,
    /// Per input that feeds it, what waits in the ring between them; none from a run older
    /// than this, and none from a desk that does not measure.
    #[serde(default)]
    pub rings: Vec<RingState>,
    /// How long its device says sound takes from the callback to the output, in
    /// milliseconds, averaged; `null` where the device does not say, while it is gone, and
    /// from a run older than this.
    #[serde(default)]
    pub device_latency_ms: Option<f32>,
    /// Its dim switch.
    #[serde(default)]
    pub dim: bool,
    /// How far dim turns it down, in dB.
    #[serde(default = "dim_level")]
    pub dim_db: f32,
    /// The most its compressor took away since the previous state, in dB.
    #[serde(default)]
    pub comp_reduction_db: f32,
    /// Its multiband compressor; off from a run older than it.
    #[serde(default)]
    pub multiband: crate::multiband::MultibandSettings,
    /// The most each of its bands took away since the previous state, in dB, from the bottom.
    #[serde(default)]
    pub multiband_reduction_db: [f32; crate::multiband::BANDS],
    /// How far its limiter turned down at most since the previous state, in dB; 0 when it did not.
    #[serde(default)]
    pub limited_db: f32,
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
                .map(|(index, input)| InputState::new(index, input, &bus_name, view.buses.len()))
                .collect(),
            buses: view.buses.iter().map(BusState::new).collect(),
            events,
            scenes: view.scenes.clone(),
            midi: view.midi.clone(),
            talk: view.talk,
            pfl: view.pfl,
        }
    }
}

/// A bus compressor is all compressed in a state that does not say.
fn full_mix() -> f32 {
    100.0
}

/// Its sidechain filter is at its lowest in a state that does not say.
fn lowest_hpf() -> f32 {
    crate::buscomp::HPF_HZ.0
}

/// A gate counts as open in a state that does not say.
fn open() -> bool {
    true
}

impl InputState {
    fn new(
        index: usize,
        input: &InputView,
        bus_name: &impl Fn(&usize) -> Option<String>,
        buses: usize,
    ) -> Self {
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
            gate: input.gate,
            comp: input.comp,
            automix: input.automix,
            automix_weight_db: input.automix_weight_db,
            automix_gain_db: input.automix_gain_db,
            gate_reduction_db: input.gate_reduction_db,
            gate_open: input.gate_open,
            comp_reduction_db: input.comp_reduction_db,
            strip: input.strip,
            talkback: input.talkback,
            eq_on: input.eq_on,
            eq: input.eq.to_vec(),
            eq_pre: input.eq_pre,
            limit_on: input.limit_on,
            limit_db: input.limit_db,
            limited_db: input.limited_db,
            sidechain: input.sidechain,
            deess: input.deess,
            deess_reduction_db: input.deess_reduction_db,
            duck: input.duck,
            duck_reduction_db: input.duck_reduction_db,
            buses: input.buses.iter().filter_map(bus_name).collect(),
            sends: (0..buses)
                .filter_map(|bus| {
                    let send = input.send(bus);
                    Some(SendState {
                        bus: bus_name(&bus)?,
                        gain_db: send.db,
                        pre: send.pre,
                    })
                })
                .collect(),
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
            eq_on: bus.eq_on,
            eq: bus.eq.to_vec(),
            limit: bus.limit,
            monitor: bus.monitor,
            minus: bus.minus.map(|input| input + 1),
            comp: bus.comp,
            comp_mix: bus.comp_mix,
            comp_hpf_hz: bus.comp_hpf_hz,
            delay_ms: bus.delay_ms,
            latency_ms: bus.latency_ms(),
            rings: bus
                .rings
                .iter()
                .map(|ring| RingState {
                    input: ring.input + 1,
                    ms: ring.ms,
                })
                .collect(),
            device_latency_ms: bus.device_latency_ms,
            dim: bus.dim,
            dim_db: bus.dim_db,
            comp_reduction_db: bus.comp_reduction_db,
            multiband: bus.multiband,
            multiband_reduction_db: bus.multiband_reduction_db,
            loudness: LoudnessState {
                momentary_lufs: bus.loudness.momentary_lufs,
                short_term_lufs: bus.loudness.short_term_lufs,
                integrated_lufs: bus.loudness.integrated_lufs,
                range_lu: bus.loudness.range_lu,
                true_peak_db: bus.loudness.true_peak_db,
            },
            limited_db: bus.limited_db,
            present: bus.present,
            peak_db: bus.peak_db,
            clipped: bus.clipped,
        }
    }
}

/// A new value for a parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OpValue {
    /// For a mute, a solo, a crosspoint or the switch of an equalizer.
    Switch(bool),
    /// For a gain in dB, a pan, the one knob of a gate or a compressor, or a number of a band.
    Number(f32),
    /// For the kind of a band: `low`, `bell` or `high`.
    Word(String),
}

/// What a client asks of the mixer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Op {
    /// Sets the parameter at `address`.
    Set {
        /// For example `input/2/gain`, `bus/A1/mute`, `bus/A1/eq/2/gain` or `matrix/2/A1`.
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
        match command {
            Command::Scene(name) => return Some(Self::Scene { name: name.clone() }),
            Command::Learn(target) => {
                return Some(Self::Learn {
                    target: target.clone(),
                });
            }
            Command::Unlearn(target) => {
                return Some(Self::Unlearn {
                    target: target.clone(),
                });
            }
            _ => {}
        }
        let (place, param, value, ramp_ms) = set_of(command)?;
        let address = match place {
            Place::Input(input) => format!("input/{}/{param}", input + 1),
            Place::Bus(to) => format!("bus/{}/{param}", bus(to)?),
            Place::Crossing(input, to) => format!("{param}/{}/{}", input + 1, bus(to)?),
            Place::Route => param,
        };
        Some(Self::Set {
            address,
            value,
            ramp_ms: (ramp_ms > 0).then_some(ramp_ms),
        })
    }

    /// The action this asks for in `view`.
    ///
    /// # Errors
    ///
    /// A message when the address names nothing in `view`, its parameter is only read, the
    /// value is of the wrong kind or out of range, or a ramp is too long or asked of
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
                let value = value_for(binding, value, address)?;
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

/// Where a parameter that a command sets lives.
enum Place {
    /// At an input: `input/<n>/<param>`.
    Input(usize),
    /// At a bus: `bus/<name>/<param>`.
    Bus(usize),
    /// At the crossing of an input and a bus: `<param>/<n>/<bus>`.
    Crossing(usize, usize),
    /// At the route itself: `<param>`.
    Route,
}

impl From<Target> for Place {
    fn from(target: Target) -> Self {
        match target {
            Target::Input(index) => Self::Input(index),
            Target::Bus(index) => Self::Bus(index),
        }
    }
}

/// What a command sets: where, the words of its parameter, the value, and the time it takes.
/// `None` for a command that sets no parameter, and for the lookahead of the limiter of an
/// input, which has none.
fn set_of(command: &Command) -> Option<(Place, String, OpValue, u32)> {
    use OpValue::{Number, Switch};
    use Place::{Bus, Crossing, Input, Route};
    let at = |place: Place, param: &str, value: OpValue| (place, param.to_owned(), value, 0);
    let limit = |place: Place, change: LimitChange| match change {
        LimitChange::Switch(on) => at(place, "limit", Switch(on)),
        LimitChange::Ceiling(db) => at(place, "limit/ceiling", Number(db)),
        LimitChange::Lookahead(ms) => at(place, "limit/lookahead", Number(ms)),
    };
    let band = |place: Place, band: usize, change: BandChange| {
        let (setting, value) = band_op(change);
        (place, format!("eq/{}/{setting}", band + 1), value, 0)
    };
    Some(match *command {
        Command::Gain(to, db, ramp_ms) => (to.into(), "gain".to_owned(), Number(db), ramp_ms),
        Command::Mute(to, muted) => at(to.into(), "mute", Switch(muted)),
        Command::Solo(input, solo) => at(Input(input), "solo", Switch(solo)),
        Command::Pan(input, pan, ramp_ms) => (Input(input), "pan".to_owned(), Number(pan), ramp_ms),
        Command::Gate(input, amount) => at(Input(input), "gate", Number(amount)),
        Command::Comp(input, amount) => at(Input(input), "comp", Number(amount)),
        Command::Strip(input, change) => {
            let (param, value) = strip_op(change);
            at(Input(input), param, value)
        }
        Command::Duck(input, change) => at(
            Input(input),
            change.param().address(),
            Number(change.number()),
        ),
        Command::Sidechain(input, change) => at(
            Input(input),
            change.param().address(),
            Number(change.number()),
        ),
        Command::Deess(input, change) => at(
            Input(input),
            change.param().address(),
            Number(change.number()),
        ),
        Command::Automix(input, on) => at(Input(input), "automix", Switch(on)),
        Command::AutomixWeight(input, db) => at(Input(input), "automix/weight", Number(db)),
        Command::Talkback(input, on) => at(Input(input), "talkback", Switch(on)),
        Command::InputEq(input, on) => at(Input(input), "eq", Switch(on)),
        Command::InputEqPre(input, pre) => at(Input(input), "eq/pre", Switch(pre)),
        Command::InputEqBand(input, index, change) => band(Input(input), index, change),
        Command::InputLimit(input, change @ (LimitChange::Switch(_) | LimitChange::Ceiling(_))) => {
            limit(Input(input), change)
        }
        Command::Eq(to, on) => at(Bus(to), "eq", Switch(on)),
        Command::EqBand(to, index, change) => band(Bus(to), index, change),
        Command::Limit(to, change) => limit(Bus(to), change),
        Command::BusComp(to, amount) => at(Bus(to), "comp", Number(amount)),
        Command::BusCompMix(to, percent) => at(Bus(to), "comp/mix", Number(percent)),
        Command::BusCompHpf(to, hz) => at(Bus(to), "comp/hpf", Number(hz)),
        Command::Multiband(to, change) => at(
            Bus(to),
            &format!("mb/{}", change.param().word()),
            Number(change.number()),
        ),
        Command::BusDelay(to, ms) => at(Bus(to), "delay", Number(ms)),
        Command::Monitor(to, on) => at(Bus(to), "monitor", Switch(on)),
        Command::Dim(to, on) => at(Bus(to), "dim", Switch(on)),
        Command::DimLevel(to, db) => at(Bus(to), "dim/level", Number(db)),
        Command::Route(input, to, on) => at(Crossing(input, to), "matrix", Switch(on)),
        Command::Send(input, to, db, ramp_ms) => {
            (Crossing(input, to), "send".to_owned(), Number(db), ramp_ms)
        }
        Command::SendPre(input, to, pre) => at(Crossing(input, to), "sendpre", Switch(pre)),
        Command::Talk(on) => at(Route, "talk", Switch(on)),
        Command::Pfl(on) => at(Route, "pfl", Switch(on)),
        // The limiter of an input has no lookahead.
        Command::InputLimit(_, LimitChange::Lookahead(_))
        | Command::Scene(_)
        | Command::Learn(_)
        | Command::Unlearn(_)
        | Command::Scenes
        | Command::Show
        | Command::Save(_)
        | Command::Help
        | Command::Stop => return None,
    })
}

/// The address word and the value of a change of a band.
fn band_op(change: BandChange) -> (&'static str, OpValue) {
    match change {
        BandChange::Kind(kind) => ("kind", OpValue::Word(kind.word().to_owned())),
        BandChange::Freq(hz) => ("freq", OpValue::Number(hz)),
        BandChange::Gain(db) => ("gain", OpValue::Number(db)),
        BandChange::Q(q) => ("q", OpValue::Number(q)),
        BandChange::Threshold(db) => ("threshold", OpValue::Number(db)),
    }
}

/// `value` as a setting of a band of an equalizer, checked.
fn band_value(param: BandParam, value: &OpValue, address: &str) -> Result<Value, String> {
    let checked = |change: BandChange, value: Value| {
        change
            .checked()
            .map(|_| value)
            .map_err(|why| format!("{address}: {why}"))
    };
    match (param, value) {
        (BandParam::Kind, OpValue::Word(word)) => BandKind::from_word(word)
            .map(Value::Kind)
            .ok_or_else(|| format!("{address} takes low, bell or high, not \"{word}\"")),
        (BandParam::Kind, _) => Err(format!("{address} takes low, bell or high")),
        (_, OpValue::Number(number)) if !number.is_finite() => {
            Err(format!("{address} takes a finite number"))
        }
        (BandParam::Gain, OpValue::Number(db)) => checked(BandChange::Gain(*db), Value::Boost(*db)),
        (BandParam::Freq, OpValue::Number(hz)) => checked(BandChange::Freq(*hz), Value::Freq(*hz)),
        (BandParam::Q, OpValue::Number(q)) => checked(BandChange::Q(*q), Value::Width(*q)),
        (BandParam::Threshold, OpValue::Number(db)) => {
            checked(BandChange::Threshold(*db), Value::Threshold(*db))
        }
        _ => Err(format!("{address} takes a number")),
    }
}

/// Whether `binding` is a switch.
fn is_switch(binding: Binding) -> bool {
    use BusParam as B;
    use InputParam as I;
    matches!(
        binding,
        Binding::Input(
            _,
            I::Mute
                | I::Solo
                | I::Automix
                | I::Polarity
                | I::Talkback
                | I::Eq
                | I::EqPre
                | I::Limit
        ) | Binding::Bus(_, B::Mute | B::Eq | B::Limit | B::Monitor | B::Dim)
            | Binding::Route { .. }
            | Binding::SendPre { .. }
            | Binding::Talk
            | Binding::Pfl
    )
}

/// The dim level a bus has when a state does not say.
fn dim_level() -> f32 {
    crate::view::DEFAULT_DIM_DB
}

/// Whether [`setting_value`] checks the number of `binding`.
fn is_setting(binding: Binding) -> bool {
    matches!(
        binding,
        Binding::Bus(
            _,
            BusParam::CompMix
                | BusParam::CompHpf
                | BusParam::Delay
                | BusParam::Multiband(_)
                | BusParam::DimLevel,
        ) | Binding::Input(
            _,
            InputParam::Hpf | InputParam::Lpf | InputParam::Delay | InputParam::Width,
        )
    )
}

/// The address word and the value of a change of a channel strip.
fn strip_op(change: StripChange) -> (&'static str, OpValue) {
    match change {
        StripChange::Polarity(on) => ("polarity", OpValue::Switch(on)),
        StripChange::Hpf(hz) => ("hpf", OpValue::Number(hz)),
        StripChange::Lpf(hz) => ("lpf", OpValue::Number(hz)),
        StripChange::Delay(ms) => ("delay", OpValue::Number(ms)),
        StripChange::Width(percent) => ("width", OpValue::Number(percent)),
    }
}

/// A number setting of a channel strip, or the mix (percent) or the sidechain corner (Hz) of a
/// bus compressor, checked.
fn setting_value(binding: Binding, number: f32, address: &str) -> Result<Value, String> {
    let strip = |change: StripChange, value: Value| {
        change
            .checked()
            .map(|_| value)
            .map_err(|why| format!("{address}: {why}"))
    };
    match binding {
        Binding::Bus(_, BusParam::DimLevel) => crate::view::check_dim(number)
            .map(Value::Gain)
            .map_err(|why| format!("{address}: {why}")),
        Binding::Bus(_, BusParam::Multiband(param)) => param
            .change(number)
            .checked()
            .map(|_| match param {
                MultibandParam::Band(_) => Value::Amount(number),
                _ => Value::Freq(number),
            })
            .map_err(|why| format!("{address}: {why}")),
        Binding::Input(_, InputParam::AutomixWeight) if (-12.0..=12.0).contains(&number) => {
            Ok(Value::Gain(number))
        }
        Binding::Input(_, InputParam::AutomixWeight) => {
            Err(format!("{address} takes -12 to 12 dB, not {number}"))
        }
        Binding::Input(_, InputParam::Hpf) => strip(StripChange::Hpf(number), Value::Freq(number)),
        Binding::Input(_, InputParam::Lpf) => strip(StripChange::Lpf(number), Value::Freq(number)),
        Binding::Input(_, InputParam::Delay) => {
            strip(StripChange::Delay(number), Value::Delay(number))
        }
        Binding::Input(..) => strip(StripChange::Width(number), Value::Percent(number)),
        Binding::Bus(_, BusParam::CompMix) if (0.0..=100.0).contains(&number) => {
            Ok(Value::Percent(number))
        }
        Binding::Bus(_, BusParam::CompMix) => {
            Err(format!("{address} takes 0 to 100, not {number}"))
        }
        Binding::Bus(_, BusParam::Delay) => crate::busdelay::check_delay(number)
            .map(Value::Delay)
            .map_err(|why| format!("{address}: {why}")),
        _ => {
            let (low, high) = crate::buscomp::HPF_HZ;
            if (low..=high).contains(&number) {
                Ok(Value::Freq(number))
            } else {
                Err(format!("{address} takes {low} to {high} Hz, not {number}"))
            }
        }
    }
}

/// `value` as the kind of value `binding` takes.
fn value_for(binding: Binding, value: &OpValue, address: &str) -> Result<Value, String> {
    use BusParam::EqBand;
    let wants_number = |number: f32| {
        if number.is_finite() {
            Ok(number)
        } else {
            Err(format!("{address} takes a finite number"))
        }
    };
    let limit = |change: LimitChange| change.checked().map_err(|why| format!("{address}: {why}"));
    match (binding, value) {
        (Binding::Input(_, InputParam::Meter) | Binding::Bus(_, BusParam::Meter), _) => {
            Err(format!("{address} can only be read"))
        }
        (
            Binding::Input(_, InputParam::Gain)
            | Binding::Bus(_, BusParam::Gain)
            | Binding::Send { .. },
            OpValue::Number(db),
        ) => {
            let (low, high) = GAIN_RANGE_DB;
            let db = wants_number(*db)?;
            if (low..=high).contains(&db) {
                Ok(Value::Gain(db))
            } else {
                Err(format!("gain {db} dB is outside {low} to {high} dB"))
            }
        }
        (Binding::Input(_, InputParam::AutomixWeight), OpValue::Number(db)) => {
            setting_value(binding, wants_number(*db)?, address)
        }
        (Binding::Input(_, InputParam::Pan), OpValue::Number(pan)) => {
            Ok(Value::Pan(wants_number(*pan)?))
        }
        (binding, OpValue::Number(n)) if is_setting(binding) => {
            setting_value(binding, wants_number(*n)?, address)
        }
        (Binding::Input(_, InputParam::Gate), OpValue::Number(amount)) => {
            check_amount("gate", wants_number(*amount)?).map(Value::Amount)
        }
        (Binding::Input(_, InputParam::Duck(duck)), OpValue::Number(number)) => duck
            .change(wants_number(*number)?)
            .checked()
            .map(Value::Duck)
            .map_err(|why| format!("{address}: {why}")),
        (Binding::Input(_, InputParam::Deess(param)), OpValue::Number(number)) => param
            .change(wants_number(*number)?)
            .checked()
            .map(Value::Deess)
            .map_err(|why| format!("{address}: {why}")),
        (Binding::Input(_, InputParam::Sidechain(param)), OpValue::Number(number)) => param
            .change(wants_number(*number)?)
            .checked()
            .map(Value::Sidechain)
            .map_err(|why| format!("{address}: {why}")),
        (
            Binding::Input(_, InputParam::Comp) | Binding::Bus(_, BusParam::Comp),
            OpValue::Number(amount),
        ) => check_amount("comp", wants_number(*amount)?).map(Value::Amount),
        (
            Binding::Bus(_, EqBand(_, param)) | Binding::Input(_, InputParam::EqBand(_, param)),
            value,
        ) => band_value(param, value, address),
        (
            Binding::Bus(_, BusParam::LimitCeiling) | Binding::Input(_, InputParam::LimitCeiling),
            OpValue::Number(db),
        ) => limit(LimitChange::Ceiling(wants_number(*db)?)).map(|_| Value::Ceiling(*db)),
        (Binding::Bus(_, BusParam::LimitLookahead), OpValue::Number(ms)) => {
            limit(LimitChange::Lookahead(wants_number(*ms)?)).map(|_| Value::Lookahead(*ms))
        }
        (binding, OpValue::Switch(on)) if is_switch(binding) => Ok(Value::Switch(*on)),
        (binding, _) if is_switch(binding) => Err(format!("{address} takes true or false")),
        _ => Err(format!("{address} takes a number")),
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
            action(r#"{"op":"set","address":"input/2/gate","value":3}"#),
            Ok(Action::InputGate {
                input: 1,
                amount: 3.0
            })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"input/2/comp","value":0}"#),
            Ok(Action::InputComp {
                input: 1,
                amount: 0.0
            })
        );
        let limit = |change| Ok(Action::BusLimit { bus: 1, change });
        assert_eq!(
            action(r#"{"op":"set","address":"bus/A2/limit","value":false}"#),
            limit(LimitChange::Switch(false))
        );
        assert_eq!(
            action(r#"{"op":"set","address":"bus/A2/limit/ceiling","value":-6}"#),
            limit(LimitChange::Ceiling(-6.0))
        );
        assert_eq!(
            action(r#"{"op":"set","address":"bus/A2/limit/lookahead","value":2.5}"#),
            limit(LimitChange::Lookahead(2.5))
        );
        assert_eq!(
            action(r#"{"op":"set","address":"bus/A2/eq","value":false}"#),
            Ok(Action::BusEq { bus: 1, on: false })
        );
        let band = |change| {
            Ok(Action::BusEqBand {
                bus: 0,
                band: 1,
                change,
            })
        };
        assert_eq!(
            action(r#"{"op":"set","address":"bus/A1/eq/2/gain","value":-4.5}"#),
            band(BandChange::Gain(-4.5))
        );
        assert_eq!(
            action(r#"{"op":"set","address":"bus/A1/eq/2/freq","value":1000}"#),
            band(BandChange::Freq(1000.0))
        );
        assert_eq!(
            action(r#"{"op":"set","address":"bus/A1/eq/2/q","value":2}"#),
            band(BandChange::Q(2.0))
        );
        assert_eq!(
            action(r#"{"op":"set","address":"bus/A1/eq/2/kind","value":"HIGH"}"#),
            band(BandChange::Kind(BandKind::HighShelf))
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
    fn the_equalizer_and_the_limiter_of_an_input_are_set_by_address() {
        assert_eq!(
            action(r#"{"op":"set","address":"input/2/eq","value":false}"#),
            Ok(Action::InputEq {
                input: 1,
                on: false
            })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"input/2/eq/3/gain","value":-4.5}"#),
            Ok(Action::InputEqBand {
                input: 1,
                band: 2,
                change: BandChange::Gain(-4.5)
            })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"input/1/eq/1/kind","value":"bell"}"#),
            Ok(Action::InputEqBand {
                input: 0,
                band: 0,
                change: BandChange::Kind(BandKind::Peak)
            })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"input/1/eq/pre","value":true}"#),
            Ok(Action::InputEqPre {
                input: 0,
                pre: true
            })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"input/1/limit","value":true}"#),
            Ok(Action::InputLimit {
                input: 0,
                change: LimitChange::Switch(true)
            })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"input/1/limit/ceiling","value":-6}"#),
            Ok(Action::InputLimit {
                input: 0,
                change: LimitChange::Ceiling(-6.0)
            })
        );
        assert!(action(r#"{"op":"set","address":"input/1/eq/9/gain","value":1}"#).is_err());
        assert!(action(r#"{"op":"set","address":"input/1/eq/2/gain","value":99}"#).is_err());
        assert!(action(r#"{"op":"set","address":"input/1/limit/ceiling","value":3}"#).is_err());
        assert!(action(r#"{"op":"set","address":"input/1/limit/lookahead","value":1}"#).is_err());
    }

    #[test]
    fn dim_and_talkback_are_set_by_address() {
        assert_eq!(
            action(r#"{"op":"set","address":"talk","value":true}"#),
            Ok(Action::Talk { on: true })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"bus/A1/dim","value":true}"#),
            Ok(Action::BusDim { bus: 0, on: true })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"bus/A1/dim/level","value":12}"#),
            Ok(Action::BusDimLevel { bus: 0, db: 12.0 })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"input/2/talkback","value":true}"#),
            Ok(Action::InputTalkback { input: 1, on: true })
        );
        assert!(action(r#"{"op":"set","address":"bus/A1/dim/level","value":90}"#).is_err());
        assert!(action(r#"{"op":"set","address":"talk","value":1}"#).is_err());
        assert_eq!(
            action(r#"{"op":"set","address":"pfl","value":true}"#),
            Ok(Action::Pfl { on: true })
        );
    }

    #[test]
    fn a_multiband_compressor_is_set_by_address() {
        assert_eq!(
            action(r#"{"op":"set","address":"bus/A1/mb/mid","value":4}"#),
            Ok(Action::BusMultiband {
                bus: 0,
                change: crate::multiband::MultibandChange::Band(1, 4.0)
            })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"bus/A1/mb/xhigh","value":3000}"#),
            Ok(Action::BusMultiband {
                bus: 0,
                change: crate::multiband::MultibandChange::HighSplit(3000.0)
            })
        );
        assert!(action(r#"{"op":"set","address":"bus/A1/mb/low","value":11}"#).is_err());
        assert!(action(r#"{"op":"set","address":"bus/A1/mb/xlow","value":5}"#).is_err());
        assert!(action(r#"{"op":"set","address":"bus/A1/mb/top","value":1}"#).is_err());
    }

    #[test]
    fn sends_and_the_automixer_are_set_by_address() {
        assert_eq!(
            action(r#"{"op":"set","address":"input/1/polarity","value":true}"#),
            Ok(Action::InputStrip {
                input: 0,
                change: StripChange::Polarity(true)
            })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"input/2/hpf","value":80}"#),
            Ok(Action::InputStrip {
                input: 1,
                change: StripChange::Hpf(80.0)
            })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"input/2/delay","value":5}"#),
            Ok(Action::InputStrip {
                input: 1,
                change: StripChange::Delay(5.0)
            })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"input/1/width","value":0}"#),
            Ok(Action::InputStrip {
                input: 0,
                change: StripChange::Width(0.0)
            })
        );
        assert!(action(r#"{"op":"set","address":"input/1/lpf","value":50}"#).is_err());
        assert!(action(r#"{"op":"set","address":"input/1/polarity","value":3}"#).is_err());
        assert_eq!(
            action(r#"{"op":"set","address":"bus/A1/comp","value":4}"#),
            Ok(Action::BusComp {
                bus: 0,
                amount: 4.0
            })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"bus/A1/comp/mix","value":40}"#),
            Ok(Action::BusCompMix {
                bus: 0,
                percent: 40.0
            })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"bus/A1/comp/hpf","value":120}"#),
            Ok(Action::BusCompHpf { bus: 0, hz: 120.0 })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"bus/A1/delay","value":12.5}"#),
            Ok(Action::BusDelay { bus: 0, ms: 12.5 })
        );
        assert!(action(r#"{"op":"set","address":"bus/A1/delay","value":900}"#).is_err());
        assert_eq!(
            action(r#"{"op":"set","address":"bus/A1/delay","value":0}"#),
            Ok(Action::BusDelay { bus: 0, ms: 0.0 })
        );
        assert!(action(r#"{"op":"set","address":"bus/A1/comp/hpf","value":5}"#).is_err());
        assert_eq!(
            action(r#"{"op":"set","address":"send/2/A1","value":-6,"ramp_ms":200}"#),
            Ok(Action::Send {
                input: 1,
                bus: 0,
                db: -6.0,
                ramp_ms: 200
            })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"sendpre/2/A1","value":true}"#),
            Ok(Action::SendPre {
                input: 1,
                bus: 0,
                pre: true
            })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"input/1/automix","value":true}"#),
            Ok(Action::InputAutomix { input: 0, on: true })
        );
        assert_eq!(
            action(r#"{"op":"set","address":"input/1/automix/weight","value":6}"#),
            Ok(Action::InputAutomixWeight { input: 0, db: 6.0 })
        );
        assert!(action(r#"{"op":"set","address":"input/1/automix/weight","value":20}"#).is_err());
    }

    #[test]
    fn what_a_limiter_reports_survives_json() {
        let mut limiter = crate::limiter::Limiter::new(48_000, 1);
        let controls = crate::limiter::LimiterControls::default();
        controls.set_all(crate::limiter::LimitSettings {
            on: true,
            ceiling_db: -6.0,
            lookahead_ms: 0.0,
        });
        let mut view = view();
        view.buses[0].limited_db = limiter.process(&controls, &mut [f32::INFINITY], 1);
        let text = serde_json::to_string(&State::new(&view, Vec::new())).expect("to JSON");
        let read: State = serde_json::from_str(&text).expect("from JSON");
        assert!(read.buses[0].limited_db.is_finite());
    }

    #[test]
    fn a_command_of_a_limiter_becomes_the_op_for_its_address() {
        let view = view();
        let state = State::new(&view, Vec::new());
        for change in [
            LimitChange::Switch(false),
            LimitChange::Ceiling(-6.0),
            LimitChange::Lookahead(2.0),
        ] {
            let op = Op::of_command(&Command::Limit(1, change), &state).expect("an op");
            assert_eq!(op.action(&view), Ok(Action::BusLimit { bus: 1, change }));
        }
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
            action(&Command::Solo(1, true)),
            Ok(Action::InputSolo {
                input: 1,
                solo: true
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
            action(&Command::Gate(1, 2.5)),
            Ok(Action::InputGate {
                input: 1,
                amount: 2.5
            })
        );
        assert_eq!(
            action(&Command::Comp(0, 10.0)),
            Ok(Action::InputComp {
                input: 0,
                amount: 10.0
            })
        );
        assert_eq!(
            action(&Command::Eq(1, false)),
            Ok(Action::BusEq { bus: 1, on: false })
        );
        for change in [
            BandChange::Kind(BandKind::LowShelf),
            BandChange::Freq(250.0),
            BandChange::Gain(-3.0),
            BandChange::Q(1.5),
        ] {
            assert_eq!(
                action(&Command::EqBand(0, 3, change)),
                Ok(Action::BusEqBand {
                    bus: 0,
                    band: 3,
                    change
                })
            );
        }
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
                .contains("can ramp")
        );
        assert!(
            refused(r#"{"op":"set","address":"matrix/1/A1","value":true,"ramp_ms":100}"#)
                .contains("can ramp")
        );
    }

    #[test]
    fn a_bus_says_what_holds_its_sound_back_and_reads_without_it() {
        let mut view = view();
        view.buses[0].delay_ms = 10.0;
        view.buses[0].rings = vec![
            crate::view::RingView { input: 0, ms: 20.5 },
            crate::view::RingView { input: 2, ms: 19.0 },
        ];
        view.buses[0].device_latency_ms = Some(31.0);
        let state = State::new(&view, Vec::new());
        let json = serde_json::to_value(&state).expect("to JSON");
        let bus = &json["buses"][0];
        // The demo bus has its limiter on with 1 ms of lookahead.
        assert_eq!(bus["latency_ms"], serde_json::json!(11.0));
        assert_eq!(
            bus["rings"],
            serde_json::json!([{ "input": 1, "ms": 20.5 }, { "input": 3, "ms": 19.0 }])
        );
        assert_eq!(bus["device_latency_ms"], serde_json::json!(31.0));
        assert_eq!(
            json["buses"][1]["device_latency_ms"],
            serde_json::Value::Null
        );
        // A run older than these figures says nothing of them.
        let mut older = json.clone();
        let bus = older["buses"][0].as_object_mut().expect("a bus");
        for key in ["latency_ms", "rings", "device_latency_ms"] {
            assert!(bus.remove(key).is_some(), "{key}");
        }
        let read: State = serde_json::from_value(older).expect("from JSON");
        assert_eq!(read.buses[0].latency_ms, 0.0);
        assert_eq!(read.buses[0].rings, Vec::new());
        assert_eq!(read.buses[0].device_latency_ms, None);
    }

    #[test]
    fn the_limiter_keeps_its_three_loose_fields_in_json() {
        let state = State::new(&view(), Vec::new());
        let json = serde_json::to_value(&state).expect("to JSON");
        let bus = json["buses"][0].as_object().expect("a bus");
        assert!(!bus.contains_key("limit"), "{bus:?}");
        assert_eq!(bus["limit_on"], serde_json::json!(true));
        assert_eq!(
            bus["limit_db"],
            serde_json::json!(f64::from(crate::limiter::DEFAULT_CEILING_DB))
        );
        assert_eq!(bus["limit_lookahead_ms"], serde_json::json!(1.0));
        // A message as a run before the struct wrote it reads back the same.
        let mut older = json.clone();
        let bus = older["buses"][0].as_object_mut().expect("a bus");
        bus.insert("limit_on".to_owned(), serde_json::json!(false));
        bus.insert("limit_db".to_owned(), serde_json::json!(-6.0));
        bus.insert("limit_lookahead_ms".to_owned(), serde_json::json!(2.5));
        let read: State = serde_json::from_value(older.clone()).expect("from JSON");
        assert_eq!(
            read.buses[0].limit,
            LimitSettings {
                on: false,
                ceiling_db: -6.0,
                lookahead_ms: 2.5
            }
        );
        assert_eq!(serde_json::to_value(&read).expect("to JSON"), older);
    }

    #[test]
    fn a_state_lists_the_scenes_and_reads_without_them() {
        let state = State::new(&view(), Vec::new());
        assert_eq!(state.scenes, ["Abend", "Laut"]);

        let mut older = serde_json::to_value(&state).expect("to JSON");
        older.as_object_mut().expect("an object").remove("scenes");
        // A run older than gates and compressors says nothing of them: they read as off.
        let input = older["inputs"][0].as_object_mut().expect("an input");
        input.remove("gate");
        input.remove("comp");
        let bus = older["buses"][0].as_object_mut().expect("a bus");
        bus.remove("eq_on");
        bus.remove("eq");
        for key in ["limit_on", "limit_db", "limit_lookahead_ms", "limited_db"] {
            bus.remove(key);
        }
        let read: State = serde_json::from_value(older).expect("from JSON");
        assert_eq!(read.scenes, Vec::<String>::new());
        assert_eq!((read.inputs[0].gate, read.inputs[0].comp), (0.0, 0.0));
        assert!(!read.buses[0].eq_on && read.buses[0].eq.is_empty());
        assert!(!read.buses[0].limit.on);
        assert!(state.buses[0].eq_on);
        assert_eq!(state.buses[0].eq.len(), crate::eq::EQ_BANDS);
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
        assert!(
            refused(r#"{"op":"set","address":"bus/A1/limit/ceiling","value":3}"#)
                .contains("the ceiling of a limiter must be -20 to 0 dB")
        );
        assert!(
            refused(r#"{"op":"set","address":"bus/A1/limit/lookahead","value":6}"#)
                .contains("the lookahead of a limiter must be 0 to 5 ms")
        );
        assert!(
            refused(r#"{"op":"set","address":"bus/A1/limit","value":1}"#).contains("true or false")
        );
        assert!(
            refused(r#"{"op":"set","address":"bus/A1/limit/ceiling","value":true}"#)
                .contains("takes a number")
        );
        assert!(
            refused(r#"{"op":"set","address":"bus/A1/limit/ceiling","value":-3,"ramp_ms":100}"#)
                .contains("can ramp")
        );
        assert!(
            refused(r#"{"op":"set","address":"bus/A1/eq/2/gain","value":30}"#)
                .contains("the gain of a band must be -24 to 24 dB")
        );
        assert!(
            refused(r#"{"op":"set","address":"bus/A1/eq/2/freq","value":5}"#)
                .contains("the freq of a band must be 20 to 20000 Hz")
        );
        assert!(
            refused(r#"{"op":"set","address":"bus/A1/eq/2/kind","value":"notch"}"#)
                .contains("takes low, bell or high, not \"notch\"")
        );
        assert!(
            refused(r#"{"op":"set","address":"bus/A1/eq/2/kind","value":1}"#)
                .contains("takes low, bell or high")
        );
        assert!(
            refused(r#"{"op":"set","address":"bus/A1/eq/2/gain","value":"bell"}"#)
                .contains("takes a number")
        );
        assert!(
            refused(r#"{"op":"set","address":"bus/A1/eq","value":"bell"}"#)
                .contains("true or false")
        );
        assert!(
            refused(r#"{"op":"set","address":"bus/A1/eq/2/gain","value":3,"ramp_ms":100}"#)
                .contains("can ramp")
        );
        assert!(
            refused(r#"{"op":"set","address":"input/1/gate","value":11}"#)
                .contains("gate must be 0 (off) to 10")
        );
        assert!(
            refused(r#"{"op":"set","address":"input/1/comp","value":-1}"#)
                .contains("comp must be 0 (off) to 10")
        );
        assert!(
            refused(r#"{"op":"set","address":"input/1/comp","value":true}"#)
                .contains("takes a number")
        );
        assert!(
            refused(r#"{"op":"set","address":"input/1/comp","value":3,"ramp_ms":100}"#)
                .contains("can ramp")
        );
    }

    #[test]
    fn nothing_but_the_known_ops_reads() {
        for json in [
            r#"{"op":"save","path":"C:\\evil.toml"}"#,
            r#"{"op":"delete"}"#,
            r#"{"op":"set","address":"input/1/gain"}"#,
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
