//! Parameter addresses: what a control is bound to, in the window, a layout file, the hub or
//! a MIDI map alike.
//!
//! An address names one parameter of the mixer as a path, the way the command line names
//! inputs and buses: inputs by their place in the route from 1 (`input/2/gain`), buses by
//! name (`bus/A1/mute`), crosspoints by both (`matrix/2/A1`), the bands of a bus's equalizer
//! by their place from 1 (`bus/A1/eq/2/gain`), the settings of its limiter by name
//! (`bus/A1/limit/ceiling`). A [`Binding`] is an address
//! resolved against a [`MixerView`]; it reads its value from the view and turns a new value
//! into an [`Action`].

use std::fmt;

use crate::deess::{DeessChange, DeessParam};
use crate::duck::{DuckChange, DuckParam};
use crate::eq::{BandChange, BandKind, EQ_BANDS};
use crate::limiter::LimitChange;
use crate::multiband::MultibandParam;
use crate::sidechain::{SidechainChange, SidechainParam};
use crate::strip::StripChange;
use crate::view::{Action, BusView, GAIN_RANGE_DB, InputView, MixerView};

/// A parameter of an input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InputParam {
    /// Its gain.
    Gain,
    /// Its mute.
    Mute,
    /// Its solo.
    Solo,
    /// Its pan.
    Pan,
    /// Its gate, as one knob.
    Gate,
    /// Its compressor, as one knob.
    Comp,
    /// Whether it takes part in the automixer.
    Automix,
    /// Its priority in the automixer, in dB.
    AutomixWeight,
    /// Its polarity, turned over or not.
    Polarity,
    /// The corner of its high-pass, in Hz.
    Hpf,
    /// The corner of its low-pass, in Hz.
    Lpf,
    /// Its delay, in milliseconds.
    Delay,
    /// The width of a stereo input, in percent.
    Width,
    /// One setting of its ducker.
    Duck(DuckParam),
    /// One setting of its sidechain: what its gate and compressor listen to.
    Sidechain(SidechainParam),
    /// One setting of its de-esser.
    Deess(DeessParam),
    /// Whether it is a talkback microphone: silent until someone talks.
    Talkback,
    /// The switch of its equalizer.
    Eq,
    /// One setting of one band of its equalizer, the band from 0.
    EqBand(usize, BandParam),
    /// Whether its equalizer comes before gate and compressor instead of after them.
    EqPre,
    /// The switch of its limiter.
    Limit,
    /// The ceiling of its limiter.
    LimitCeiling,
    /// Its peak, read only.
    Meter,
}

/// A parameter of a bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BusParam {
    /// Its gain.
    Gain,
    /// Its mute.
    Mute,
    /// The switch of its equalizer.
    Eq,
    /// One setting of one band of its equalizer, the band from 0.
    EqBand(usize, BandParam),
    /// The switch of its limiter.
    Limit,
    /// The ceiling of its limiter.
    LimitCeiling,
    /// The lookahead of its limiter.
    LimitLookahead,
    /// Whether it is a monitor bus, the one solo acts on.
    Monitor,
    /// Its dim switch: turned down by its dim level.
    Dim,
    /// How far dim turns it down, in dB.
    DimLevel,
    /// Its compressor, as one knob.
    Comp,
    /// The mix of its compressor, dry to compressed, in percent.
    CompMix,
    /// The corner of the sidechain high-pass of its compressor, in Hz.
    CompHpf,
    /// One setting of its multiband compressor.
    Multiband(MultibandParam),
    /// Its delay, in milliseconds.
    Delay,
    /// Its peak, read only.
    Meter,
}

/// A setting of a band of an equalizer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BandParam {
    /// What it does: `low`, `bell` or `high`.
    Kind,
    /// Its frequency.
    Freq,
    /// Its gain.
    Gain,
    /// Its width.
    Q,
}

impl BandParam {
    fn from_word(word: &str) -> Option<Self> {
        Some(match word {
            "kind" => Self::Kind,
            "freq" => Self::Freq,
            "gain" => Self::Gain,
            "q" => Self::Q,
            _ => return None,
        })
    }

    /// The change that sets it to `value`, when `value` is of its kind.
    fn change(self, value: Value) -> Option<BandChange> {
        Some(match (self, value) {
            (Self::Kind, Value::Kind(kind)) => BandChange::Kind(kind),
            (Self::Freq, Value::Freq(hz)) => BandChange::Freq(hz),
            (Self::Gain, Value::Boost(db)) => BandChange::Gain(db),
            (Self::Q, Value::Width(q)) => BandChange::Q(q),
            _ => return None,
        })
    }

    /// Its value in `band`.
    fn read(self, band: &crate::eq::Band) -> Value {
        match self {
            Self::Kind => Value::Kind(band.kind),
            Self::Freq => Value::Freq(band.freq_hz),
            Self::Gain => Value::Boost(band.gain_db),
            Self::Q => Value::Width(band.q),
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Kind => "kind",
            Self::Freq => "freq",
            Self::Gain => "gain",
            Self::Q => "q",
        }
    }
}

/// One parameter of the mixer, by index into the view's inputs and buses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Binding {
    /// A parameter of an input.
    Input(usize, InputParam),
    /// A parameter of a bus.
    Bus(usize, BusParam),
    /// Whether input `input` feeds bus `bus`.
    Route {
        /// Input index.
        input: usize,
        /// Bus index.
        bus: usize,
    },
    /// The level of the crossing of input `input` and bus `bus`, in dB: `send/<n>/<bus>`.
    Send {
        /// Input index.
        input: usize,
        /// Bus index.
        bus: usize,
    },
    /// Whether that send is taken before the input's fader: `sendpre/<n>/<bus>`.
    SendPre {
        /// Input index.
        input: usize,
        /// Bus index.
        bus: usize,
    },
    /// Whether someone talks into the talkback microphones: `talk`.
    Talk,
    /// Whether solo on a monitor bus listens before the fader (PFL) instead of after it: `pfl`.
    Pfl,
}

/// The value of a parameter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    /// A gain in dB.
    Gain(f32),
    /// A switch: mute, solo, a route.
    Switch(bool),
    /// -1 (left) to 1 (right).
    Pan(f32),
    /// The one knob of a gate or a compressor, 0 (off) to 10.
    Amount(f32),
    /// The gain of a band of an equalizer in dB.
    Boost(f32),
    /// The frequency of a band in Hz.
    Freq(f32),
    /// The width of a band (Q).
    Width(f32),
    /// What a band does.
    Kind(BandKind),
    /// The ceiling of a limiter in dB.
    Ceiling(f32),
    /// The lookahead of a limiter in milliseconds.
    Lookahead(f32),
    /// A share in percent: the mix of a compressor, the width of an input.
    Percent(f32),
    /// A delay in milliseconds.
    Delay(f32),
    /// One setting of a ducker, as the change that sets it.
    Duck(DuckChange),
    /// One setting of a sidechain, as the change that sets it.
    Sidechain(SidechainChange),
    /// One setting of a de-esser, as the change that sets it.
    Deess(DeessChange),
    /// A peak in dBFS; `None` for silence.
    Peak(Option<f32>),
}

