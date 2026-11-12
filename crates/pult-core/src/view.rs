//! What a user interface shows of a running mixer, and what it can ask of it.
//!
//! The window (`pult-app`) knows only this module: a [`Desk`] hands out a [`MixerView`],
//! a plain snapshot without references into the engine, and takes [`Action`]s. The engine
//! side implements the trait over a running route; [`FakeDesk`] implements it in memory,
//! with moving meters, so the interface can be built and tested without any audio device.
//!
//! Indices count from zero in the order of the route (the console's `[1]` is index 0).

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use crate::ramp::MAX_RAMP_MS;

pub use crate::binding::{
    AddressError, BandParam, Binding, BusParam, InputParam, Value, action_gain,
};
pub use crate::eq::{Band, BandChange, BandKind, EQ_BANDS};
pub use crate::limiter::LimitChange;
use crate::limiter::{DEFAULT_CEILING_DB, DEFAULT_LOOKAHEAD_MS};

/// A snapshot of the whole mixer, taken about 30 times a second.
#[derive(Debug, Clone, PartialEq)]
pub struct MixerView {
    /// Time since the route started.
    pub elapsed: Duration,
    /// Dropouts so far per hour, as events: inputs that ran dry, hard resyncs and device
    /// glitches that come together count once ([`crate::dropouts`]).
    pub dropouts_per_hour: f64,
    /// Glitches the devices reported so far.
    pub glitches: u64,
    /// Where the input meters measure.
    pub meter_point: MeterPoint,
    /// The inputs, in route order.
    pub inputs: Vec<InputView>,
    /// The buses, in route order.
    pub buses: Vec<BusView>,
    /// What happened since the previous view (devices lost, back or refused), oldest first.
    pub events: Vec<EventView>,
    /// The profile the route came from or was last saved to.
    pub profile: Option<PathBuf>,
    /// The scenes of the route, in profile order.
    pub scenes: Vec<String>,
    /// The MIDI controller, where someone runs one for this desk.
    pub midi: Option<crate::midi::MidiStatus>,
}

/// What an input captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// A capture device (microphone, line in, a virtual cable).
    Device,
    /// The sound of one program (process loopback).
    Program,
}

/// The send of one crossing of an input and a bus.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SendView {
    /// Its level in dB, 0 at first.
    pub db: f32,
    /// Taken before the input's fader.
    pub pre: bool,
}

impl Default for SendView {
    fn default() -> Self {
        Self {
            db: 0.0,
            pre: false,
        }
    }
}

/// One input as a channel strip shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct InputView {
    /// The device's or program's full name.
    pub name: String,
    /// What the profile calls it (`Musik`), if it does; [`InputView::shown_name`] picks.
    pub label: Option<String>,
    /// Device or program.
    pub kind: SourceKind,
    /// Its sample rate in Hz.
    pub sample_rate: u32,
    /// Gain in dB.
    pub gain_db: f32,
    /// Muted.
    pub muted: bool,
    /// Soloed: while any input is soloed, only soloed inputs sound.
    pub solo: bool,
    /// -1 (left) to 1 (right).
    pub pan: f32,
    /// Its gate as one knob, 0 (off) to 10: the higher, the louder a sound must be to pass.
    pub gate: f32,
    /// Its compressor as one knob, 0 (off) to 10: the higher, the more even and the louder.
    pub comp: f32,
    /// The buses it feeds, as indices into [`MixerView::buses`].
    pub buses: Vec<usize>,
    /// The send to each bus, by bus index; a bus beyond the list has the default (0 dB, post).
    pub sends: Vec<SendView>,
    /// Whether the device or program is there; false while it is gone.
    pub present: bool,
    /// Peak since the previous view in dBFS, at the [`MixerView::meter_point`]; `None` for
    /// silence.
    pub peak_db: Option<f32>,
    /// The resampler's correction against clock drift in ppm.
    pub drift_ppm: f32,
    /// Times it ran dry or resynced so far.
    pub dropouts: u64,
}

impl InputView {
    /// The send of this input to bus `bus`.
    #[must_use]
    pub fn send(&self, bus: usize) -> SendView {
        self.sends.get(bus).copied().unwrap_or_default()
    }

    /// The send to bus `bus`, to change it; the list grows to it.
    pub fn send_mut(&mut self, bus: usize) -> &mut SendView {
        if self.sends.len() <= bus {
            self.sends.resize(bus + 1, SendView::default());
        }
        &mut self.sends[bus]
    }

    /// What a strip calls it: its label, else its full name.
    #[must_use]
    pub fn shown_name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }
}

