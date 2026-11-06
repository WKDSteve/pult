//! The console as a list of elements, each bound to a parameter address.
//!
//! An element is a control (fader, meter, knob, toggle, a label) with a place and a
//! [`Binding`]; a channel strip is an element made of elements, each in a zone of the strip.
//! The standard console is such a list, built here in code; a layout file can bring another
//! one later. Top-level elements are strips, in the row of inputs or the bus section; free
//! places on the surface come with the layout editor.

use crate::binding::{Binding, BusParam, InputParam};
use crate::widgets::meter::MeterState;
use crate::widgets::toggle::Role;
use pult_core::view::MixerView;

/// Where an element sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// In the row of input strips, which scrolls sideways.
    InputRow,
    /// In the bus section on the right.
    BusSection,
    /// In a zone of the strip around it.
    Zone(Zone),
}

/// The zones of a strip from top to bottom; every strip has them all, so faders and meters of
/// all strips line up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Header,
    Pan,
    /// The bus assignment buttons.
    Assign,
    /// Gate and compressor of an input, the switch of the equalizer of a bus.
    Dynamics,
    MuteSolo,
    /// Fader, meter and meter numbers.
    Fader,
    Readout,
    /// Pan and assignment as one: where a bus strip names its device.
    Info,
}

/// What a strip belongs to: its header, readout note and device come from there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Input(usize),
    Bus(usize),
}

/// The look of a strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripStyle {
    /// On the surface; veiled while its device is gone.
    Input,
    /// Raised, as a master strip.
    Bus,
}

/// A strip: elements in its zones, drawn in order; neighbours in the same zone share it.
#[derive(Clone, Debug, PartialEq)]
pub struct Strip {
    pub owner: Owner,
    pub style: StripStyle,
    pub parts: Vec<Element>,
}

/// A control of the console.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    /// A strip made of elements.
    Strip(Strip),
    /// The owner's name and detail, `missing` while it is gone.
    Header,
    /// A pan knob.
    Knob,
    /// A pan as text.
    PanText,
    /// A small dial with a short `text` over it and its value under it.
    Dial {
        text: String,
    },
    /// A latching button with a short `text`.
    Toggle {
        text: String,
        role: Role,
    },
    Fader,
    Meter,
    /// The numbers beside a meter.
    MeterScale,
    /// A gain as text, with a note on the owner under it.
    Readout,
    /// The output device of a bus.
    Device,
}

/// One element: a control, where it sits and what it is bound to.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub place: Place,
    pub control: Control,
    pub bind: Option<Binding>,
}

impl Element {
    fn part(zone: Zone, control: Control, bind: Option<Binding>) -> Self {
        Self {
            place: Place::Zone(zone),
            control,
            bind,
        }
    }

    fn toggle(zone: Zone, text: &str, role: Role, bind: Binding) -> Self {
        let text = text.to_owned();
        Self::part(zone, Control::Toggle { text, role }, Some(bind))
    }
}

/// The standard console for `view`: a strip per input, then a strip per bus.
#[must_use]
pub fn standard(view: &MixerView) -> Vec<Element> {
    let inputs = (0..view.inputs.len()).map(|index| input_strip(index, view));
    let buses = (0..view.buses.len()).map(bus_strip);
    inputs.chain(buses).collect()
}

/// The strip of input `index`: header, pan, a button per bus of `view`, gate and compressor, mute
/// and solo, fader with meter, readout.
#[must_use]
pub fn input_strip(index: usize, view: &MixerView) -> Element {
    let at = |param| Some(Binding::Input(index, param));
    let mut parts = vec![
        Element::part(Zone::Header, Control::Header, None),
        Element::part(Zone::Pan, Control::Knob, at(InputParam::Pan)),
        Element::part(Zone::Pan, Control::PanText, at(InputParam::Pan)),
    ];
    parts.extend(view.buses.iter().enumerate().map(|(bus, b)| {
        let route = Binding::Route { input: index, bus };
        Element::toggle(Zone::Assign, &b.name, Role::Assign, route)
    }));
    parts.extend([
        Element::part(
            Zone::Dynamics,
            Control::Dial {
                text: "GATE".to_owned(),
            },
            at(InputParam::Gate),
        ),
        Element::part(
            Zone::Dynamics,
            Control::Dial {
                text: "COMP".to_owned(),
            },
            at(InputParam::Comp),
        ),
        Element::toggle(
            Zone::MuteSolo,
            "M",
            Role::Mute,
            Binding::Input(index, InputParam::Mute),
        ),
        Element::toggle(
            Zone::MuteSolo,
            "S",
            Role::Solo,
            Binding::Input(index, InputParam::Solo),
        ),
        Element::part(Zone::Fader, Control::Fader, at(InputParam::Gain)),
        Element::part(Zone::Fader, Control::Meter, at(InputParam::Meter)),
        Element::part(Zone::Readout, Control::Readout, at(InputParam::Gain)),
    ]);
    Element {
        place: Place::InputRow,
        control: Control::Strip(Strip {
            owner: Owner::Input(index),
            style: StripStyle::Input,
            parts,
        }),
        bind: None,
    }
}