/// Why an address does not name a parameter of a view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddressError {
    /// Not of the form `input/<n>/<param>`, `bus/<name>/<param>` or `matrix/<n>/<bus>`.
    Malformed(String),
    /// No such input in the route.
    NoInput(String),
    /// No such bus in the route.
    NoBus(String),
    /// The input or bus has no such parameter.
    NoParam(String),
}

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(address) => write!(f, "not an address: {address}"),
            Self::NoInput(input) => write!(f, "no input {input}"),
            Self::NoBus(bus) => write!(f, "no bus {bus}"),
            Self::NoParam(param) => write!(f, "no parameter {param}"),
        }
    }
}

impl std::error::Error for AddressError {}

impl Binding {
    /// Resolves `address` against `view`.
    ///
    /// # Errors
    ///
    /// When the address is malformed or names an input, bus or parameter `view` does not have.
    pub fn parse(address: &str, view: &MixerView) -> Result<Self, AddressError> {
        let input = |number: &str| {
            place(number)
                .filter(|&index| index < view.inputs.len())
                .ok_or_else(|| AddressError::NoInput(number.to_owned()))
        };
        let bus = |name: &str| {
            view.buses
                .iter()
                .position(|bus| bus.name == name)
                .ok_or_else(|| AddressError::NoBus(name.to_owned()))
        };
        let malformed = || AddressError::Malformed(address.to_owned());
        match address {
            "talk" => return Ok(Self::Talk),
            "pfl" => return Ok(Self::Pfl),
            _ => {}
        }
        // A bus name may hold a slash: it is all between the input or kind and the parameter.
        let (kind, rest) = address.split_once('/').ok_or_else(malformed)?;
        // A band of an equalizer, unless no bus has that name: then the words belong to a name.
        if kind == "bus"
            && let Some((name, number, setting)) = band_address(rest)
            && let Ok(index) = bus(name)
        {
            let band = place(number)
                .filter(|&band| band < EQ_BANDS)
                .ok_or_else(|| AddressError::NoParam(format!("eq/{number}")))?;
            let param = match setting {
                "kind" => BandParam::Kind,
                "freq" => BandParam::Freq,
                "gain" => BandParam::Gain,
                "q" => BandParam::Q,
                other => return Err(AddressError::NoParam(format!("eq/{number}/{other}"))),
            };
            return Ok(Self::Bus(index, BusParam::EqBand(band, param)));
        }
        // A setting of a bus compressor, unless no bus has that name.
        if kind == "bus"
            && let Some((index, param)) = comp_setting(rest, &bus)
        {
            return Ok(Self::Bus(index, param));
        }
        // A setting of a limiter, unless no bus has that name. Anything else is read as a
        // parameter of a bus whose name ends in `limit`.
        if kind == "bus"
            && let Some((before, setting)) = rest.rsplit_once('/')
            && let Some(name) = before.strip_suffix("/limit")
            && let Ok(index) = bus(name)
            && let Some(param) = match setting {
                "ceiling" => Some(BusParam::LimitCeiling),
                "lookahead" => Some(BusParam::LimitLookahead),
                _ => None,
            }
        {
            return Ok(Self::Bus(index, param));
        }
        let (first, last) = match kind {
            "bus" => rest.rsplit_once('/'),
            _ => rest.split_once('/'),
        }
        .ok_or_else(malformed)?;
        match (kind, first, last) {
            ("input", number, param) => {
                let param =
                    input_param(param).ok_or_else(|| AddressError::NoParam(param.to_owned()))?;
                Ok(Self::Input(input(number)?, param))
            }
            ("bus", name, param) => {
                let param = match param {
                    "gain" => BusParam::Gain,
                    "mute" => BusParam::Mute,
                    "eq" => BusParam::Eq,
                    "limit" => BusParam::Limit,
                    "monitor" => BusParam::Monitor,
                    "dim" => BusParam::Dim,
                    "comp" => BusParam::Comp,
                    "delay" => BusParam::Delay,
                    "meter" => BusParam::Meter,
                    other => return Err(AddressError::NoParam(other.to_owned())),
                };
                Ok(Self::Bus(bus(name)?, param))
            }
            ("matrix", number, name) => Ok(Self::Route {
                input: input(number)?,
                bus: bus(name)?,
            }),
            ("send", number, name) => Ok(Self::Send {
                input: input(number)?,
                bus: bus(name)?,
            }),
            ("sendpre", number, name) => Ok(Self::SendPre {
                input: input(number)?,
                bus: bus(name)?,
            }),
            _ => Err(malformed()),
        }
    }

    /// The address in `view`; `None` when the view has no such input or bus.
    #[must_use]
    pub fn address(self, view: &MixerView) -> Option<String> {
        let bus = |index: usize| view.buses.get(index).map(|bus| bus.name.as_str());
        let input = |index: usize| (index < view.inputs.len()).then_some(index + 1);
        Some(match self {
            Self::Input(index, InputParam::EqBand(band, param)) => {
                format!("input/{}/eq/{}/{}", input(index)?, band + 1, param.word())
            }
            Self::Input(index, param) => {
                let param = match param {
                    InputParam::Gain => "gain",
                    InputParam::Mute => "mute",
                    InputParam::Solo => "solo",
                    InputParam::Pan => "pan",
                    InputParam::Gate => "gate",
                    InputParam::Comp => "comp",
                    InputParam::Automix => "automix",
                    InputParam::AutomixWeight => "automix/weight",
                    InputParam::Polarity => "polarity",
                    InputParam::Hpf => "hpf",
                    InputParam::Lpf => "lpf",
                    InputParam::Delay => "delay",
                    InputParam::Width => "width",
                    InputParam::Duck(duck) => duck.address(),
                    InputParam::Sidechain(param) => param.address(),
                    InputParam::Deess(param) => param.address(),
                    InputParam::Talkback => "talkback",
                    InputParam::Eq | InputParam::EqBand(..) => "eq",
                    InputParam::EqPre => "eq/pre",
                    InputParam::Limit => "limit",
                    InputParam::LimitCeiling => "limit/ceiling",
                    InputParam::Meter => "meter",
                };
                format!("input/{}/{param}", input(index)?)
            }
            Self::Bus(index, BusParam::EqBand(band, param)) => {
                format!("bus/{}/eq/{}/{}", bus(index)?, band + 1, param.word())
            }
            Self::Bus(index, BusParam::Multiband(param)) => {
                format!("bus/{}/mb/{}", bus(index)?, param.word())
            }
            Self::Bus(index, param) => {
                let param = match param {
                    BusParam::Gain => "gain",
                    BusParam::Mute => "mute",
                    BusParam::Eq | BusParam::EqBand(..) | BusParam::Multiband(..) => "eq",
                    BusParam::Limit => "limit",
                    BusParam::LimitCeiling => "limit/ceiling",
                    BusParam::LimitLookahead => "limit/lookahead",
                    BusParam::Monitor => "monitor",
                    BusParam::Dim => "dim",
                    BusParam::DimLevel => "dim/level",
                    BusParam::Comp => "comp",
                    BusParam::CompMix => "comp/mix",
                    BusParam::CompHpf => "comp/hpf",
                    BusParam::Delay => "delay",
                    BusParam::Meter => "meter",
                };
                format!("bus/{}/{param}", bus(index)?)
            }
            Self::Route { input: i, bus: b } => format!("matrix/{}/{}", input(i)?, bus(b)?),
            Self::Send { input: i, bus: b } => format!("send/{}/{}", input(i)?, bus(b)?),
            Self::SendPre { input: i, bus: b } => format!("sendpre/{}/{}", input(i)?, bus(b)?),
            Self::Talk => "talk".to_owned(),
            Self::Pfl => "pfl".to_owned(),
        })
    }