/// One bus as its master strip shows it.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // Switches of a console, each on its own.
pub struct BusView {
    /// The short name (`A1`), which commands and addresses use.
    pub name: String,
    /// What the profile calls it (`Boxen`), if it does; [`BusView::shown_name`] picks.
    pub label: Option<String>,
    /// The output device's full name.
    pub device: String,
    /// Its sample rate in Hz.
    pub sample_rate: u32,
    /// Gain in dB.
    pub gain_db: f32,
    /// Muted.
    pub muted: bool,
    /// Whether its equalizer is switched on; off, the bands keep their settings.
    pub eq_on: bool,
    /// The bands of its equalizer.
    pub eq: [Band; EQ_BANDS],
    /// Whether its limiter is switched on; off, it keeps its settings.
    pub limit_on: bool,
    /// The ceiling of its limiter in dB.
    pub limit_db: f32,
    /// The lookahead of its limiter in milliseconds.
    pub limit_lookahead_ms: f32,
    /// A monitor bus: solo acts on it (and, while the route has one, only on such buses).
    pub monitor: bool,
    /// How far its limiter turned down at most since the previous view, in dB; 0 when it did not.
    pub limited_db: f32,
    /// Whether the device is there; false while it is gone.
    pub present: bool,
    /// Peak of the output since the previous view in dBFS; `None` for silence.
    pub peak_db: Option<f32>,
    /// Samples clipped so far.
    pub clipped: u64,
}

impl BusView {
    /// What its strip calls it: its label, else its short name.
    #[must_use]
    pub fn shown_name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }
}

/// What kind of thing happened to a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// It stopped; the rest plays on.
    Lost,
    /// It plays again.
    Back,
    /// It is there again but does not open yet.
    Refused,
    /// It plays, but Windows refused it realtime priority.
    Slow,
}

/// Something that happened to a device, as one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventView {
    /// When, as time since the route started.
    pub elapsed: Duration,
    /// What kind of thing, for those who show it without reading the text.
    pub kind: EventKind,
    /// What, as the console says it (`A1 Focusrite is back`).
    pub text: String,
}

/// Where the input meters measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MeterPoint {
    /// Before gate, compressor, gain and mute: what arrives.
    PreFader,
    /// After all of those: what goes into the mix.
    #[default]
    PostFader,
}

/// A change the interface asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Sets an input's gain in dB.
    InputGain {
        /// Input index.
        input: usize,
        /// Gain in dB.
        db: f32,
        /// Milliseconds to get there (0: at once, free of clicks).
        ramp_ms: u32,
    },
    /// Mutes or unmutes an input.
    InputMute {
        /// Input index.
        input: usize,
        /// Muted.
        muted: bool,
    },
    /// Solos or unsolos an input.
    InputSolo {
        /// Input index.
        input: usize,
        /// Soloed.
        solo: bool,
    },
    /// Sets an input's pan, -1 to 1.
    InputPan {
        /// Input index.
        input: usize,
        /// Pan.
        pan: f32,
        /// Milliseconds to get there (0: at once, free of clicks).
        ramp_ms: u32,
    },
    /// Sets an input's gate by its one knob.
    InputGate {
        /// Input index.
        input: usize,
        /// 0 (off) to 10.
        amount: f32,
    },
    /// Sets an input's compressor by its one knob.
    InputComp {
        /// Input index.
        input: usize,
        /// 0 (off) to 10.
        amount: f32,
    },
    /// Sets a bus's gain in dB.
    BusGain {
        /// Bus index.
        bus: usize,
        /// Gain in dB.
        db: f32,
        /// Milliseconds to get there (0: at once, free of clicks).
        ramp_ms: u32,
    },
    /// Mutes or unmutes a bus.
    BusMute {
        /// Bus index.
        bus: usize,
        /// Muted.
        muted: bool,
    },
    /// Switches a bus's equalizer on or off.
    BusEq {
        /// Bus index.
        bus: usize,
        /// On.
        on: bool,
    },
    /// Changes one setting of one band of a bus's equalizer.
    BusEqBand {
        /// Bus index.
        bus: usize,
        /// Band index, from 0.
        band: usize,
        /// The new setting.
        change: BandChange,
    },
    /// Changes one setting of a bus's limiter.
    BusLimit {
        /// Bus index.
        bus: usize,
        /// The new setting.
        change: LimitChange,
    },
    /// Makes a bus a monitor bus or takes that away.
    BusMonitor {
        /// Bus index.
        bus: usize,
        /// Monitor or not.
        on: bool,
    },
    /// Routes an input to a bus or takes it off (a matrix button).
    Route {
        /// Input index.
        input: usize,
        /// Bus index.
        bus: usize,
        /// On or off.
        on: bool,
    },
    /// Sets the level of a crossing.
    Send {
        /// Input index.
        input: usize,
        /// Bus index.
        bus: usize,
        /// Level in dB.
        db: f32,
        /// Milliseconds to reach it in.
        ramp_ms: u32,
    },
    /// Takes the send of a crossing before or after the input's fader.
    SendPre {
        /// Input index.
        input: usize,
        /// Bus index.
        bus: usize,
        /// Before the fader.
        pre: bool,
    },
    /// Changes to a scene of the route.
    Scene {
        /// Its name, ignoring case.
        name: String,
    },
    /// Saves the current state as a profile; `None` saves to [`MixerView::profile`].
    SaveProfile {
        /// Where to.
        path: Option<PathBuf>,
    },
    /// Moves the input meters before or after the fader.
    SetMeterPoint(MeterPoint),
}

