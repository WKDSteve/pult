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
pub use crate::limiter::{LimitChange, LimitSettings};

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
    /// Someone talks: the talkback microphones are open and the monitor buses dimmed.
    pub talk: bool,
    /// Solo on a monitor bus listens before the fader (PFL): a soloed input is heard there at
    /// unity, whatever its fader, mute, send and routing say. False: after them (AFL).
    pub pfl: bool,
}

/// Range of the dim level of a bus in dB: how far its dim switch turns it down.
pub const DIM_RANGE_DB: (f32, f32) = (0.0, 60.0);
/// The dim level a bus starts with.
pub const DEFAULT_DIM_DB: f32 = 20.0;

/// `db` as a dim level.
///
/// # Errors
///
/// A message naming the range when it is outside [`DIM_RANGE_DB`].
pub fn check_dim(db: f32) -> Result<f32, String> {
    let (low, high) = DIM_RANGE_DB;
    if (low..=high).contains(&db) {
        Ok(db)
    } else {
        Err(format!("a dim level is {low} to {high} dB, not {db}"))
    }
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
#[allow(clippy::struct_excessive_bools)] // Switches and states of a channel strip, each on its own.
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
    /// Takes part in the automixer.
    pub automix: bool,
    /// Its priority in the automixer in dB.
    pub automix_weight_db: f32,
    /// What the automixer gives it now in dB (0 outside it).
    pub automix_gain_db: f32,
    /// The most the gate took away since the previous view, in dB (0: open or off).
    pub gate_reduction_db: f32,
    /// Whether the gate is open (or off) now.
    pub gate_open: bool,
    /// The most the compressor took away since the previous view, in dB.
    pub comp_reduction_db: f32,
    /// Its channel strip: polarity, filters, delay, width.
    pub strip: crate::strip::StripSettings,
    /// A talkback microphone: silent until someone talks ([`MixerView::talk`]).
    pub talkback: bool,
    /// Whether its equalizer is on.
    pub eq_on: bool,
    /// The bands of its equalizer.
    pub eq: [Band; EQ_BANDS],
    /// The gain each band has now in dB: on a slide, or as far in as a dynamic band is.
    pub eq_now_db: [f32; EQ_BANDS],
    /// Its equalizer comes before gate and compressor instead of after them.
    pub eq_pre: bool,
    /// Whether its limiter is on; it has no lookahead.
    pub limit_on: bool,
    /// The ceiling of its limiter in dB.
    pub limit_db: f32,
    /// How far its limiter turned down at most since the previous view, in dB; 0 when it did not.
    pub limited_db: f32,
    /// Its sidechain: what its gate and compressor listen to.
    pub sidechain: crate::sidechain::SidechainSettings,
    /// Its de-esser: the level above which the band of its sibilants goes down, and its corner.
    pub deess: crate::deess::DeessSettings,
    /// The most its de-esser took from that band since the previous view, in dB.
    pub deess_reduction_db: f32,
    /// Its ducker: the input that turns it down, how far and how fast.
    pub duck: crate::duck::DuckSettings,
    /// The most its ducker took away since the previous view, in dB.
    pub duck_reduction_db: f32,
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

/// The loudness of a bus ([`crate::loudness`]); `None` until it is measured.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LoudnessView {
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

/// What of one input waits before a bus takes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingView {
    /// The input, from 0.
    pub input: usize,
    /// What waits in its ring to this bus, in milliseconds, averaged.
    pub ms: f32,
}

/// What holds the sound of a bus back, part by part.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeldBack {
    /// The fullest ring of the inputs that play on the bus, in milliseconds; 0 without one.
    pub ring_ms: f32,
    /// The bus itself: the lookahead of its limiter while that is on, and its delay.
    pub bus_ms: f32,
    /// What the device says it holds; `None` while it has not said.
    pub device_ms: Option<f32>,
}