    /// The parameter's value in `view`; `None` when the view has no such input or bus.
    #[must_use]
    pub fn read(self, view: &MixerView) -> Option<Value> {
        Some(match self {
            Self::Input(index, param) => {
                let input = view.inputs.get(index)?;
                match param {
                    InputParam::Gain => Value::Gain(input.gain_db),
                    InputParam::Mute => Value::Switch(input.muted),
                    InputParam::Solo => Value::Switch(input.solo),
                    InputParam::Pan => Value::Pan(input.pan),
                    InputParam::Gate => Value::Amount(input.gate),
                    InputParam::Comp => Value::Amount(input.comp),
                    InputParam::Automix => Value::Switch(input.automix),
                    InputParam::AutomixWeight => Value::Gain(input.automix_weight_db),
                    InputParam::Polarity => Value::Switch(input.strip.polarity),
                    InputParam::Hpf => Value::Freq(input.strip.hpf_hz),
                    InputParam::Lpf => Value::Freq(input.strip.lpf_hz),
                    InputParam::Delay => Value::Delay(input.strip.delay_ms),
                    InputParam::Width => Value::Percent(input.strip.width),
                    InputParam::Duck(duck) => Value::Duck(duck.read(&input.duck)),
                    InputParam::Sidechain(param) => Value::Sidechain(param.read(&input.sidechain)),
                    InputParam::Deess(param) => Value::Deess(param.read(&input.deess)),
                    InputParam::Talkback => Value::Switch(input.talkback),
                    InputParam::Eq => Value::Switch(input.eq_on),
                    InputParam::EqBand(band, param) => param.read(input.eq.get(band)?),
                    InputParam::EqPre => Value::Switch(input.eq_pre),
                    InputParam::Limit => Value::Switch(input.limit_on),
                    InputParam::LimitCeiling => Value::Ceiling(input.limit_db),
                    InputParam::Meter => Value::Peak(input.peak_db),
                }
            }
            Self::Bus(index, param) => {
                let bus = view.buses.get(index)?;
                match param {
                    BusParam::Gain => Value::Gain(bus.gain_db),
                    BusParam::Mute => Value::Switch(bus.muted),
                    BusParam::Eq => Value::Switch(bus.eq_on),
                    BusParam::EqBand(band, param) => {
                        let band = bus.eq.get(band)?;
                        match param {
                            BandParam::Kind => Value::Kind(band.kind),
                            BandParam::Freq => Value::Freq(band.freq_hz),
                            BandParam::Gain => Value::Boost(band.gain_db),
                            BandParam::Q => Value::Width(band.q),
                        }
                    }
                    BusParam::Limit => Value::Switch(bus.limit.on),
                    BusParam::LimitCeiling => Value::Ceiling(bus.limit.ceiling_db),
                    BusParam::LimitLookahead => Value::Lookahead(bus.limit.lookahead_ms),
                    BusParam::Monitor => Value::Switch(bus.monitor),
                    BusParam::Dim => Value::Switch(bus.dim),
                    BusParam::DimLevel => Value::Gain(bus.dim_db),
                    BusParam::Comp => Value::Amount(bus.comp),
                    BusParam::CompMix => Value::Percent(bus.comp_mix),
                    BusParam::CompHpf => Value::Freq(bus.comp_hpf_hz),
                    BusParam::Multiband(param @ MultibandParam::Band(_)) => {
                        Value::Amount(param.read(&bus.multiband))
                    }
                    BusParam::Multiband(param) => Value::Freq(param.read(&bus.multiband)),
                    BusParam::Delay => Value::Delay(bus.delay_ms),
                    BusParam::Meter => Value::Peak(bus.peak_db),
                }
            }
            Self::Route { input, bus } => {
                view.buses.get(bus)?;
                Value::Switch(view.inputs.get(input)?.buses.contains(&bus))
            }
            Self::Send { input, bus } => {
                view.buses.get(bus)?;
                Value::Gain(view.inputs.get(input)?.send(bus).db)
            }
            Self::SendPre { input, bus } => {
                view.buses.get(bus)?;
                Value::Switch(view.inputs.get(input)?.send(bus).pre)
            }
            Self::Talk => Value::Switch(view.talk),
            Self::Pfl => Value::Switch(view.pfl),
        })
    }

    /// The action that sets the parameter to `value`; `None` for a meter, which is only read,
    /// and for a value of the wrong kind. A gain below the mixer's range (a fader's off) is
    /// sent as its lowest gain.
    #[must_use]
    pub fn write(self, value: Value) -> Option<Action> {
        Some(match (self, value) {
            (Self::Input(input, InputParam::Gain), Value::Gain(db)) => Action::InputGain {
                input,
                db: action_gain(db),
                ramp_ms: 0,
            },
            (Self::Input(input, InputParam::Mute), Value::Switch(muted)) => {
                Action::InputMute { input, muted }
            }
            (Self::Input(input, InputParam::Solo), Value::Switch(solo)) => {
                Action::InputSolo { input, solo }
            }
            (Self::Input(input, InputParam::Pan), Value::Pan(pan)) => Action::InputPan {
                input,
                pan,
                ramp_ms: 0,
            },
            (Self::Input(input, InputParam::Gate), Value::Amount(amount)) => {
                Action::InputGate { input, amount }
            }
            (Self::Input(input, InputParam::Comp), Value::Amount(amount)) => {
                Action::InputComp { input, amount }
            }
            (Self::Input(input, InputParam::Automix), Value::Switch(on)) => {
                Action::InputAutomix { input, on }
            }
            (Self::Input(input, InputParam::AutomixWeight), Value::Gain(db)) => {
                Action::InputAutomixWeight { input, db }
            }
            (Self::Input(input, InputParam::Duck(duck)), Value::Duck(change))
                if change.param() == duck =>
            {
                Action::InputDuck { input, change }
            }
            (Self::Input(input, param), value) if strip_change(param, value).is_some() => {
                Action::InputStrip {
                    input,
                    change: strip_change(param, value)?,
                }
            }
            (Self::Bus(bus, BusParam::Gain), Value::Gain(db)) => Action::BusGain {
                bus,
                db: action_gain(db),
                ramp_ms: 0,
            },
            (Self::Bus(bus, BusParam::Mute), Value::Switch(muted)) => {
                Action::BusMute { bus, muted }
            }
            (Self::Bus(bus, BusParam::Eq), Value::Switch(on)) => Action::BusEq { bus, on },
            (Self::Bus(bus, BusParam::Monitor), Value::Switch(on)) => {
                Action::BusMonitor { bus, on }
            }
            (Self::Bus(bus, BusParam::Comp), Value::Amount(amount)) => {
                Action::BusComp { bus, amount }
            }
            (Self::Bus(bus, BusParam::CompMix), Value::Percent(percent)) => {
                Action::BusCompMix { bus, percent }
            }
            (Self::Bus(bus, BusParam::CompHpf), Value::Freq(hz)) => Action::BusCompHpf { bus, hz },
            (Self::Bus(bus, BusParam::Multiband(param)), value) => Action::BusMultiband {
                bus,
                change: param.change(multiband_number(param, value)?),
            },
            (Self::Bus(bus, BusParam::Delay), Value::Delay(ms)) => Action::BusDelay { bus, ms },
            (Self::Bus(bus, BusParam::EqBand(band, param)), value) => {
                let change = match (param, value) {
                    (BandParam::Kind, Value::Kind(kind)) => BandChange::Kind(kind),
                    (BandParam::Freq, Value::Freq(hz)) => BandChange::Freq(hz),
                    (BandParam::Gain, Value::Boost(db)) => BandChange::Gain(db),
                    (BandParam::Q, Value::Width(q)) => BandChange::Q(q),
                    _ => return None,
                };
                Action::BusEqBand { bus, band, change }
            }
            (Self::Bus(bus, BusParam::Limit), Value::Switch(on)) => Action::BusLimit {
                bus,
                change: LimitChange::Switch(on),
            },
            (Self::Bus(bus, BusParam::LimitCeiling), Value::Ceiling(db)) => Action::BusLimit {
                bus,
                change: LimitChange::Ceiling(db),
            },
            (Self::Bus(bus, BusParam::LimitLookahead), Value::Lookahead(ms)) => Action::BusLimit {
                bus,
                change: LimitChange::Lookahead(ms),
            },
            (Self::Route { input, bus }, Value::Switch(on)) => Action::Route { input, bus, on },
            (Self::Send { input, bus }, Value::Gain(db)) => Action::Send {
                input,
                bus,
                db: action_gain(db),
                ramp_ms: 0,
            },
            (Self::SendPre { input, bus }, Value::Switch(pre)) => {
                Action::SendPre { input, bus, pre }
            }
            _ => return self.write_monitoring(value),
        })
    }

