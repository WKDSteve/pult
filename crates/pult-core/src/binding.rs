//! Parameter addresses: what a control is bound to, in the window, a layout file, the hub or
//! a MIDI map alike.
//!
//! An address names one parameter of the mixer as a path, the way the command line names
//! inputs and buses: inputs by their place in the route from 1 (`input/2/gain`), buses by
//! name (`bus/A1/mute`), crosspoints by both (`matrix/2/A1`). A [`Binding`] is an address
//! resolved against a [`MixerView`]; it reads its value from the view and turns a new value
//! into an [`Action`].

use std::fmt;

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
    /// Its peak, read only.
    Meter,
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
            // Only the form `address` prints: digits, no sign, no leading zero.
            let canonical = number.bytes().all(|b| b.is_ascii_digit()) && !number.starts_with('0');
            number
                .parse::<usize>()
                .ok()
                .filter(|_| canonical)
                .and_then(|n| n.checked_sub(1))
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
        // A bus name may hold a slash: it is all between the input or kind and the parameter.
        let (kind, rest) = address.split_once('/').ok_or_else(malformed)?;
        let (first, last) = match kind {
            "bus" => rest.rsplit_once('/'),
            _ => rest.split_once('/'),
        }
        .ok_or_else(malformed)?;
        match (kind, first, last) {
            ("input", number, param) => {
                let param = match param {
                    "gain" => InputParam::Gain,
                    "mute" => InputParam::Mute,
                    "solo" => InputParam::Solo,
                    "pan" => InputParam::Pan,
                    "meter" => InputParam::Meter,
                    other => return Err(AddressError::NoParam(other.to_owned())),
                };
                Ok(Self::Input(input(number)?, param))
            }
            ("bus", name, param) => {
                let param = match param {
                    "gain" => BusParam::Gain,
                    "mute" => BusParam::Mute,
                    "meter" => BusParam::Meter,
                    other => return Err(AddressError::NoParam(other.to_owned())),
                };
                Ok(Self::Bus(bus(name)?, param))
            }
            ("matrix", number, name) => Ok(Self::Route {
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
            Self::Input(index, param) => {
                let param = match param {
                    InputParam::Gain => "gain",
                    InputParam::Mute => "mute",
                    InputParam::Solo => "solo",
                    InputParam::Pan => "pan",
                    InputParam::Meter => "meter",
                };
                format!("input/{}/{param}", input(index)?)
            }
            Self::Bus(index, param) => {
                let param = match param {
                    BusParam::Gain => "gain",
                    BusParam::Mute => "mute",
                    BusParam::Meter => "meter",
                };
                format!("bus/{}/{param}", bus(index)?)
            }
            Self::Route { input: i, bus: b } => format!("matrix/{}/{}", input(i)?, bus(b)?),
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
                    InputParam::Meter => Value::Peak(input.peak_db),
                }
            }
            Self::Bus(index, param) => {
                let bus = view.buses.get(index)?;
                match param {
                    BusParam::Gain => Value::Gain(bus.gain_db),
                    BusParam::Mute => Value::Switch(bus.muted),
                    BusParam::Meter => Value::Peak(bus.peak_db),
                }
            }
            Self::Route { input, bus } => {
                view.buses.get(bus)?;
                Value::Switch(view.inputs.get(input)?.buses.contains(&bus))
            }
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
            },
            (Self::Input(input, InputParam::Mute), Value::Switch(muted)) => {
                Action::InputMute { input, muted }
            }
            (Self::Input(input, InputParam::Solo), Value::Switch(solo)) => {
                Action::InputSolo { input, solo }
            }
            (Self::Input(input, InputParam::Pan), Value::Pan(pan)) => {
                Action::InputPan { input, pan }
            }
            (Self::Bus(bus, BusParam::Gain), Value::Gain(db)) => Action::BusGain {
                bus,
                db: action_gain(db),
            },
            (Self::Bus(bus, BusParam::Mute), Value::Switch(muted)) => {
                Action::BusMute { bus, muted }
            }
            (Self::Route { input, bus }, Value::Switch(on)) => Action::Route { input, bus, on },
            _ => return None,
        })
    }

    /// What a control bound here is called for assistive technology: `Guitar mute`,
    /// `Guitar to A1`; just the owner's name for a gain, pan or meter, whose controls say what
    /// they are themselves. Empty when the view has no such input or bus.
    #[must_use]
    pub fn title(self, view: &MixerView) -> String {
        let input = |index: usize| view.inputs.get(index).map_or("", InputView::shown_name);
        let bus = |index: usize| view.buses.get(index).map_or("", BusView::shown_name);
        match self {
            Self::Input(index, InputParam::Mute) => format!("{} mute", input(index)),
            Self::Input(index, InputParam::Solo) => format!("{} solo", input(index)),
            Self::Input(index, _) => input(index).to_owned(),
            Self::Bus(index, BusParam::Mute) => format!("{} mute", bus(index)),
            Self::Bus(index, _) => bus(index).to_owned(),
            Self::Route { input: i, bus: b } => format!("{} to {}", input(i), bus(b)),
        }
    }
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
            ("input/2/meter", Binding::Input(1, InputParam::Meter)),
            ("bus/A1/gain", Binding::Bus(0, BusParam::Gain)),
            ("bus/A1/mute", Binding::Bus(0, BusParam::Mute)),
            ("bus/A1/meter", Binding::Bus(0, BusParam::Meter)),
            ("matrix/2/A1", Binding::Route { input: 1, bus: 0 }),
        ]
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
        assert_eq!(error("input/2"), AddressError::Malformed("input/2".into()));
        assert_eq!(error(""), AddressError::Malformed(String::new()));
    }

    #[test]
    fn a_bus_name_may_hold_a_slash() {
        let mut view = view();
        view.buses[1].name = "PC/Mic".to_owned();
        for (address, binding) in [
            ("bus/PC/Mic/mute", Binding::Bus(1, BusParam::Mute)),
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
        input.peak_db = Some(-12.0);
        input.buses = vec![0];
        let bus = &mut view.buses[0];
        bus.gain_db = -1.5;
        bus.muted = false;
        bus.peak_db = None;
        let read = |address| Binding::parse(address, &view).unwrap().read(&view);
        assert_eq!(read("input/2/gain"), Some(Value::Gain(-3.0)));
        assert_eq!(read("input/2/mute"), Some(Value::Switch(true)));
        assert_eq!(read("input/2/solo"), Some(Value::Switch(false)));
        assert_eq!(read("input/2/pan"), Some(Value::Pan(0.25)));
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
            Some(Action::InputGain { input: 1, db: -3.0 })
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
                pan: -0.5
            })
        );
        assert_eq!(
            write(
                Binding::Bus(0, BusParam::Gain),
                Value::Gain(f32::NEG_INFINITY)
            ),
            Some(Action::BusGain {
                bus: 0,
                db: GAIN_RANGE_DB.0
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
            Binding::Bus(0, BusParam::Mute).title(&view),
            format!("{bus} mute")
        );
        assert_eq!(
            Binding::Route { input: 1, bus: 0 }.title(&view),
            format!("{input} to {bus}")
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