impl HeldBack {
    /// All of it in milliseconds, the slowest input's way; without the device while that is
    /// not known.
    #[must_use]
    pub fn total_ms(&self) -> f32 {
        self.ring_ms + self.bus_ms + self.device_ms.unwrap_or(0.0)
    }

    /// Whether anything is known at all: a bus that does not play and has no delay holds
    /// nothing back that could be told.
    #[must_use]
    pub fn is_known(&self) -> bool {
        self.total_ms() > 0.0
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
    /// The gain each band has now in dB: on a slide, or as far in as a dynamic band is.
    pub eq_now_db: [f32; EQ_BANDS],
    /// Its limiter; switched off, it keeps its settings.
    pub limit: LimitSettings,
    /// A monitor bus: solo acts on it (and, while the route has one, only on such buses).
    pub monitor: bool,
    /// A mix-minus (N-1) bus: the input it is for, which never feeds it.
    pub minus: Option<usize>,
    /// Its loudness after EBU R128.
    pub loudness: LoudnessView,
    /// Its compressor as one knob, 0 (off) to 10.
    pub comp: f32,
    /// The mix of its compressor in percent, 0 (dry) to 100 (all compressed).
    pub comp_mix: f32,
    /// The corner of the sidechain high-pass of its compressor in Hz.
    pub comp_hpf_hz: f32,
    /// Its delay in milliseconds, behind the limiter.
    pub delay_ms: f32,
    /// Per input that feeds it, what waits in the ring between them; empty where nobody
    /// measures (a desk without an engine).
    pub rings: Vec<RingView>,
    /// How long its device says sound takes from the callback to the output, in
    /// milliseconds; `None` where the device does not say, or is gone.
    pub device_latency_ms: Option<f32>,
    /// Its dim switch: turned down by [`Self::dim_db`]. A monitor bus is dimmed too while
    /// someone talks, without this switch.
    pub dim: bool,
    /// How far dim turns it down, in dB.
    pub dim_db: f32,
    /// The most its compressor took away since the previous view, in dB.
    pub comp_reduction_db: f32,
    /// Its multiband compressor: the knob of each band and the two corners between them.
    pub multiband: crate::multiband::MultibandSettings,
    /// The most each band of it took away since the previous view, in dB, from the bottom.
    pub multiband_reduction_db: [f32; crate::multiband::BANDS],
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

    /// How much later sound leaves the bus than it was mixed, in milliseconds: the lookahead
    /// of its limiter while that is on, and its delay.
    #[must_use]
    pub fn latency_ms(&self) -> f32 {
        let look = if self.limit.on {
            self.limit.lookahead_ms
        } else {
            0.0
        };
        look + self.delay_ms
    }

    /// What holds the sound of this bus back: its fullest ring, itself and its device. A ring
    /// or device that reports nonsense counts as nothing.
    #[must_use]
    pub fn held_back(&self) -> HeldBack {
        let sane = |ms: f32| ms.is_finite() && ms > 0.0;
        HeldBack {
            ring_ms: self
                .rings
                .iter()
                .map(|ring| ring.ms)
                .filter(|ms| sane(*ms))
                .fold(0.0, f32::max),
            bus_ms: self.latency_ms(),
            device_ms: self.device_latency_ms.filter(|ms| sane(*ms)),
        }
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
    /// Takes an input into the automixer or out of it.
    InputAutomix {
        /// Input index.
        input: usize,
        /// In or out.
        on: bool,
    },
    /// Sets the priority of an input in the automixer.
    InputAutomixWeight {
        /// Input index.
        input: usize,
        /// Priority in dB, -12 to 12.
        db: f32,
    },
    /// Changes one setting of the channel strip of an input.
    InputStrip {
        /// Input index.
        input: usize,
        /// The new setting.
        change: crate::strip::StripChange,
    },
    /// Switches the equalizer of an input.
    InputEq {
        /// Input index.
        input: usize,
        /// On or off.
        on: bool,
    },
    /// Changes one setting of one band of the equalizer of an input.
    InputEqBand {
        /// Input index.
        input: usize,
        /// Band index, from 0.
        band: usize,
        /// The new setting.
        change: BandChange,
    },
    /// Puts the equalizer of an input before its gate and compressor, or behind them.
    InputEqPre {
        /// Input index.
        input: usize,
        /// Before them or not.
        pre: bool,
    },
    /// Changes the switch or the ceiling of the limiter of an input; it has no lookahead.
    InputLimit {
        /// Input index.
        input: usize,
        /// The new setting.
        change: LimitChange,
    },
    /// Makes an input a talkback microphone or a plain input again.
    InputTalkback {
        /// Input index.
        input: usize,
        /// Talkback or not.
        on: bool,
    },
    /// Dims a bus or lets it back.
    BusDim {
        /// Bus index.
        bus: usize,
        /// Dimmed or not.
        on: bool,
    },
    /// Sets how far dim turns a bus down.
    BusDimLevel {
        /// Bus index.
        bus: usize,
        /// In dB, [`DIM_RANGE_DB`].
        db: f32,
    },
    /// Opens the talkback microphones and dims the monitor buses, or ends that.
    Talk {
        /// Talking or not.
        on: bool,
    },
    /// Lets solo on a monitor bus listen before the fader (PFL) or after it (AFL).
    Pfl {
        /// Before the fader or not.
        on: bool,
    },
    /// Changes one setting of the multiband compressor of a bus.
    BusMultiband {
        /// Bus index.
        bus: usize,
        /// The new setting.
        change: crate::multiband::MultibandChange,
    },
    /// Changes one setting of the de-esser of an input.
    InputDeess {
        /// Index of the input.
        input: usize,
        /// What to set.
        change: crate::deess::DeessChange,
    },
    /// Changes one setting of the sidechain of an input.
    InputSidechain {
        /// Index of the input.
        input: usize,
        /// What to set.
        change: crate::sidechain::SidechainChange,
    },
    /// Changes one setting of the ducker of an input.
    InputDuck {
        /// Input index.
        input: usize,
        /// The new setting.
        change: crate::duck::DuckChange,
    },
    /// Sets the compressor of a bus by its one knob.
    BusComp {
        /// Bus index.
        bus: usize,
        /// 0 (off) to 10.
        amount: f32,
    },
    /// Sets the mix of the compressor of a bus.
    BusCompMix {
        /// Bus index.
        bus: usize,
        /// 0 (dry) to 100 (all compressed).
        percent: f32,
    },
    /// Sets the sidechain high-pass of the compressor of a bus.
    BusCompHpf {
        /// Bus index.
        bus: usize,
        /// Its corner in Hz.
        hz: f32,
    },
    /// Sets the delay of a bus.
    BusDelay {
        /// Bus index.
        bus: usize,
        /// In milliseconds.
        ms: f32,
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
            | Self::BusGain { ramp_ms, .. }
            | Self::Send { ramp_ms, .. } => *ramp_ms = ms,
            _ => return Err("only a gain, a send or a pan can ramp".to_owned()),
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

/// An input of the demo mixer, with everything else as an input starts.
fn demo_input(
    name: &str,
    kind: SourceKind,
    sample_rate: u32,
    gain_db: f32,
    muted: bool,
    buses: Vec<usize>,
) -> InputView {
    InputView {
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
        automix: false,
        automix_weight_db: 0.0,
        automix_gain_db: 0.0,
        gate_reduction_db: 0.0,
        gate_open: true,
        comp_reduction_db: 0.0,
        strip: crate::strip::StripSettings::default(),
        sidechain: crate::sidechain::SidechainSettings::default(),
        deess: crate::deess::DeessSettings::default(),
        deess_reduction_db: 0.0,
        duck: crate::duck::DuckSettings::default(),
        duck_reduction_db: 0.0,
        talkback: false,
        eq_on: true,
        eq: Band::all_flat(),
        eq_now_db: [0.0; EQ_BANDS],
        eq_pre: false,
        limit_on: crate::limiter::LimitSettings::INPUT.on,
        limit_db: crate::limiter::LimitSettings::INPUT.ceiling_db,
        limited_db: 0.0,
        buses,
        sends: Vec::new(),
        present: true,
        peak_db: None,
        drift_ppm: 0.0,
        dropouts: 0,
    }
}

impl FakeDesk {
    /// The demo mixer: a virtual cable, a microphone and a program on two buses.
    #[must_use]
    pub fn new() -> Self {
        let input = demo_input;
        let bus = |name: &str, device: &str, sample_rate| BusView {
            name: name.to_owned(),
            label: None,
            device: device.to_owned(),
            sample_rate,
            gain_db: 0.0,
            muted: false,
            eq_on: true,
            eq: Band::all_flat(),
            eq_now_db: [0.0; EQ_BANDS],
            limit: LimitSettings::default(),
            monitor: false,
            minus: None,
            loudness: LoudnessView::default(),
            comp: 0.0,
            comp_mix: 100.0,
            comp_hpf_hz: 20.0,
            delay_ms: 0.0,
            rings: Vec::new(),
            device_latency_ms: None,
            dim: false,
            dim_db: DEFAULT_DIM_DB,
            comp_reduction_db: 0.0,
            multiband: crate::multiband::MultibandSettings::default(),
            multiband_reduction_db: [0.0; crate::multiband::BANDS],
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
                talk: false,
                pfl: false,
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

    /// Routes `input` to `bus` or takes it off; an input keeps at least one bus.
    fn set_route(&mut self, input: usize, bus: usize, on: bool) -> Result<Vec<String>, String> {
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

    /// The equalizer and the limiter of an input.
    fn set_input_eq(&mut self, action: &Action) -> Result<Vec<String>, String> {
        let on_off = |on: bool| if on { "on" } else { "off" };
        match *action {
            Action::InputEq { input, on } => {
                self.input_mut(input)?.eq_on = on;
                Ok(vec![format!("[{}] eq {}", input + 1, on_off(on))])
            }
            Action::InputEqBand {
                input,
                band,
                change,
            } => {
                let strip = self.input_mut(input)?;
                let Some(set) = strip.eq.get_mut(band) else {
                    return Err(format!(
                        "there is no band {}; bands are 1 to {EQ_BANDS}",
                        band + 1
                    ));
                };
                *set = set.changed(change)?;
                Ok(vec![format!(
                    "[{}] eq {}: {}",
                    input + 1,
                    band + 1,
                    band_line(*set)
                )])
            }
            Action::InputEqPre { input, pre } => {
                self.input_mut(input)?.eq_pre = pre;
                let place = if pre { "before" } else { "behind" };
                Ok(vec![format!(
                    "[{}] eq {place} gate and compressor",
                    input + 1
                )])
            }
            Action::InputLimit { input, change } => {
                let strip = self.input_mut(input)?;
                match input_limit(change)? {
                    LimitChange::Switch(on) => strip.limit_on = on,
                    LimitChange::Ceiling(db) => strip.limit_db = db,
                    LimitChange::Lookahead(_) => {}
                }
                Ok(vec![format!(
                    "[{}] limit {} at {:.1} dB",
                    input + 1,
                    on_off(strip.limit_on),
                    strip.limit_db
                )])
            }
            _ => Err("not for the equalizer of an input".to_owned()),
        }
    }

    /// The pan of `input`.
    fn set_pan(&mut self, input: usize, pan: f32) -> Result<Vec<String>, String> {
        if !(-1.0..=1.0).contains(&pan) {
            return Err(format!("pan {pan} is outside -1 to 1"));
        }
        self.input_mut(input)?.pan = pan;
        Ok(vec![format!("[{}] pan {pan:.2}", input + 1)])
    }

    /// Dim, talkback and where solo listens.
    fn set_monitoring(&mut self, action: &Action) -> Result<Vec<String>, String> {
        let on_off = |on: bool| if on { "on" } else { "off" };
        match *action {
            Action::InputTalkback { input, on } => {
                self.input_mut(input)?.talkback = on;
                Ok(vec![format!("[{}] talkback {}", input + 1, on_off(on))])
            }
            Action::BusDim { bus, on } => {
                let bus = self.bus_mut(bus)?;
                bus.dim = on;
                Ok(vec![format!("{} dim {}", bus.name, on_off(on))])
            }
            Action::BusDimLevel { bus, db } => {
                let db = check_dim(db)?;
                let bus = self.bus_mut(bus)?;
                bus.dim_db = db;
                Ok(vec![format!("{} dim level {db:.0} dB", bus.name)])
            }
            Action::Talk { on } => {
                self.view.talk = on;
                Ok(vec![format!("talk {}", on_off(on))])
            }
            Action::Pfl { on } => {
                self.view.pfl = on;
                Ok(vec![format!("pfl {}", on_off(on))])
            }
            _ => Err("not for dim or talkback".to_owned()),
        }
    }

    /// One setting of the multiband compressor of `bus`.
    fn set_multiband(
        &mut self,
        bus: usize,
        change: crate::multiband::MultibandChange,
    ) -> Result<Vec<String>, String> {
        let change = change.checked()?;
        let bus = self.bus_mut(bus)?;
        bus.multiband.apply(change);
        Ok(vec![format!("{} {}", bus.name, change.said())])
    }

    /// One setting of the channel strip of `input`.
    fn set_strip(
        &mut self,
        input: usize,
        change: crate::strip::StripChange,
    ) -> Result<Vec<String>, String> {
        let change = change.checked()?;
        self.input_mut(input)?.strip.apply(change);
        Ok(vec![format!("[{}] {}", input + 1, change.said())])
    }

    /// One setting of the de-esser of `input`.
    fn set_deess(
        &mut self,
        input: usize,
        change: crate::deess::DeessChange,
    ) -> Result<Vec<String>, String> {
        let change = change.checked()?;
        self.input_mut(input)?.deess.apply(change);
        Ok(vec![format!("[{}] {}", input + 1, change.said())])
    }

    /// One setting of the sidechain of `input`.
    fn set_sidechain(
        &mut self,
        input: usize,
        change: crate::sidechain::SidechainChange,
    ) -> Result<Vec<String>, String> {
        let change = change.checked()?;
        if let crate::sidechain::SidechainChange::Key(key) = change {
            crate::sidechain::check_key(input, key, self.view.inputs.len())?;
        }
        self.input_mut(input)?.sidechain.apply(change);
        Ok(vec![format!("[{}] {}", input + 1, change.said())])
    }

    /// One setting of the ducker of `input`.
    fn set_duck(
        &mut self,
        input: usize,
        change: crate::duck::DuckChange,
    ) -> Result<Vec<String>, String> {
        let change = change.checked()?;
        if let crate::duck::DuckChange::Key(key) = change {
            crate::duck::check_key(input, key, self.view.inputs.len())?;
        }
        self.input_mut(input)?.duck.apply(change);
        Ok(vec![format!("[{}] {}", input + 1, change.said())])
    }

    /// Takes `input` into the automixer or out of it, and/or sets its priority.
    fn set_automix(
        &mut self,
        input: usize,
        on: Option<bool>,
        weight_db: Option<f32>,
    ) -> Result<Vec<String>, String> {
        if let Some(db) = weight_db
            && !(-12.0..=12.0).contains(&db)
        {
            return Err(format!("an automix weight is -12 to 12 dB, not {db}"));
        }
        let strip = self.input_mut(input)?;
        let mut said = Vec::new();
        if let Some(on) = on {
            strip.automix = on;
            let state = if on { "in" } else { "out of" };
            said.push(format!("[{}] {state} the automixer", input + 1));
        }
        if let Some(db) = weight_db {
            strip.automix_weight_db = db;
            said.push(format!("[{}] automix weight {db:+.1} dB", input + 1));
        }
        Ok(said)
    }

    /// One setting of the compressor of a bus.
    fn set_bus_comp(&mut self, action: &Action) -> Result<Vec<String>, String> {
        let (bus, line) = match *action {
            Action::BusComp { bus, amount } => {
                let amount = check_amount("comp", amount)?;
                self.bus_mut(bus)?.comp = amount;
                (bus, format!("comp {amount:.1}"))
            }
            Action::BusCompMix { bus, percent } => {
                if !(0.0..=100.0).contains(&percent) {
                    return Err(format!("a mix is 0 to 100 %, not {percent}"));
                }
                self.bus_mut(bus)?.comp_mix = percent;
                (bus, format!("comp mix {percent:.0} %"))
            }
            Action::BusCompHpf { bus, hz } => {
                let (low, high) = crate::buscomp::HPF_HZ;
                if !(low..=high).contains(&hz) {
                    return Err(format!(
                        "the sidechain filter is {low} to {high} Hz, not {hz}"
                    ));
                }
                self.bus_mut(bus)?.comp_hpf_hz = hz;
                (bus, format!("comp hpf {hz:.0} Hz"))
            }
            _ => return Err("not a setting of a bus compressor".to_owned()),
        };
        Ok(vec![format!("{}: {line}", self.bus_mut(bus)?.name)])
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
        bus.limit.apply(change);
        Ok(vec![format!("{} {}", bus.name, bus.limit.line())])
    }

    fn bus_mut(&mut self, bus: usize) -> Result<&mut BusView, String> {
        let count = self.view.buses.len();
        self.view
            .buses
            .get_mut(bus)
            .ok_or_else(|| format!("there is no bus {} (only {count})", bus + 1))
    }
}

/// `change` as a change of the limiter of an input, checked: its switch or its ceiling.
///
/// # Errors
///
/// A message for a ceiling outside its range, and for a lookahead: the limiter of an input has
/// none, an input that came later would no longer line up with the others.
pub fn input_limit(change: LimitChange) -> Result<LimitChange, String> {
    match change {
        LimitChange::Lookahead(_) => Err(
            "the limiter of an input has no lookahead: it would no longer line up with the others"
                .to_owned(),
        ),
        other => other.checked(),
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
        // The demo has no sound to listen to: every band stands where it is set.
        let standing = |on: bool, bands: &[Band; EQ_BANDS]| {
            bands.map(|band| if on { band.gain_db } else { 0.0 })
        };
        for input in &mut self.view.inputs {
            input.eq_now_db = standing(input.eq_on, &input.eq);
        }
        for bus in &mut self.view.buses {
            bus.eq_now_db = standing(bus.eq_on, &bus.eq);
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
            Action::InputPan { input, pan, .. } => self.set_pan(input, pan),
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
            Action::BusDelay { bus, ms } => {
                let ms = crate::busdelay::check_delay(ms)?;
                let bus = self.bus_mut(bus)?;
                bus.delay_ms = ms;
                Ok(vec![format!("{} delay {ms:.1} ms", bus.name)])
            }
            Action::BusComp { .. } | Action::BusCompMix { .. } | Action::BusCompHpf { .. } => {
                self.set_bus_comp(&action)
            }
            Action::InputAutomix { input, on } => self.set_automix(input, Some(on), None),
            Action::InputAutomixWeight { input, db } => self.set_automix(input, None, Some(db)),
            Action::InputStrip { input, change } => self.set_strip(input, change),
            Action::InputDuck { input, change } => self.set_duck(input, change),
            Action::InputSidechain { input, change } => self.set_sidechain(input, change),
            Action::InputDeess { input, change } => self.set_deess(input, change),
            Action::BusMultiband { bus, change } => self.set_multiband(bus, change),
            Action::InputEq { .. }
            | Action::InputEqBand { .. }
            | Action::InputEqPre { .. }
            | Action::InputLimit { .. } => self.set_input_eq(&action),
            Action::InputTalkback { .. }
            | Action::BusDim { .. }
            | Action::BusDimLevel { .. }
            | Action::Talk { .. }
            | Action::Pfl { .. } => self.set_monitoring(&action),
            Action::Send { input, bus, db, .. } => self.set_send(input, bus, Some(db), None),
            Action::SendPre { input, bus, pre } => self.set_send(input, bus, None, Some(pre)),
            Action::Route { input, bus, on } => self.set_route(input, bus, on),
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
mod held_back_tests {
    use super::*;
    use crate::{Desk, FakeDesk};

    #[test]
    fn a_bus_is_held_back_by_its_fullest_ring_itself_and_its_device() {
        let mut bus = FakeDesk::new().view().buses.remove(0);
        bus.limit.on = true;
        bus.limit.lookahead_ms = 2.0;
        bus.delay_ms = 10.0;
        bus.rings = vec![
            RingView { input: 0, ms: 30.0 },
            RingView { input: 1, ms: 45.0 },
            RingView {
                input: 2,
                ms: f32::NAN,
            },
        ];
        bus.device_latency_ms = None;
        let held = bus.held_back();
        assert!((held.ring_ms - 45.0).abs() < 1e-6);
        assert!((held.bus_ms - 12.0).abs() < 1e-6);
        assert_eq!(held.device_ms, None);
        assert!((held.total_ms() - 57.0).abs() < 1e-6);
        assert!(held.is_known());
        bus.device_latency_ms = Some(40.0);
        assert!((bus.held_back().total_ms() - 97.0).abs() < 1e-6);
        // A device that reports nonsense is one that has not said.
        bus.device_latency_ms = Some(-3.0);
        assert_eq!(bus.held_back().device_ms, None);
        bus.device_latency_ms = Some(f32::INFINITY);
        assert_eq!(bus.held_back().device_ms, None);
        // A bus that does not play and has neither lookahead nor delay.
        bus.rings.clear();
        bus.limit.on = false;
        bus.delay_ms = 0.0;
        bus.device_latency_ms = None;
        assert!(!bus.held_back().is_known());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_latency_of_a_bus_is_its_delay_and_the_lookahead_of_a_limiter_that_is_on() {
        let mut bus = FakeDesk::new().view().buses.swap_remove(0);
        bus.limit = LimitSettings {
            on: true,
            ceiling_db: -1.0,
            lookahead_ms: 2.0,
        };
        bus.delay_ms = 12.5;
        assert_eq!(bus.latency_ms(), 14.5);
        // Switched off, the limiter drops its delay.
        bus.limit.on = false;
        assert_eq!(bus.latency_ms(), 12.5);
    }

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
        assert!(!view.buses[1].limit.on);
        assert!(view.buses[0].limit.on);
        assert!(mixer.act(limit(LimitChange::Ceiling(3.0))).is_err());
        assert!(mixer.act(limit(LimitChange::Lookahead(6.0))).is_err());
        assert!(mixer.act(limit(LimitChange::Ceiling(f32::NAN))).is_err());
    }

    #[test]
    fn a_line_names_a_limiter_only_when_it_differs_from_its_start() {
        let start = LimitSettings::default();
        assert_eq!(start.summary(), "");
        let off = LimitSettings { on: false, ..start };
        assert_eq!(off.summary(), "  limit off");
        let set = LimitSettings {
            on: true,
            ceiling_db: -6.0,
            lookahead_ms: 0.0,
        };
        assert_eq!(set.summary(), "  limit -6.0 dB, 0 ms");
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
                q: 0.7,
                threshold_db: 0.0,
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