    /// [`Self::write`] for what belongs to listening and talking: dim and talkback; and for the
    /// sidechain and the de-esser of an input.
    fn write_monitoring(self, value: Value) -> Option<Action> {
        Some(match (self, value) {
            (Self::Input(input, InputParam::Deess(param)), Value::Deess(change))
                if change.param() == param =>
            {
                Action::InputDeess { input, change }
            }
            (Self::Input(input, InputParam::Sidechain(param)), Value::Sidechain(change))
                if change.param() == param =>
            {
                Action::InputSidechain { input, change }
            }
            (Self::Input(input, InputParam::Talkback), Value::Switch(on)) => {
                Action::InputTalkback { input, on }
            }
            (Self::Bus(bus, BusParam::Dim), Value::Switch(on)) => Action::BusDim { bus, on },
            (Self::Bus(bus, BusParam::DimLevel), Value::Gain(db)) => {
                Action::BusDimLevel { bus, db }
            }
            (Self::Talk, Value::Switch(on)) => Action::Talk { on },
            (Self::Pfl, Value::Switch(on)) => Action::Pfl { on },
            (Self::Input(input, InputParam::Eq), Value::Switch(on)) => {
                Action::InputEq { input, on }
            }
            (Self::Input(input, InputParam::EqBand(band, param)), value) => Action::InputEqBand {
                input,
                band,
                change: param.change(value)?,
            },
            (Self::Input(input, InputParam::EqPre), Value::Switch(pre)) => {
                Action::InputEqPre { input, pre }
            }
            (Self::Input(input, InputParam::Limit), Value::Switch(on)) => Action::InputLimit {
                input,
                change: LimitChange::Switch(on),
            },
            (Self::Input(input, InputParam::LimitCeiling), Value::Ceiling(db)) => {
                Action::InputLimit {
                    input,
                    change: LimitChange::Ceiling(db),
                }
            }
            _ => return None,
        })
    }

    /// What a control bound here is called for assistive technology: `Guitar mute`,
    /// `Guitar gate`, `Guitar to A1`; just the owner's name for a gain, pan or meter, whose
    /// controls say what they are themselves. Empty when the view has no such input or bus.
    #[must_use]
    pub fn title(self, view: &MixerView) -> String {
        let input = |index: usize| view.inputs.get(index).map_or("", InputView::shown_name);
        let bus = |index: usize| view.buses.get(index).map_or("", BusView::shown_name);
        match self {
            Self::Input(index, InputParam::Mute) => format!("{} mute", input(index)),
            Self::Input(index, InputParam::Solo) => format!("{} solo", input(index)),
            Self::Input(index, InputParam::Gate) => format!("{} gate", input(index)),
            Self::Input(index, InputParam::Comp) => format!("{} compressor", input(index)),
            Self::Input(index, InputParam::Automix) => format!("{} automix", input(index)),
            Self::Input(index, InputParam::AutomixWeight) => {
                format!("{} automix weight", input(index))
            }
            Self::Input(index, InputParam::Polarity) => format!("{} polarity", input(index)),
            Self::Input(index, InputParam::Hpf) => format!("{} high-pass", input(index)),
            Self::Input(index, InputParam::Lpf) => format!("{} low-pass", input(index)),
            Self::Input(index, InputParam::Delay) => format!("{} delay", input(index)),
            Self::Input(index, InputParam::Width) => format!("{} width", input(index)),
            Self::Input(index, InputParam::Duck(duck)) => {
                let what = match duck {
                    DuckParam::Depth => "ducking depth",
                    DuckParam::Key => "ducking key",
                    DuckParam::Threshold => "ducking threshold",
                    DuckParam::Attack => "ducking attack",
                    DuckParam::Hold => "ducking hold",
                    DuckParam::Release => "ducking release",
                };
                format!("{} {what}", input(index))
            }
            Self::Input(index, InputParam::Sidechain(param)) => {
                let what = match param {
                    SidechainParam::Key => "sidechain key",
                    SidechainParam::Hpf => "sidechain high-pass",
                    SidechainParam::Lpf => "sidechain low-pass",
                };
                format!("{} {what}", input(index))
            }
            Self::Input(index, InputParam::Deess(param)) => {
                format!("{} {}", input(index), param.title())
            }
            Self::Input(index, InputParam::Talkback) => format!("{} talkback", input(index)),
            Self::Input(index, InputParam::Eq) => format!("{} equalizer", input(index)),
            Self::Input(index, InputParam::EqBand(band, param)) => {
                format!(
                    "{} equalizer band {} {}",
                    input(index),
                    band + 1,
                    param.word()
                )
            }
            Self::Input(index, InputParam::EqPre) => {
                format!("{} equalizer before the compressor", input(index))
            }
            Self::Input(index, InputParam::Limit) => format!("{} limiter", input(index)),
            Self::Input(index, InputParam::LimitCeiling) => {
                format!("{} limiter ceiling", input(index))
            }
            Self::Input(index, _) => input(index).to_owned(),
            Self::Bus(index, BusParam::Mute) => format!("{} mute", bus(index)),
            Self::Bus(index, BusParam::Eq) => format!("{} equalizer", bus(index)),
            Self::Bus(index, BusParam::EqBand(band, param)) => {
                format!(
                    "{} equalizer band {} {}",
                    bus(index),
                    band + 1,
                    param.word()
                )
            }
            Self::Bus(index, BusParam::Limit) => format!("{} limiter", bus(index)),
            Self::Bus(index, BusParam::Monitor) => format!("{} monitor", bus(index)),
            Self::Bus(index, BusParam::Dim) => format!("{} dim", bus(index)),
            Self::Bus(index, BusParam::DimLevel) => format!("{} dim level", bus(index)),
            Self::Talk => "talk".to_owned(),
            Self::Pfl => "solo before the fader".to_owned(),
            Self::Bus(index, BusParam::Comp) => format!("{} compressor", bus(index)),
            Self::Bus(index, BusParam::CompMix) => format!("{} compressor mix", bus(index)),
            Self::Bus(index, BusParam::CompHpf) => {
                format!("{} compressor sidechain filter", bus(index))
            }
            Self::Bus(index, BusParam::Delay) => format!("{} delay", bus(index)),
            Self::Bus(index, BusParam::Multiband(param)) => {
                format!("{} multiband {}", bus(index), param.title())
            }
            Self::Bus(index, BusParam::LimitCeiling) => format!("{} limiter ceiling", bus(index)),
            Self::Bus(index, BusParam::LimitLookahead) => {
                format!("{} limiter lookahead", bus(index))
            }
            Self::Bus(index, _) => bus(index).to_owned(),
            Self::Route { input: i, bus: b } => format!("{} to {}", input(i), bus(b)),
            Self::Send { input: i, bus: b } => format!("{} send to {}", input(i), bus(b)),
            Self::SendPre { input: i, bus: b } => {
                format!("{} send to {} before the fader", input(i), bus(b))
            }
        }
    }
}