impl Action {
    /// The same action taking `ms` milliseconds.
    ///
    /// # Errors
    ///
    /// A message when it is no gain or pan, which alone ramp, or `ms` is beyond [`MAX_RAMP_MS`].
    pub fn over(mut self, ms: u32) -> Result<Self, String> {
        if ms > MAX_RAMP_MS {
            return Err(format!("a ramp takes at most {MAX_RAMP_MS} ms, not {ms}"));
        }
        match &mut self {
            Self::InputGain { ramp_ms, .. }
            | Self::InputPan { ramp_ms, .. }
            | Self::BusGain { ramp_ms, .. } => *ramp_ms = ms,
            _ => return Err("only a gain or a pan can ramp".to_owned()),
        }
        Ok(self)
    }
}

/// What an action did, one line each, or why it was not done.
pub type Reply = Result<Vec<String>, String>;

/// A mixing desk an interface can show and control.
pub trait Desk {
    /// The state now; takes the peaks and events since the previous call.
    fn view(&mut self) -> MixerView;

    /// Carries out `action`.
    ///
    /// # Errors
    ///
    /// The reason, when the action does not fit (an index out of range, a value out of
    /// range, a profile that cannot be written, an input that would feed no bus).
    fn act(&mut self, action: Action) -> Reply;
}

/// Lowest and highest gain in dB an action may set; profiles and commands use the same
/// range (`pult_engine::profile::GAIN_DB` is built from it).
pub const GAIN_RANGE_DB: (f32, f32) = (-96.0, 24.0);

/// Lowest and highest setting of the one knob of a gate or a compressor; the lowest is off.
pub const AMOUNT_RANGE: (f32, f32) = (0.0, crate::dynamics::SIMPLE_MAX);

/// `amount` when it is a setting of the one knob of a gate or a compressor (`what`).
///
/// # Errors
///
/// A message naming the range otherwise.
pub fn check_amount(what: &str, amount: f32) -> Result<f32, String> {
    let (low, high) = AMOUNT_RANGE;
    if (low..=high).contains(&amount) {
        Ok(amount)
    } else {
        Err(format!(
            "{what} must be {low} (off) to {high}, not {amount}"
        ))
    }
}

/// A mixer in memory for building and testing interfaces: three inputs, two buses, meters
/// that move with every view (one view counts as 1/30 s), an event now and then, and every
/// action applied to its own state. It never touches a device, a file or the clock.
#[derive(Debug, Clone)]
pub struct FakeDesk {
    view: MixerView,
    frame: u64,
}

impl Default for FakeDesk {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeDesk {
    /// The demo mixer: a virtual cable, a microphone and a program on two buses.
    #[must_use]
    pub fn new() -> Self {
        let input = |name: &str, kind, sample_rate, gain_db, muted, buses: Vec<usize>| InputView {
            name: name.to_owned(),
            label: None,
            kind,
            sample_rate,
            gain_db,
            muted,
            solo: false,
            pan: 0.0,
            gate: 0.0,
            comp: 0.0,
            buses,
            sends: Vec::new(),
            present: true,
            peak_db: None,
            drift_ppm: 0.0,
            dropouts: 0,
        };
        let bus = |name: &str, device: &str, sample_rate| BusView {
            name: name.to_owned(),
            label: None,
            device: device.to_owned(),
            sample_rate,
            gain_db: 0.0,
            muted: false,
            eq_on: true,
            eq: Band::all_flat(),
            limit_on: true,
            limit_db: DEFAULT_CEILING_DB,
            limit_lookahead_ms: DEFAULT_LOOKAHEAD_MS,
            monitor: false,
            limited_db: 0.0,
            present: true,
            peak_db: None,
            clipped: 0,
        };
        Self {
            view: MixerView {
                elapsed: Duration::ZERO,
                dropouts_per_hour: 0.0,
                glitches: 0,
                meter_point: MeterPoint::PostFader,
                inputs: vec![
                    input(
                        "CABLE Output (VB-Audio Virtual Cable)",
                        SourceKind::Device,
                        48_000,
                        0.0,
                        false,
                        vec![0],
                    ),
                    input(
                        "Analogue 1 + 2 (2- Focusrite USB Audio)",
                        SourceKind::Device,
                        48_000,
                        -6.0,
                        true,
                        vec![0, 1],
                    ),
                    input(
                        "Spotify.exe",
                        SourceKind::Program,
                        48_000,
                        -3.0,
                        false,
                        vec![1],
                    ),
                ],
                buses: vec![
                    bus("A1", "Lautsprecher (2- Focusrite USB Audio)", 48_000),
                    bus("A2", "Kopfhörer (Realtek Audio)", 44_100),
                ],
                events: Vec::new(),
                profile: Some(PathBuf::from("pult.toml")),
                scenes: vec!["Abend".to_owned(), "Laut".to_owned()],
                midi: None,
            },
            frame: 0,
        }
    }