/// The master strip of bus `index`: header, device, the switch of its equalizer, mute, fader with
/// meter and numbers, readout.
#[must_use]
pub fn bus_strip(index: usize) -> Element {
    let at = |param| Some(Binding::Bus(index, param));
    let parts = vec![
        Element::part(Zone::Header, Control::Header, None),
        Element::part(Zone::Info, Control::Device, None),
        Element::toggle(
            Zone::Dynamics,
            "EQ",
            Role::Assign,
            Binding::Bus(index, BusParam::Eq),
        ),
        Element::toggle(
            Zone::MuteSolo,
            "MUTE",
            Role::Mute,
            Binding::Bus(index, BusParam::Mute),
        ),
        Element::part(Zone::Fader, Control::Fader, at(BusParam::Gain)),
        Element::part(Zone::Fader, Control::Meter, at(BusParam::Meter)),
        Element::part(Zone::Fader, Control::MeterScale, None),
        Element::part(Zone::Readout, Control::Readout, at(BusParam::Gain)),
    ];
    Element {
        place: Place::BusSection,
        control: Control::Strip(Strip {
            owner: Owner::Bus(index),
            style: StripStyle::Bus,
            parts,
        }),
        bind: None,
    }
}

/// What the meters keep between frames (peak hold, clip lamp), by the meter they show.
#[derive(Clone, Debug, Default)]
pub struct Meters {
    inputs: Vec<MeterState>,
    buses: Vec<MeterState>,
}

impl Meters {
    /// Keeps one state per input and bus of `view`.
    pub fn fit(&mut self, view: &MixerView) {
        self.inputs
            .resize_with(view.inputs.len(), MeterState::default);
        self.buses
            .resize_with(view.buses.len(), MeterState::default);
    }

    /// The state of the meter `binding` names; `None` for anything else.
    pub fn get_mut(&mut self, binding: Binding) -> Option<&mut MeterState> {
        match binding {
            Binding::Input(index, InputParam::Meter) => self.inputs.get_mut(index),
            Binding::Bus(index, BusParam::Meter) => self.buses.get_mut(index),
            _ => None,
        }
    }

    /// The meter states of the buses, in route order.
    pub fn buses_mut(&mut self) -> &mut [MeterState] {
        &mut self.buses
    }

    #[must_use]
    pub fn bus(&self, index: usize) -> Option<&MeterState> {
        self.buses.get(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pult_core::view::{Desk, FakeDesk};

    fn strip(element: &Element) -> &Strip {
        match &element.control {
            Control::Strip(strip) => strip,
            other => panic!("not a strip: {other:?}"),
        }
    }

    #[test]
    fn the_standard_console_is_a_strip_per_input_then_per_bus() {
        let view = FakeDesk::new().view();
        let elements = standard(&view);
        assert_eq!(elements.len(), view.inputs.len() + view.buses.len());
        let (inputs, buses) = elements.split_at(view.inputs.len());
        for (index, element) in inputs.iter().enumerate() {
            assert_eq!(element.place, Place::InputRow);
            assert_eq!(strip(element).owner, Owner::Input(index));
        }
        for (index, element) in buses.iter().enumerate() {
            assert_eq!(element.place, Place::BusSection);
            assert_eq!(strip(element).owner, Owner::Bus(index));
        }
    }

    #[test]
    fn an_input_strip_binds_every_parameter_of_its_input() {
        let view = FakeDesk::new().view();
        let element = input_strip(1, &view);
        let addresses: Vec<String> = strip(&element)
            .parts
            .iter()
            .filter_map(|part| part.bind?.address(&view))
            .collect();
        let mut expected = vec!["input/2/pan".to_owned(), "input/2/pan".to_owned()];
        expected.extend(view.buses.iter().map(|b| format!("matrix/2/{}", b.name)));
        expected.extend(
            ["gate", "comp", "mute", "solo", "gain", "meter", "gain"]
                .map(|param| format!("input/2/{param}")),
        );
        assert_eq!(addresses, expected);
        assert!(
            strip(&element)
                .parts
                .iter()
                .all(|part| matches!(part.place, Place::Zone(_)))
        );
    }

    #[test]
    fn a_bus_strip_binds_its_bus() {
        let view = FakeDesk::new().view();
        let element = bus_strip(0);
        let addresses: Vec<String> = strip(&element)
            .parts
            .iter()
            .filter_map(|part| part.bind?.address(&view))
            .collect();
        assert_eq!(
            addresses,
            [
                "bus/A1/eq",
                "bus/A1/mute",
                "bus/A1/gain",
                "bus/A1/meter",
                "bus/A1/gain"
            ]
        );
    }

    #[test]
    fn meters_are_kept_by_their_binding() {
        let view = FakeDesk::new().view();
        let mut meters = Meters::default();
        meters.fit(&view);
        let input = Binding::Input(0, InputParam::Meter);
        meters.get_mut(input).unwrap().mark_clipped();
        assert!(meters.get_mut(input).unwrap().clipped());
        assert!(
            !meters
                .get_mut(Binding::Bus(0, BusParam::Meter))
                .unwrap()
                .clipped()
        );
        assert!(
            meters
                .get_mut(Binding::Input(0, InputParam::Gain))
                .is_none()
        );
        assert!(
            meters
                .get_mut(Binding::Input(99, InputParam::Meter))
                .is_none()
        );
    }
}