/// The number `value` gives a setting of a multiband compressor: an amount for a band, a
/// frequency for a split.
fn multiband_number(param: MultibandParam, value: Value) -> Option<f32> {
    match (param, value) {
        (MultibandParam::Band(_), Value::Amount(number))
        | (MultibandParam::LowSplit | MultibandParam::HighSplit, Value::Freq(number)) => {
            Some(number)
        }
        _ => None,
    }
}

/// The parameter of an input an address names after its number.
fn input_param(word: &str) -> Option<InputParam> {
    Some(match word {
        "gain" => InputParam::Gain,
        "mute" => InputParam::Mute,
        "solo" => InputParam::Solo,
        "pan" => InputParam::Pan,
        "gate" => InputParam::Gate,
        "comp" => InputParam::Comp,
        "automix" => InputParam::Automix,
        "automix/weight" => InputParam::AutomixWeight,
        "polarity" => InputParam::Polarity,
        "hpf" => InputParam::Hpf,
        "lpf" => InputParam::Lpf,
        "delay" => InputParam::Delay,
        "width" => InputParam::Width,
        "meter" => InputParam::Meter,
        "talkback" => InputParam::Talkback,
        "eq" => InputParam::Eq,
        "eq/pre" => InputParam::EqPre,
        "limit" => InputParam::Limit,
        "limit/ceiling" => InputParam::LimitCeiling,
        band if band.starts_with("eq/") => {
            let (number, setting) = band.strip_prefix("eq/")?.split_once('/')?;
            let index = place(number).filter(|&index| index < EQ_BANDS)?;
            InputParam::EqBand(index, BandParam::from_word(setting)?)
        }
        other => {
            if let Some(param) = SidechainParam::from_address(other) {
                InputParam::Sidechain(param)
            } else if let Some(param) = DeessParam::from_address(other) {
                InputParam::Deess(param)
            } else {
                InputParam::Duck(DuckParam::from_address(other)?)
            }
        }
    })
}

/// The change of a channel strip that `value` makes of `param`, if it is one.
fn strip_change(param: InputParam, value: Value) -> Option<StripChange> {
    Some(match (param, value) {
        (InputParam::Polarity, Value::Switch(on)) => StripChange::Polarity(on),
        (InputParam::Hpf, Value::Freq(hz)) => StripChange::Hpf(hz),
        (InputParam::Lpf, Value::Freq(hz)) => StripChange::Lpf(hz),
        (InputParam::Delay, Value::Delay(ms)) => StripChange::Delay(ms),
        (InputParam::Width, Value::Percent(percent)) => StripChange::Width(percent),
        _ => return None,
    })
}

/// `<bus>/comp/mix`, `<bus>/comp/hpf` or `<bus>/mb/<setting>` of a bus that `bus` finds, as its
/// index and parameter.
fn comp_setting<E>(
    rest: &str,
    bus: &impl Fn(&str) -> Result<usize, E>,
) -> Option<(usize, BusParam)> {
    let (before, setting) = rest.rsplit_once('/')?;
    if let Some(name) = before.strip_suffix("/mb") {
        let param = MultibandParam::from_word(setting)?;
        return Some((bus(name).ok()?, BusParam::Multiband(param)));
    }
    if let Some(name) = before.strip_suffix("/dim") {
        return (setting == "level").then_some((bus(name).ok()?, BusParam::DimLevel));
    }
    let index = bus(before.strip_suffix("/comp")?).ok()?;
    let param = match setting {
        "mix" => BusParam::CompMix,
        "hpf" => BusParam::CompHpf,
        _ => return None,
    };
    Some((index, param))
}

/// The index a number in an address names: digits only, without a sign or a leading zero (the
/// one form [`Binding::address`] prints), counted from 1.
fn place(number: &str) -> Option<usize> {
    let canonical = number.bytes().all(|b| b.is_ascii_digit()) && !number.starts_with('0');
    number
        .parse::<usize>()
        .ok()
        .filter(|_| canonical)
        .and_then(|n| n.checked_sub(1))
}

/// The bus name, the band number and the setting of `<bus>/eq/<band>/<setting>`.
fn band_address(rest: &str) -> Option<(&str, &str, &str)> {
    let mut parts = rest.rsplitn(4, '/');
    let (setting, band, eq, name) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    (eq == "eq").then_some((name, band, setting))
}