    /// The level an input's source plays at this frame, in dBFS: a slow swell with a beat.
    fn source_db(&self, input: usize) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        let t = self.frame as f32 / 30.0;
        let phase = t * (0.7 + 0.31 * input as f32);
        let beat = if (self.frame + 7 * input as u64) % 15 < 3 {
            6.0
        } else {
            0.0
        };
        -24.0 + 8.0 * phase.sin() + beat
    }

    /// Makes `bus` a monitor bus or takes that away.
    fn set_monitor(&mut self, bus: usize, on: bool) -> Result<Vec<String>, String> {
        let bus = self.bus_mut(bus)?;
        bus.monitor = on;
        let state = if on {
            "is a monitor bus"
        } else {
            "is no monitor bus"
        };
        Ok(vec![format!("{} {state}", bus.name)])
    }

    /// Sets the level and/or the point of the send of `input` to `bus`.
    fn set_send(
        &mut self,
        input: usize,
        bus: usize,
        db: Option<f32>,
        pre: Option<bool>,
    ) -> Result<Vec<String>, String> {
        let db = db.map(check_gain).transpose()?;
        let name = self.bus_mut(bus)?.name.clone();
        let send = self.input_mut(input)?.send_mut(bus);
        let mut said = Vec::new();
        if let Some(db) = db {
            send.db = db;
            said.push(format!("[{}] send to {name} {db:.1} dB", input + 1));
        }
        if let Some(pre) = pre {
            send.pre = pre;
            let point = if pre { "pre" } else { "post" };
            said.push(format!("[{}] send to {name} {point}", input + 1));
        }
        Ok(said)
    }

    fn input_mut(&mut self, input: usize) -> Result<&mut InputView, String> {
        let count = self.view.inputs.len();
        self.view
            .inputs
            .get_mut(input)
            .ok_or_else(|| format!("there is no input {} (only {count})", input + 1))
    }

    /// Sets the one knob `what` (`gate` or `comp`) of an input.
    fn set_knob(&mut self, input: usize, what: &str, amount: f32) -> Reply {
        let amount = check_amount(what, amount)?;
        let strip = self.input_mut(input)?;
        if what == "gate" {
            strip.gate = amount;
        } else {
            strip.comp = amount;
        }
        Ok(vec![format!("[{}] {what} {amount:.1}", input + 1)])
    }

    /// Switches a bus's equalizer.
    fn set_eq(&mut self, bus: usize, on: bool) -> Reply {
        let bus = self.bus_mut(bus)?;
        bus.eq_on = on;
        let state = if on { "on" } else { "off" };
        Ok(vec![format!("{} eq {state}", bus.name)])
    }

    /// Changes one setting of one band of a bus's equalizer.
    fn set_band(&mut self, bus: usize, band: usize, change: BandChange) -> Reply {
        let bus = self.bus_mut(bus)?;
        let Some(set) = bus.eq.get_mut(band) else {
            return Err(format!(
                "there is no band {}; bands are 1 to {EQ_BANDS}",
                band + 1
            ));
        };
        *set = set.changed(change)?;
        Ok(vec![format!(
            "{} eq {}: {}",
            bus.name,
            band + 1,
            band_line(*set)
        )])
    }

    /// Changes one setting of a bus's limiter.
    fn set_limit(&mut self, bus: usize, change: LimitChange) -> Reply {
        let change = change.checked()?;
        let bus = self.bus_mut(bus)?;
        apply_limit(
            &mut bus.limit_on,
            &mut bus.limit_db,
            &mut bus.limit_lookahead_ms,
            change,
        );
        Ok(vec![format!(
            "{} {}",
            bus.name,
            limit_line(bus.limit_on, bus.limit_db, bus.limit_lookahead_ms)
        )])
    }

    fn bus_mut(&mut self, bus: usize) -> Result<&mut BusView, String> {
        let count = self.view.buses.len();
        self.view
            .buses
            .get_mut(bus)
            .ok_or_else(|| format!("there is no bus {} (only {count})", bus + 1))
    }
}

/// Makes `change` in the three settings of a limiter, wherever they are kept.
pub fn apply_limit(
    on: &mut bool,
    ceiling_db: &mut f32,
    lookahead_ms: &mut f32,
    change: LimitChange,
) {
    match change {
        LimitChange::Switch(new) => *on = new,
        LimitChange::Ceiling(db) => *ceiling_db = db,
        LimitChange::Lookahead(ms) => *lookahead_ms = ms,
    }
}

/// A limiter as a line says it: `limit off`, or `limit -0.3 dB, 1 ms`.
#[must_use]
pub fn limit_line(on: bool, ceiling_db: f32, lookahead_ms: f32) -> String {
    if on {
        format!("limit {ceiling_db:.1} dB, {lookahead_ms} ms")
    } else {
        "limit off".to_owned()
    }
}

/// What a line of `show` says of a limiter, to put behind the rest of a bus: nothing for one that
/// stands as it starts.
#[must_use]
pub fn limit_summary(on: bool, ceiling_db: f32, lookahead_ms: f32) -> String {
    #[allow(clippy::float_cmp)] // Exactly: as set, or not.
    let as_it_starts =
        on && ceiling_db == DEFAULT_CEILING_DB && lookahead_ms == DEFAULT_LOOKAHEAD_MS;
    if as_it_starts {
        String::new()
    } else {
        format!("  {}", limit_line(on, ceiling_db, lookahead_ms))
    }
}

/// A band as a line says it: `bell 1000 Hz +6.0 dB q 0.7`.
#[must_use]
pub fn band_line(band: Band) -> String {
    format!(
        "{} {} Hz {:+.1} dB q {}",
        band.kind.word(),
        band.freq_hz,
        band.gain_db,
        band.q
    )
}

/// What a line of `show` says of an equalizer, to put behind the rest of a bus: nothing for a
/// flat one that is on, else `  eq off` and every band that is not flat.
#[must_use]
pub fn eq_summary(on: bool, bands: &[Band]) -> String {
    let mut text = String::new();
    if !on {
        text.push_str("  eq off");
    }
    for (index, band) in bands.iter().enumerate() {
        if band.gain_db != 0.0 {
            let _ = write!(text, "  eq {}: {}", index + 1, band_line(*band));
        }
    }
    text
}

fn check_gain(db: f32) -> Result<f32, String> {
    let (low, high) = GAIN_RANGE_DB;
    if (low..=high).contains(&db) {
        Ok(db)
    } else {
        Err(format!("gain {db} dB is outside {low} to {high} dB"))
    }
}

/// Adds levels in dB as uncorrelated power.
fn sum_db(levels: &[f32]) -> Option<f32> {
    let power: f32 = levels.iter().map(|db| 10f32.powf(db / 10.0)).sum();
    (power > 0.0).then(|| 10.0 * power.log10())
}

impl Desk for FakeDesk {
    fn view(&mut self) -> MixerView {
        self.frame += 1;
        self.view.elapsed = Duration::from_millis(self.frame * 1000 / 30);
        let soloing = self.view.inputs.iter().any(|i| i.solo);
        let mut bus_levels: Vec<Vec<f32>> = vec![Vec::new(); self.view.buses.len()];
        for index in 0..self.view.inputs.len() {
            let source = self.source_db(index);
            let input = &self.view.inputs[index];
            let silent = input.muted || (soloing && !input.solo) || !input.present;
            let fed = (!silent).then_some(source + input.gain_db);
            let peak = match self.view.meter_point {
                MeterPoint::PreFader => input.present.then_some(source),
                MeterPoint::PostFader => fed,
            };
            if let Some(level) = fed {
                for &bus in &input.buses {
                    bus_levels[bus].push(level);
                }
            }
            #[allow(clippy::cast_precision_loss)]
            let drift = 12.0 * (self.frame as f32 / 90.0 + index as f32).sin();
            let input = &mut self.view.inputs[index];
            input.peak_db = peak;
            input.drift_ppm = drift;
        }
        for (bus, levels) in self.view.buses.iter_mut().zip(&bus_levels) {
            bus.peak_db = if bus.muted {
                None
            } else {
                sum_db(levels).map(|db| db + bus.gain_db)
            };
            if bus.peak_db.is_some_and(|db| db > 0.0) {
                bus.clipped += 1;
            }
        }
        // Every 20 s the program goes away for 3 s, so the interface sees a missing input.
        if self.frame.is_multiple_of(600) {
            self.view.inputs[2].present = false;
            self.view.events.push(EventView {
                elapsed: self.view.elapsed,
                kind: EventKind::Lost,
                text: format!("[3] {} is gone (demo)", self.view.inputs[2].name),
            });
        } else if self.frame % 600 == 90 && !self.view.inputs[2].present {
            self.view.inputs[2].present = true;
            self.view.events.push(EventView {
                elapsed: self.view.elapsed,
                kind: EventKind::Back,
                text: format!("[3] {} is back", self.view.inputs[2].name),
            });
        }
        let snapshot = self.view.clone();
        self.view.events.clear();
        snapshot
    }