/// The gain an action asks for: the fader's off (-inf) becomes the lowest gain the mixer takes.
#[must_use]
pub fn action_gain(db: f32) -> f32 {
    db.clamp(GAIN_RANGE_DB.0, GAIN_RANGE_DB.1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{Desk, FakeDesk};

    fn view() -> MixerView {
        FakeDesk::new().view()
    }

    /// Every kind of address, with what it reads and how it writes.
    fn every_kind() -> Vec<(&'static str, Binding)> {
        vec![
            ("input/2/gain", Binding::Input(1, InputParam::Gain)),
            ("input/2/mute", Binding::Input(1, InputParam::Mute)),
            ("input/2/solo", Binding::Input(1, InputParam::Solo)),
            ("input/2/pan", Binding::Input(1, InputParam::Pan)),
            ("input/2/gate", Binding::Input(1, InputParam::Gate)),
            ("input/2/comp", Binding::Input(1, InputParam::Comp)),
            ("input/2/meter", Binding::Input(1, InputParam::Meter)),
            (
                "input/2/duck",
                Binding::Input(1, InputParam::Duck(DuckParam::Depth)),
            ),
            (
                "input/2/duck/key",
                Binding::Input(1, InputParam::Duck(DuckParam::Key)),
            ),
            (
                "input/2/duck/threshold",
                Binding::Input(1, InputParam::Duck(DuckParam::Threshold)),
            ),
            (
                "input/2/duck/attack",
                Binding::Input(1, InputParam::Duck(DuckParam::Attack)),
            ),
            (
                "input/2/duck/hold",
                Binding::Input(1, InputParam::Duck(DuckParam::Hold)),
            ),
            (
                "input/2/duck/release",
                Binding::Input(1, InputParam::Duck(DuckParam::Release)),
            ),
            (
                "input/2/sidechain/key",
                Binding::Input(1, InputParam::Sidechain(SidechainParam::Key)),
            ),
            (
                "input/2/sidechain/hpf",
                Binding::Input(1, InputParam::Sidechain(SidechainParam::Hpf)),
            ),
            (
                "input/2/sidechain/lpf",
                Binding::Input(1, InputParam::Sidechain(SidechainParam::Lpf)),
            ),
            (
                "input/2/deess/threshold",
                Binding::Input(1, InputParam::Deess(DeessParam::Threshold)),
            ),
            (
                "input/2/deess/freq",
                Binding::Input(1, InputParam::Deess(DeessParam::Freq)),
            ),
            ("bus/A1/gain", Binding::Bus(0, BusParam::Gain)),
            ("bus/A1/mute", Binding::Bus(0, BusParam::Mute)),
            ("bus/A1/meter", Binding::Bus(0, BusParam::Meter)),
            ("bus/A1/eq", Binding::Bus(0, BusParam::Eq)),
            (
                "bus/A2/eq/1/kind",
                Binding::Bus(1, BusParam::EqBand(0, BandParam::Kind)),
            ),
            (
                "bus/A2/eq/2/freq",
                Binding::Bus(1, BusParam::EqBand(1, BandParam::Freq)),
            ),
            (
                "bus/A2/eq/3/gain",
                Binding::Bus(1, BusParam::EqBand(2, BandParam::Gain)),
            ),
            (
                "bus/A2/eq/4/q",
                Binding::Bus(1, BusParam::EqBand(3, BandParam::Q)),
            ),
            ("bus/A1/limit", Binding::Bus(0, BusParam::Limit)),
            ("bus/A1/delay", Binding::Bus(0, BusParam::Delay)),
            (
                "bus/A2/limit/ceiling",
                Binding::Bus(1, BusParam::LimitCeiling),
            ),
            (
                "bus/A2/limit/lookahead",
                Binding::Bus(1, BusParam::LimitLookahead),
            ),
            ("matrix/2/A1", Binding::Route { input: 1, bus: 0 }),
            ("send/2/A2", Binding::Send { input: 1, bus: 1 }),
            ("sendpre/1/A1", Binding::SendPre { input: 0, bus: 0 }),
        ]
    }

    #[test]
    fn a_send_reads_and_writes_its_crossing() {
        let mut view = view();
        view.inputs[1].send_mut(1).db = -6.0;
        view.inputs[1].send_mut(1).pre = true;
        let send = Binding::Send { input: 1, bus: 1 };
        let pre = Binding::SendPre { input: 1, bus: 1 };
        assert_eq!(send.read(&view), Some(Value::Gain(-6.0)));
        assert_eq!(pre.read(&view), Some(Value::Switch(true)));
        assert_eq!(
            Binding::Send { input: 0, bus: 0 }.read(&view),
            Some(Value::Gain(0.0)),
            "a crossing without an entry is at 0 dB"
        );
        assert_eq!(
            send.write(Value::Gain(-3.0)),
            Some(Action::Send {
                input: 1,
                bus: 1,
                db: -3.0,
                ramp_ms: 0
            })
        );
        assert_eq!(
            pre.write(Value::Switch(false)),
            Some(Action::SendPre {
                input: 1,
                bus: 1,
                pre: false
            })
        );
        assert_eq!(
            send.write(Value::Switch(true)),
            None,
            "a send takes a level"
        );
    }

    #[test]
    fn a_de_esser_reads_and_writes_its_settings() {
        let mut view = FakeDesk::new().view();
        view.inputs[0].deess.apply(DeessChange::Threshold(-30.0));
        let threshold = Binding::Input(0, InputParam::Deess(DeessParam::Threshold));
        let freq = Binding::Input(0, InputParam::Deess(DeessParam::Freq));
        assert_eq!(
            threshold.read(&view),
            Some(Value::Deess(DeessChange::Threshold(-30.0)))
        );
        assert_eq!(
            freq.read(&view),
            Some(Value::Deess(DeessChange::Freq(6000.0)))
        );
        assert_eq!(
            freq.write(Value::Deess(DeessChange::Freq(8000.0))),
            Some(Action::InputDeess {
                input: 0,
                change: DeessChange::Freq(8000.0)
            })
        );
        // A value of another setting is not for this address.
        assert_eq!(freq.write(Value::Deess(DeessChange::Threshold(-3.0))), None);
        assert_eq!(
            threshold.title(&view),
            format!("{} de-esser threshold", view.inputs[0].name)
        );
    }

    #[test]
    fn a_sidechain_reads_and_writes_its_settings() {
        let mut view = FakeDesk::new().view();
        view.inputs[0].sidechain.apply(SidechainChange::Key(2));
        let key = Binding::Input(0, InputParam::Sidechain(SidechainParam::Key));
        let hpf = Binding::Input(0, InputParam::Sidechain(SidechainParam::Hpf));
        assert_eq!(
            key.read(&view),
            Some(Value::Sidechain(SidechainChange::Key(2)))
        );
        assert_eq!(
            hpf.read(&view),
            Some(Value::Sidechain(SidechainChange::Hpf(20.0)))
        );
        assert_eq!(
            hpf.write(Value::Sidechain(SidechainChange::Hpf(4000.0))),
            Some(Action::InputSidechain {
                input: 0,
                change: SidechainChange::Hpf(4000.0)
            })
        );
        // A value of another setting is not for this address.
        assert_eq!(hpf.write(Value::Sidechain(SidechainChange::Key(3))), None);
        assert_eq!(
            key.title(&view),
            format!("{} sidechain key", view.inputs[0].name)
        );
    }

    #[test]
    fn a_ducker_reads_and_writes_its_settings() {
        let mut view = view();
        view.inputs[0].duck.apply(DuckChange::Key(2));
        let depth = Binding::Input(0, InputParam::Duck(DuckParam::Depth));
        let key = Binding::Input(0, InputParam::Duck(DuckParam::Key));
        assert_eq!(
            depth.read(&view),
            Some(Value::Duck(DuckChange::Depth(12.0)))
        );
        assert_eq!(key.read(&view), Some(Value::Duck(DuckChange::Key(2))));
        assert_eq!(
            key.write(Value::Duck(DuckChange::Key(3))),
            Some(Action::InputDuck {
                input: 0,
                change: DuckChange::Key(3)
            })
        );
        assert_eq!(
            depth.write(Value::Duck(DuckChange::Key(3))),
            None,
            "a depth takes no key"
        );
        assert_eq!(
            depth.title(&view),
            format!("{} ducking depth", view.inputs[0].name)
        );
    }

    #[test]
    fn addresses_parse_and_print_back() {
        let view = view();
        for (address, binding) in every_kind() {
            assert_eq!(Binding::parse(address, &view), Ok(binding), "{address}");
            assert_eq!(binding.address(&view).as_deref(), Some(address));
        }
    }

    #[test]
    fn bad_addresses_say_why() {
        let view = view();
        let error = |address| Binding::parse(address, &view).unwrap_err();
        assert_eq!(error("input/0/gain"), AddressError::NoInput("0".into()));
        assert_eq!(error("input/99/gain"), AddressError::NoInput("99".into()));
        assert_eq!(error("input/x/gain"), AddressError::NoInput("x".into()));
        assert_eq!(error("input/+2/gain"), AddressError::NoInput("+2".into()));
        assert_eq!(error("input/02/gain"), AddressError::NoInput("02".into()));
        assert_eq!(
            error("nothing/2/gain"),
            AddressError::Malformed("nothing/2/gain".into())
        );
        assert_eq!(error("bus/Z9/mute"), AddressError::NoBus("Z9".into()));
        assert_eq!(error("bus/A1/solo"), AddressError::NoParam("solo".into()));
        assert_eq!(error("matrix/2/Z9"), AddressError::NoBus("Z9".into()));
        assert_eq!(
            error("bus/A1/eq/5/gain"),
            AddressError::NoParam("eq/5".into())
        );
        assert_eq!(
            error("bus/A1/eq/0/gain"),
            AddressError::NoParam("eq/0".into())
        );
        assert_eq!(
            error("bus/A1/eq/2/slope"),
            AddressError::NoParam("eq/2/slope".into())
        );
        // No bus of that name: the words are read as a name, and that bus is missing too.
        assert_eq!(
            error("bus/Z9/eq/2/gain"),
            AddressError::NoBus("Z9/eq/2".into())
        );
        assert_eq!(
            error("bus/A1/limit/slope"),
            AddressError::NoParam("slope".into())
        );
        assert_eq!(
            error("bus/Z9/limit/ceiling"),
            AddressError::NoParam("ceiling".into())
        );
        assert_eq!(error("input/2"), AddressError::Malformed("input/2".into()));
        assert_eq!(error(""), AddressError::Malformed(String::new()));
    }

    #[test]
    fn only_its_settings_belong_to_a_limiter_when_a_bus_ends_in_limit() {
        let mut view = view();
        view.buses[0].name = "PC".to_owned();
        view.buses[1].name = "PC/limit".to_owned();
        let parse = |address| Binding::parse(address, &view);
        assert_eq!(
            parse("bus/PC/limit/mute"),
            Ok(Binding::Bus(1, BusParam::Mute))
        );
        assert_eq!(
            parse("bus/PC/limit/ceiling"),
            Ok(Binding::Bus(0, BusParam::LimitCeiling))
        );
    }

    #[test]
    fn a_bus_name_may_hold_a_slash() {
        let mut view = view();
        view.buses[1].name = "PC/Mic".to_owned();
        for (address, binding) in [
            ("bus/PC/Mic/mute", Binding::Bus(1, BusParam::Mute)),
            ("bus/PC/Mic/eq", Binding::Bus(1, BusParam::Eq)),
            ("bus/PC/Mic/limit", Binding::Bus(1, BusParam::Limit)),
            (
                "bus/PC/Mic/limit/ceiling",
                Binding::Bus(1, BusParam::LimitCeiling),
            ),
            (
                "bus/PC/Mic/eq/2/gain",
                Binding::Bus(1, BusParam::EqBand(1, BandParam::Gain)),
            ),
            ("matrix/1/PC/Mic", Binding::Route { input: 0, bus: 1 }),
        ] {
            assert_eq!(Binding::parse(address, &view), Ok(binding), "{address}");
            assert_eq!(binding.address(&view).as_deref(), Some(address));
        }
    }

    #[test]
    fn bindings_read_the_view() {
        let mut view = view();
        let input = &mut view.inputs[1];
        input.gain_db = -3.0;
        input.muted = true;
        input.solo = false;
        input.pan = 0.25;
        input.gate = 2.0;
        input.comp = 7.5;
        input.peak_db = Some(-12.0);
        input.buses = vec![0];
        let bus = &mut view.buses[0];
        bus.gain_db = -1.5;
        bus.muted = false;
        bus.peak_db = None;
        bus.eq_on = false;
        bus.eq[1] = crate::eq::Band {
            kind: BandKind::HighShelf,
            freq_hz: 3000.0,
            gain_db: -4.5,
            q: 1.4,
        };
        let read = |address| Binding::parse(address, &view).unwrap().read(&view);
        assert_eq!(read("bus/A1/eq"), Some(Value::Switch(false)));
        assert_eq!(
            read("bus/A1/eq/2/kind"),
            Some(Value::Kind(BandKind::HighShelf))
        );
        assert_eq!(read("bus/A1/eq/2/freq"), Some(Value::Freq(3000.0)));
        assert_eq!(read("bus/A1/eq/2/gain"), Some(Value::Boost(-4.5)));
        assert_eq!(read("bus/A1/eq/2/q"), Some(Value::Width(1.4)));
        assert_eq!(read("input/2/gain"), Some(Value::Gain(-3.0)));
        assert_eq!(read("input/2/mute"), Some(Value::Switch(true)));
        assert_eq!(read("input/2/solo"), Some(Value::Switch(false)));
        assert_eq!(read("input/2/pan"), Some(Value::Pan(0.25)));
        assert_eq!(read("input/2/gate"), Some(Value::Amount(2.0)));
        assert_eq!(read("input/2/comp"), Some(Value::Amount(7.5)));
        assert_eq!(read("input/2/meter"), Some(Value::Peak(Some(-12.0))));
        assert_eq!(read("bus/A1/gain"), Some(Value::Gain(-1.5)));
        assert_eq!(read("bus/A1/mute"), Some(Value::Switch(false)));
        assert_eq!(read("bus/A1/meter"), Some(Value::Peak(None)));
        assert_eq!(read("matrix/2/A1"), Some(Value::Switch(true)));
        let other = view.buses[1].name.clone();
        assert_eq!(
            read(&format!("matrix/2/{other}")),
            Some(Value::Switch(false))
        );
    }

    #[test]
    fn bindings_write_actions() {
        let write = |binding: Binding, value| binding.write(value);
        assert_eq!(
            write(Binding::Input(1, InputParam::Gain), Value::Gain(-3.0)),
            Some(Action::InputGain {
                input: 1,
                db: -3.0,
                ramp_ms: 0
            })
        );
        assert_eq!(
            write(Binding::Input(1, InputParam::Mute), Value::Switch(true)),
            Some(Action::InputMute {
                input: 1,
                muted: true
            })
        );
        assert_eq!(
            write(Binding::Input(1, InputParam::Solo), Value::Switch(true)),
            Some(Action::InputSolo {
                input: 1,
                solo: true
            })
        );
        assert_eq!(
            write(Binding::Input(1, InputParam::Pan), Value::Pan(-0.5)),
            Some(Action::InputPan {
                input: 1,
                pan: -0.5,
                ramp_ms: 0
            })
        );
        assert_eq!(
            write(Binding::Input(1, InputParam::Gate), Value::Amount(4.0)),
            Some(Action::InputGate {
                input: 1,
                amount: 4.0
            })
        );
        assert_eq!(
            write(Binding::Input(1, InputParam::Comp), Value::Amount(0.0)),
            Some(Action::InputComp {
                input: 1,
                amount: 0.0
            })
        );
        assert_eq!(
            write(
                Binding::Bus(0, BusParam::Gain),
                Value::Gain(f32::NEG_INFINITY)
            ),
            Some(Action::BusGain {
                bus: 0,
                db: GAIN_RANGE_DB.0,
                ramp_ms: 0
            })
        );
        assert_eq!(
            write(Binding::Bus(0, BusParam::Mute), Value::Switch(true)),
            Some(Action::BusMute {
                bus: 0,
                muted: true
            })
        );
        assert_eq!(
            write(Binding::Route { input: 1, bus: 0 }, Value::Switch(false)),
            Some(Action::Route {
                input: 1,
                bus: 0,
                on: false
            })
        );
    }

    #[test]
    fn bindings_of_an_equalizer_write_its_actions() {
        let write = |binding: Binding, value| binding.write(value);
        assert_eq!(
            write(Binding::Bus(0, BusParam::Eq), Value::Switch(false)),
            Some(Action::BusEq { bus: 0, on: false })
        );
        let band = |param| Binding::Bus(1, BusParam::EqBand(2, param));
        let change = |change| {
            Some(Action::BusEqBand {
                bus: 1,
                band: 2,
                change,
            })
        };
        assert_eq!(
            write(band(BandParam::Gain), Value::Boost(6.0)),
            change(BandChange::Gain(6.0))
        );
        assert_eq!(
            write(band(BandParam::Freq), Value::Freq(440.0)),
            change(BandChange::Freq(440.0))
        );
        assert_eq!(
            write(band(BandParam::Q), Value::Width(2.0)),
            change(BandChange::Q(2.0))
        );
        assert_eq!(
            write(band(BandParam::Kind), Value::Kind(BandKind::LowShelf)),
            change(BandChange::Kind(BandKind::LowShelf))
        );
        // A value of another setting writes nothing.
        assert_eq!(write(band(BandParam::Gain), Value::Freq(440.0)), None);
        assert_eq!(write(band(BandParam::Kind), Value::Gain(0.0)), None);
    }

    #[test]
    fn bindings_of_a_limiter_read_the_view_and_write_its_action() {
        let mut view = crate::view::FakeDesk::new().view();
        view.buses[0].limit.on = false;
        view.buses[0].limit.ceiling_db = -3.0;
        view.buses[0].limit.lookahead_ms = 2.0;
        let read = |param| Binding::Bus(0, param).read(&view);
        assert_eq!(read(BusParam::Limit), Some(Value::Switch(false)));
        assert_eq!(read(BusParam::LimitCeiling), Some(Value::Ceiling(-3.0)));
        assert_eq!(read(BusParam::LimitLookahead), Some(Value::Lookahead(2.0)));
        let write = |param, value| Binding::Bus(1, param).write(value);
        let action = |change| Some(Action::BusLimit { bus: 1, change });
        assert_eq!(
            write(BusParam::Limit, Value::Switch(true)),
            action(LimitChange::Switch(true))
        );
        assert_eq!(
            write(BusParam::LimitCeiling, Value::Ceiling(-6.0)),
            action(LimitChange::Ceiling(-6.0))
        );
        assert_eq!(
            write(BusParam::LimitLookahead, Value::Lookahead(1.5)),
            action(LimitChange::Lookahead(1.5))
        );
        // A value of another setting writes nothing.
        assert_eq!(write(BusParam::LimitCeiling, Value::Lookahead(1.5)), None);
    }

    #[test]
    fn meters_and_wrong_values_write_nothing() {
        let peak = Value::Peak(Some(0.0));
        assert_eq!(Binding::Input(0, InputParam::Meter).write(peak), None);
        assert_eq!(Binding::Bus(0, BusParam::Meter).write(peak), None);
        assert_eq!(
            Binding::Input(0, InputParam::Mute).write(Value::Gain(0.0)),
            None
        );
        assert_eq!(
            Binding::Route { input: 0, bus: 0 }.write(Value::Pan(0.0)),
            None
        );
    }

    #[test]
    fn a_binding_past_the_view_reads_nothing() {
        let view = view();
        let past = view.inputs.len();
        assert_eq!(Binding::Input(past, InputParam::Gain).read(&view), None);
        assert_eq!(
            Binding::Bus(view.buses.len(), BusParam::Mute).read(&view),
            None
        );
        assert_eq!(Binding::Route { input: 0, bus: 99 }.read(&view), None);
        assert_eq!(Binding::Input(past, InputParam::Gain).address(&view), None);
    }

    #[test]
    fn titles_name_the_control() {
        let view = view();
        let input = &view.inputs[1].name;
        let bus = &view.buses[0].name;
        assert_eq!(
            Binding::Input(1, InputParam::Mute).title(&view),
            format!("{input} mute")
        );
        assert_eq!(
            Binding::Input(1, InputParam::Solo).title(&view),
            format!("{input} solo")
        );
        assert_eq!(Binding::Input(1, InputParam::Gain).title(&view), *input);
        assert_eq!(
            Binding::Input(1, InputParam::Gate).title(&view),
            format!("{input} gate")
        );
        assert_eq!(
            Binding::Input(1, InputParam::Comp).title(&view),
            format!("{input} compressor")
        );
        assert_eq!(
            Binding::Bus(0, BusParam::Mute).title(&view),
            format!("{bus} mute")
        );
        assert_eq!(
            Binding::Route { input: 1, bus: 0 }.title(&view),
            format!("{input} to {bus}")
        );
        assert_eq!(
            Binding::Bus(0, BusParam::Eq).title(&view),
            format!("{bus} equalizer")
        );
        assert_eq!(
            Binding::Bus(0, BusParam::EqBand(1, BandParam::Gain)).title(&view),
            format!("{bus} equalizer band 2 gain")
        );
    }

    #[test]
    fn titles_use_the_label() {
        let mut view = view();
        view.inputs[1].label = Some("Musik".to_owned());
        view.buses[0].label = Some("Boxen".to_owned());
        assert_eq!(
            Binding::Input(1, InputParam::Mute).title(&view),
            "Musik mute"
        );
        assert_eq!(
            Binding::Route { input: 1, bus: 0 }.title(&view),
            "Musik to Boxen"
        );
    }

    #[test]
    fn off_is_sent_as_the_lowest_gain() {
        assert_eq!(action_gain(f32::NEG_INFINITY), GAIN_RANGE_DB.0);
        assert_eq!(action_gain(-6.5), -6.5);
        assert_eq!(action_gain(40.0), GAIN_RANGE_DB.1);
    }
}