    fn act(&mut self, action: Action) -> Reply {
        match action {
            Action::InputGain { input, db, .. } => {
                let db = check_gain(db)?;
                self.input_mut(input)?.gain_db = db;
                Ok(vec![format!("[{}] gain {db:.1} dB", input + 1)])
            }
            Action::InputMute { input, muted } => {
                self.input_mut(input)?.muted = muted;
                Ok(vec![format!(
                    "[{}] {}",
                    input + 1,
                    if muted { "muted" } else { "unmuted" }
                )])
            }
            Action::InputSolo { input, solo } => {
                self.input_mut(input)?.solo = solo;
                Ok(vec![format!(
                    "[{}] {}",
                    input + 1,
                    if solo { "solo" } else { "solo off" }
                )])
            }
            Action::InputPan { input, pan, .. } => {
                if !(-1.0..=1.0).contains(&pan) {
                    return Err(format!("pan {pan} is outside -1 to 1"));
                }
                self.input_mut(input)?.pan = pan;
                Ok(vec![format!("[{}] pan {pan:.2}", input + 1)])
            }
            Action::InputGate { input, amount } => self.set_knob(input, "gate", amount),
            Action::InputComp { input, amount } => self.set_knob(input, "comp", amount),
            Action::BusGain { bus, db, .. } => {
                let db = check_gain(db)?;
                let bus = self.bus_mut(bus)?;
                bus.gain_db = db;
                Ok(vec![format!("{} gain {db:.1} dB", bus.name)])
            }
            Action::BusMute { bus, muted } => {
                let bus = self.bus_mut(bus)?;
                bus.muted = muted;
                Ok(vec![format!(
                    "{} {}",
                    bus.name,
                    if muted { "muted" } else { "unmuted" }
                )])
            }
            Action::BusEq { bus, on } => self.set_eq(bus, on),
            Action::BusEqBand { bus, band, change } => self.set_band(bus, band, change),
            Action::BusLimit { bus, change } => self.set_limit(bus, change),
            Action::BusMonitor { bus, on } => self.set_monitor(bus, on),
            Action::Send { input, bus, db, .. } => self.set_send(input, bus, Some(db), None),
            Action::SendPre { input, bus, pre } => self.set_send(input, bus, None, Some(pre)),
            Action::Route { input, bus, on } => {
                let name = self.bus_mut(bus)?.name.clone();
                let strip = self.input_mut(input)?;
                if on {
                    if !strip.buses.contains(&bus) {
                        strip.buses.push(bus);
                        strip.buses.sort_unstable();
                    }
                } else {
                    if strip.buses == [bus] {
                        return Err(format!(
                            "[{}] would feed no bus; mute it instead",
                            input + 1
                        ));
                    }
                    strip.buses.retain(|&b| b != bus);
                }
                Ok(vec![format!(
                    "[{}] {} {name}",
                    input + 1,
                    if on { "->" } else { "off" }
                )])
            }
            Action::Scene { name } => {
                // The demo's scenes: one quieter with the microphone on, one with all up.
                let (cable, microphone) = if name.eq_ignore_ascii_case("Abend") {
                    (-12.0, false)
                } else if name.eq_ignore_ascii_case("Laut") {
                    (0.0, true)
                } else {
                    return Err(format!("no scene \"{name}\"; scenes: Abend, Laut"));
                };
                self.input_mut(0)?.gain_db = cable;
                self.input_mut(1)?.muted = microphone;
                Ok(vec![format!("scene {name}")])
            }
            Action::SaveProfile { path } => {
                let path = path
                    .or_else(|| self.view.profile.clone())
                    .ok_or("no profile path yet; name one")?;
                let line = format!("saved the current state to {} (demo)", path.display());
                self.view.profile = Some(path);
                Ok(vec![line])
            }
            Action::SetMeterPoint(point) => {
                self.view.meter_point = point;
                Ok(vec![format!("meters {point:?}")])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_moves_its_meters_and_honours_mute_and_solo() {
        let mut mixer = FakeDesk::new();
        let first = mixer.view();
        assert_eq!(first.inputs.len(), 3);
        assert_eq!(first.buses.len(), 2);
        // Input 2 starts muted: silent after the fader, but its source shows before it.
        assert!(first.inputs[0].peak_db.is_some());
        assert_eq!(first.inputs[1].peak_db, None);
        mixer
            .act(Action::SetMeterPoint(MeterPoint::PreFader))
            .unwrap();
        assert!(mixer.view().inputs[1].peak_db.is_some());
        mixer
            .act(Action::SetMeterPoint(MeterPoint::PostFader))
            .unwrap();

        mixer
            .act(Action::InputSolo {
                input: 2,
                solo: true,
            })
            .unwrap();
        let soloed = mixer.view();
        assert_eq!(soloed.inputs[0].peak_db, None);
        assert!(soloed.inputs[2].peak_db.is_some());
        // Bus A1 hears only input 1 and the muted input 2, so nothing while 3 is soloed.
        assert_eq!(soloed.buses[0].peak_db, None);
        assert!(soloed.buses[1].peak_db.is_some());

        let a = mixer.view().inputs[2].peak_db;
        let b = mixer.view().inputs[2].peak_db;
        assert_ne!(a, b, "meters move from view to view");
    }

    #[test]
    fn the_fake_applies_and_checks_actions() {
        let mut mixer = FakeDesk::new();
        assert!(
            mixer
                .act(Action::InputGain {
                    input: 0,
                    db: -12.0,
                    ramp_ms: 0
                })
                .is_ok()
        );
        assert!(
            mixer
                .act(Action::InputGain {
                    input: 0,
                    db: 40.0,
                    ramp_ms: 0
                })
                .is_err()
        );
        // The whole range of profiles and commands, down to -96 dB, is accepted.
        assert!(
            mixer
                .act(Action::InputGain {
                    input: 1,
                    db: -90.0,
                    ramp_ms: 0
                })
                .is_ok()
        );
        assert!(
            mixer
                .act(Action::InputGain {
                    input: 1,
                    db: -97.0,
                    ramp_ms: 0
                })
                .is_err()
        );
        assert!(
            mixer
                .act(Action::InputGain {
                    input: 9,
                    db: 0.0,
                    ramp_ms: 0
                })
                .is_err()
        );
        assert!(
            mixer
                .act(Action::InputPan {
                    input: 0,
                    pan: 2.0,
                    ramp_ms: 0
                })
                .is_err()
        );
        assert!(
            mixer
                .act(Action::BusMute {
                    bus: 1,
                    muted: true
                })
                .is_ok()
        );
        assert!(
            mixer
                .act(Action::Route {
                    input: 0,
                    bus: 1,
                    on: true
                })
                .is_ok()
        );
        assert!(
            mixer
                .act(Action::Route {
                    input: 2,
                    bus: 1,
                    on: false
                })
                .is_err()
        );
        let view = mixer.view();
        assert!((view.inputs[0].gain_db + 12.0).abs() < f32::EPSILON);
        assert_eq!(view.inputs[0].buses, [0, 1]);
        assert_eq!(view.buses[1].peak_db, None);
        let saved = mixer
            .act(Action::SaveProfile {
                path: Some("neu.toml".into()),
            })
            .unwrap();
        assert!(saved[0].contains("neu.toml"));
        assert_eq!(mixer.view().profile, Some(PathBuf::from("neu.toml")));
    }

    #[test]
    fn the_fake_takes_the_knobs_of_gate_and_compressor_in_their_range() {
        let mut mixer = FakeDesk::new();
        let gate = |amount| Action::InputGate { input: 0, amount };
        let comp = |amount| Action::InputComp { input: 0, amount };
        assert!(mixer.act(gate(3.0)).is_ok());
        assert!(mixer.act(comp(10.0)).is_ok());
        assert!(mixer.act(comp(0.0)).is_ok());
        assert!(mixer.act(comp(10.5)).is_err());
        assert!(mixer.act(gate(-1.0)).is_err());
        assert!(mixer.act(gate(f32::NAN)).is_err());
        assert!(
            mixer
                .act(Action::InputGate {
                    input: 9,
                    amount: 1.0
                })
                .is_err()
        );
        let view = mixer.view();
        assert_eq!((view.inputs[0].gate, view.inputs[0].comp), (3.0, 0.0));
    }

    #[test]
    fn the_fake_takes_a_limiter_and_refuses_what_is_out_of_range() {
        let mut mixer = FakeDesk::new();
        let limit = |change| Action::BusLimit { bus: 1, change };
        assert_eq!(
            mixer.act(limit(LimitChange::Ceiling(-6.0))),
            Ok(vec!["A2 limit -6.0 dB, 1 ms".to_owned()])
        );
        assert_eq!(
            mixer.act(limit(LimitChange::Lookahead(2.5))),
            Ok(vec!["A2 limit -6.0 dB, 2.5 ms".to_owned()])
        );
        assert_eq!(
            mixer.act(limit(LimitChange::Switch(false))),
            Ok(vec!["A2 limit off".to_owned()])
        );
        let view = mixer.view();
        assert!(!view.buses[1].limit_on);
        assert!(view.buses[0].limit_on);
        assert!(mixer.act(limit(LimitChange::Ceiling(3.0))).is_err());
        assert!(mixer.act(limit(LimitChange::Lookahead(6.0))).is_err());
        assert!(mixer.act(limit(LimitChange::Ceiling(f32::NAN))).is_err());
    }

    #[test]
    fn a_line_names_a_limiter_only_when_it_differs_from_its_start() {
        assert_eq!(
            limit_summary(true, DEFAULT_CEILING_DB, DEFAULT_LOOKAHEAD_MS),
            ""
        );
        assert_eq!(
            limit_summary(false, DEFAULT_CEILING_DB, DEFAULT_LOOKAHEAD_MS),
            "  limit off"
        );
        assert_eq!(limit_summary(true, -6.0, 0.0), "  limit -6.0 dB, 0 ms");
    }

    #[test]
    fn the_fake_takes_an_equalizer_and_refuses_what_is_out_of_range() {
        let mut mixer = FakeDesk::new();
        let band = |band, change| Action::BusEqBand {
            bus: 1,
            band,
            change,
        };
        assert_eq!(
            mixer.act(band(1, BandChange::Gain(6.0))),
            Ok(vec!["A2 eq 2: bell 500 Hz +6.0 dB q 0.7".to_owned()])
        );
        assert!(mixer.act(band(1, BandChange::Freq(1000.0))).is_ok());
        assert!(mixer.act(band(0, BandChange::Kind(BandKind::Peak))).is_ok());
        assert!(mixer.act(band(1, BandChange::Gain(25.0))).is_err());
        assert!(mixer.act(band(EQ_BANDS, BandChange::Q(1.0))).is_err());
        assert!(mixer.act(Action::BusEq { bus: 1, on: false }).is_ok());
        assert!(mixer.act(Action::BusEq { bus: 9, on: false }).is_err());
        let view = mixer.view();
        assert!(view.buses[0].eq_on && !view.buses[1].eq_on);
        assert_eq!(view.buses[1].eq[0].kind, BandKind::Peak);
        assert_eq!(
            view.buses[1].eq[1],
            Band {
                kind: BandKind::Peak,
                freq_hz: 1000.0,
                gain_db: 6.0,
                q: 0.7
            }
        );
        assert_eq!(view.buses[0].eq, Band::all_flat());
    }

    #[test]
    fn a_line_lists_only_what_an_equalizer_does() {
        let mut bands = Band::all_flat();
        assert_eq!(eq_summary(true, &bands), "");
        assert_eq!(eq_summary(false, &bands), "  eq off");
        bands[2] = bands[2].with_gain(-3.0);
        assert_eq!(
            eq_summary(false, &bands),
            "  eq off  eq 3: bell 2500 Hz -3.0 dB q 0.7"
        );
    }

    #[test]
    fn the_fake_loses_and_finds_its_program_and_reports_it_once() {
        let mut mixer = FakeDesk::new();
        let mut events = Vec::new();
        let mut missing = false;
        for _ in 0..700 {
            let view = mixer.view();
            missing |= !view.inputs[2].present;
            events.extend(view.events);
        }
        assert!(missing);
        assert_eq!(events.len(), 2, "{events:?}");
        assert!(events[0].text.ends_with("is gone (demo)"));
        assert_eq!(events[0].kind, EventKind::Lost);
        assert!(events[1].text.ends_with("is back"));
        assert_eq!(events[1].kind, EventKind::Back);
    }

    #[test]
    fn a_label_goes_before_the_name() {
        let mut view = FakeDesk::new().view();
        assert_eq!(view.inputs[0].shown_name(), view.inputs[0].name);
        assert_eq!(view.buses[0].shown_name(), "A1");
        view.inputs[0].label = Some("Musik".to_owned());
        view.buses[0].label = Some("Boxen".to_owned());
        assert_eq!(view.inputs[0].shown_name(), "Musik");
        assert_eq!(view.buses[0].shown_name(), "Boxen");
    }

    #[test]
    fn levels_add_as_power() {
        assert_eq!(sum_db(&[]), None);
        let two = sum_db(&[-6.0, -6.0]).unwrap();
        assert!((two + 2.99).abs() < 0.02, "{two}");
    }
}
