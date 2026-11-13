//! What a MIDI controller means to the mixer, with no device in sight: messages as data, which
//! control is bound to which parameter, what a movement asks for and what should be lit.
//!
//! A movement becomes an [`Op`], the same request every other client sends, so there is no second
//! way into the mixer. A target is a parameter address (`input/2/gain`) or a scene
//! (`scene/abend`).
//!
//! Faders of a controller have no motor: after a scene the hardware stands somewhere else than the
//! gain, and the first touch would be a jump in level. So a fader picks up: it acts only once it
//! has reached the value it would change.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::binding::{BandParam, Binding, BusParam, InputParam, Value};
use crate::eq::{EQ_FREQ_HZ, EQ_GAIN_DB, EQ_Q};
use crate::limiter::{LIMIT_CEILING_DB, LIMIT_LOOKAHEAD_MS};
use crate::protocol::{Op, OpValue};
use crate::scale::{Span, fader_db, fader_pos};
use crate::view::{AMOUNT_RANGE, GAIN_RANGE_DB, MixerView};

/// How long a change of a fader glides: its seven bits are steps one would hear otherwise.
const FADER_RAMP_MS: u32 = 20;

/// How near a fader must come to its value to pick it up, as a share of its travel: a move of the
/// hand skips values.
const PICKUP_NEAR: f32 = 1.5 / 127.0;

/// From this value on a controller's switch counts as pressed.
const PRESSED_FROM: u8 = 64;

/// How long a learning waits for a movement.
pub const LEARN_FOR: Duration = Duration::from_secs(30);

/// The prefix of a scene as a target.
const SCENE_PREFIX: &str = "scene/";

/// What a controller sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message {
    /// A button; velocity 0 is its release.
    Note {
        /// 0 to 15.
        channel: u8,
        /// Which button.
        note: u8,
        /// 0 is released.
        velocity: u8,
    },
    /// A fader or knob.
    Cc {
        /// 0 to 15.
        channel: u8,
        /// Which one.
        controller: u8,
        /// 0 to 127.
        value: u8,
    },
}

impl Message {
    /// The message in `bytes`; `None` for all a mixer has no use for (clock, sysex, ...).
    #[must_use]
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let [status, first, second, ..] = *bytes else {
            return None;
        };
        if first > 127 || second > 127 {
            return None;
        }
        let channel = status & 0x0F;
        match status & 0xF0 {
            0x90 => Some(Self::Note {
                channel,
                note: first,
                velocity: second,
            }),
            0x80 => Some(Self::Note {
                channel,
                note: first,
                velocity: 0,
            }),
            0xB0 => Some(Self::Cc {
                channel,
                controller: first,
                value: second,
            }),
            _ => None,
        }
    }

    /// The control that sent it.
    #[must_use]
    pub fn control(self) -> Control {
        match self {
            Self::Note { channel, note, .. } => Control {
                kind: ControlKind::Note,
                channel,
                number: note,
            },
            Self::Cc {
                channel,
                controller,
                ..
            } => Control {
                kind: ControlKind::Cc,
                channel,
                number: controller,
            },
        }
    }

    /// Whether this is a press, not a release: what learning and switches go by.
    fn is_press(self) -> bool {
        match self {
            Self::Note { velocity, .. } => velocity > 0,
            Self::Cc { value, .. } => value >= PRESSED_FROM,
        }
    }
}

/// A button or a fader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ControlKind {
    /// Sends notes.
    Note,
    /// Sends control changes.
    Cc,
}

/// One control of a controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Control {
    /// Button or fader.
    pub kind: ControlKind,
    /// 0 to 15.
    pub channel: u8,
    /// Its note or controller number.
    pub number: u8,
}

impl fmt::Display for Control {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            ControlKind::Note => "note",
            ControlKind::Cc => "CC",
        };
        write!(f, "{kind} {}", self.number)?;
        if self.channel > 0 {
            write!(f, " on channel {}", self.channel + 1)?;
        }
        Ok(())
    }
}

/// A control and what it moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    /// The control.
    pub control: Control,
    /// An address or `scene/<name>`.
    pub target: String,
    /// An endless knob: it sends steps (1 up, 127 down), not where it stands.
    pub relative: bool,
}

/// Whether the first value of a control looks like the step of an endless knob: small steps around
/// zero. A fader or a knob with ends sends these only right at its ends. Measured on the APC Key 25
/// mk2: a fast turn sends up to 6 and 123, also as the first value a learning sees.
#[must_use]
pub fn looks_relative(value: u8) -> bool {
    matches!(value, 1..=7 | 121..=127)
}

/// How many steps of an endless knob go from the bottom to the top of a value on the device `name`.
/// Measured on the APC Key 25 mk2: at one step per 1/127, half a slow turn went from +6 dB to silence.
/// Others stay at the 127 of MIDI until someone measures them: a knob with few steps a turn would
/// need many turns otherwise.
#[must_use]
pub fn knob_steps(name: &str) -> f32 {
    let name = name.to_ascii_lowercase();
    if name.contains("apc key") && name.contains("mk2") {
        400.0
    } else {
        127.0
    }
}

/// The step in a relative value: 1 to 63 up, 65 to 127 down.
fn relative_step(value: u8) -> f32 {
    if value < 64 {
        f32::from(value)
    } else {
        f32::from(value) - 128.0
    }
}

/// What a client reads about MIDI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MidiStatus {
    /// The device in use, if one was chosen yet.
    pub device: Option<String>,
    /// Whether it is there and open.
    pub connected: bool,
    /// The target waiting for a movement.
    pub learning: Option<String>,
    /// All that is bound.
    pub bindings: Vec<MidiBinding>,
    /// Why MIDI is off, if something keeps it off. Holds no path: a client reads no folders.
    #[serde(default)]
    pub problem: Option<String>,
}

/// A binding as a client reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiBinding {
    /// `CC 48`, `note 12`.
    pub control: String,
    /// An address or `scene/<name>`.
    pub target: String,
}

/// The kinds of thing a target can be.
#[derive(Debug, Clone, PartialEq)]
enum Param {
    /// A gain, on the fader's law.
    Gain(Binding),
    /// A pan.
    Pan(Binding),
    /// A number between two ends: the one knob of a gate or a compressor, a number of a band.
    Knob(Binding, Span),
    /// A mute, a solo or a crosspoint.
    Switch(Binding),
    /// A scene.
    Scene(String),
}

impl Param {
    fn of(target: &str, view: &MixerView) -> Result<Self, String> {
        if let Some(name) = target.strip_prefix(SCENE_PREFIX) {
            return view
                .scenes
                .iter()
                .find(|scene| scene.eq_ignore_ascii_case(name))
                .map(|scene| Self::Scene(scene.clone()))
                .ok_or_else(|| format!("no scene \"{name}\""));
        }
        let binding = Binding::parse(target, view).map_err(|e| e.to_string())?;
        Ok(match binding {
            Binding::Input(_, InputParam::Gain)
            | Binding::Bus(_, BusParam::Gain)
            | Binding::Send { .. } => Self::Gain(binding),
            Binding::Input(_, InputParam::Pan) => Self::Pan(binding),
            Binding::Input(_, InputParam::Gate | InputParam::Comp) => {
                Self::Knob(binding, Span::even(AMOUNT_RANGE))
            }
            Binding::Bus(_, BusParam::EqBand(_, BandParam::Gain)) => {
                Self::Knob(binding, Span::even(EQ_GAIN_DB))
            }
            Binding::Bus(_, BusParam::EqBand(_, BandParam::Freq)) => {
                Self::Knob(binding, Span::octaves(EQ_FREQ_HZ))
            }
            Binding::Bus(_, BusParam::EqBand(_, BandParam::Q)) => {
                Self::Knob(binding, Span::octaves(EQ_Q))
            }
            Binding::Bus(_, BusParam::Comp) => Self::Knob(binding, Span::even(AMOUNT_RANGE)),
            Binding::Bus(_, BusParam::CompMix) => Self::Knob(binding, Span::even((0.0, 100.0))),
            Binding::Bus(_, BusParam::Multiband(param)) => Self::Knob(
                binding,
                match param {
                    crate::multiband::MultibandParam::Band(_) => Span::even(AMOUNT_RANGE),
                    crate::multiband::MultibandParam::LowSplit => {
                        Span::octaves(crate::multiband::LOW_SPLIT_HZ)
                    }
                    crate::multiband::MultibandParam::HighSplit => {
                        Span::octaves(crate::multiband::HIGH_SPLIT_HZ)
                    }
                },
            ),
            Binding::Bus(_, BusParam::CompHpf) => {
                Self::Knob(binding, Span::octaves(crate::buscomp::HPF_HZ))
            }
            Binding::Bus(_, BusParam::Delay) => Self::Knob(
                binding,
                Span::even((0.0, crate::busdelay::MAX_BUS_DELAY_MS)),
            ),
            Binding::Input(_, InputParam::AutomixWeight) => {
                Self::Knob(binding, Span::even((-12.0, 12.0)))
            }
            Binding::Input(_, InputParam::Hpf) => {
                Self::Knob(binding, Span::octaves(crate::strip::HPF_RANGE_HZ))
            }
            Binding::Input(_, InputParam::Lpf) => {
                Self::Knob(binding, Span::octaves(crate::strip::LPF_RANGE_HZ))
            }
            Binding::Input(_, InputParam::Delay) => {
                Self::Knob(binding, Span::even((0.0, crate::strip::MAX_DELAY_MS)))
            }
            Binding::Input(_, InputParam::Width) => {
                Self::Knob(binding, Span::even(crate::strip::WIDTH_RANGE))
            }
            Binding::Input(_, InputParam::Duck(duck)) => match duck.range() {
                Some(range) => Self::Knob(binding, Span::even(range)),
                None => {
                    return Err(format!("{target} names an input, which no control can say"));
                }
            },
            Binding::Bus(_, BusParam::LimitCeiling) => {
                Self::Knob(binding, Span::even(LIMIT_CEILING_DB))
            }
            Binding::Bus(_, BusParam::LimitLookahead) => {
                Self::Knob(binding, Span::even(LIMIT_LOOKAHEAD_MS))
            }
            Binding::Bus(_, BusParam::EqBand(_, BandParam::Kind)) => {
                return Err(format!("{target} is a word, which no control can say"));
            }
            Binding::Input(
                _,
                InputParam::Mute | InputParam::Solo | InputParam::Automix | InputParam::Polarity,
            )
            | Binding::Bus(
                _,
                BusParam::Mute | BusParam::Eq | BusParam::Limit | BusParam::Monitor,
            )
            | Binding::Route { .. }
            | Binding::SendPre { .. } => Self::Switch(binding),
            Binding::Input(_, InputParam::Meter) | Binding::Bus(_, BusParam::Meter) => {
                return Err(format!("{target} can only be read"));
            }
        })
    }

    /// Whether a control of this kind can work it: a button cannot say how loud.
    fn takes(&self, kind: ControlKind) -> bool {
        kind == ControlKind::Cc || matches!(self, Self::Switch(_) | Self::Scene(_))
    }
}

/// Whether `target` is something a control can be bound to in `view`.
///
/// # Errors
///
/// The reason, for an address or scene `view` does not have, or a meter.
pub fn check_target(target: &str, view: &MixerView) -> Result<(), String> {
    Param::of(target, view).map(|_| ())
}

/// What is known of a fader between its messages.
#[derive(Debug, Clone, Copy, Default)]
struct Pick {
    /// It moves the value.
    picked: bool,
    /// Where the hardware was last, 0 to 1.
    last: Option<f32>,
    /// The position it last set, and at which view.
    sent: Option<(f32, Duration)>,
    /// It said already that it waits.
    told: bool,
}

/// What a message came to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Step {
    /// What to ask of the mixer.
    pub op: Option<Op>,
    /// What to tell the person.
    pub note: Option<String>,
    /// A control was bound: the bindings need saving.
    pub learned: bool,
}

/// The colours of a controller's buttons: the velocities that light them, and the channel to send
/// them on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// For a mute that is on.
    pub red: u8,
    /// For a route that is on.
    pub green: u8,
    /// For a solo that is on, and scenes.
    pub yellow: u8,
    /// `None`: the channel of the button.
    pub channel: Option<u8>,
}

impl Palette {
    /// The colours for a device by its name. The APC mini has fixed velocities; its mk2 has a wide
    /// palette and takes the brightness from the channel (6 is full). Others get the one value
    /// nearly all understand.
    #[must_use]
    pub fn for_device(name: &str) -> Self {
        let name = name.to_ascii_lowercase();
        if name.contains("apc") && name.contains("mk2") {
            Self {
                red: 5,
                green: 21,
                // Measured on the APC Key 25 mk2, against all 127 values: 13 looks green there.
                yellow: 96,
                channel: Some(6),
            }
        } else if name.contains("apc") {
            Self {
                red: 3,
                green: 1,
                yellow: 5,
                channel: None,
            }
        } else {
            Self {
                red: 127,
                green: 127,
                yellow: 127,
                channel: None,
            }
        }
    }
}

/// A learning under way.
#[derive(Debug, Clone)]
struct Learning {
    target: String,
    until: Option<Duration>,
}

/// The bindings and what is needed to follow a controller.
#[derive(Debug, Default)]
pub struct Map {
    bindings: Vec<Bound>,
    learning: Option<Learning>,
    picks: HashMap<Control, Pick>,
    /// Which CC switches are held, so a held one toggles once.
    held: HashMap<Control, bool>,
    /// What a switch was set to since the view at `pending_at`, which does not show that yet.
    pending: HashMap<String, bool>,
    pending_at: Duration,
    /// What each button was last lit with.
    lit: HashMap<Control, u8>,
    /// How many steps of an endless knob make the whole way, once the device is known.
    knob_steps: Option<f32>,
}

impl Map {
    /// A map of these bindings.
    #[must_use]
    pub fn new(bindings: Vec<Bound>) -> Self {
        Self {
            bindings,
            ..Self::default()
        }
    }

    /// Tells which device the bindings are on: its knobs may step finer than others.
    pub fn set_device(&mut self, name: &str) {
        self.knob_steps = Some(knob_steps(name));
    }

    /// What is bound.
    #[must_use]
    pub fn bindings(&self) -> &[Bound] {
        &self.bindings
    }

    /// The target waiting for a movement.
    #[must_use]
    pub fn learning(&self) -> Option<&str> {
        self.learning.as_ref().map(|l| l.target.as_str())
    }

    /// Lets the next movement bind `target`.
    pub fn learn(&mut self, target: String) {
        self.learning = Some(Learning {
            target,
            until: None,
        });
    }

    /// Takes the binding of `target` away; whether there was one. Its light goes out with the next
    /// [`Self::lights`].
    pub fn forget(&mut self, target: &str) -> bool {
        let before = self.bindings.len();
        self.bindings.retain(|bound| bound.target != target);
        self.bindings.len() != before
    }

    /// Ends a learning that waited too long; what to tell the person.
    pub fn expire(&mut self, view: &MixerView) -> Option<String> {
        let learning = self.learning.as_mut()?;
        let until = *learning.until.get_or_insert(view.elapsed + LEARN_FOR);
        if view.elapsed < until {
            return None;
        }
        let target = self.learning.take()?.target;
        Some(format!("nothing moved for {target}"))
    }

    /// Lights everything anew: the device came back, and knows nothing of what was lit.
    pub fn forget_lights(&mut self) {
        self.lit.clear();
    }

    /// The messages that put out what is lit, all of them: nobody is left to send the rest later.
    pub fn lights_out(&mut self, palette: Palette) -> Vec<[u8; 3]> {
        let mut lit: Vec<Control> = self
            .lit
            .drain()
            .filter(|(_, velocity)| *velocity != 0)
            .map(|(control, _)| control)
            .collect();
        lit.sort_by_key(|control| (control.channel, control.number));
        lit.into_iter()
            .map(|control| {
                let channel = palette.channel.unwrap_or(control.channel) & 0x0F;
                [0x90 | channel, control.number, 0]
            })
            .collect()
    }

    /// Whether `control` is bound as an endless knob, whose every message is a step of its own.
    #[must_use]
    pub fn is_relative(&self, control: Control) -> bool {
        self.bindings
            .iter()
            .any(|bound| bound.control == control && bound.relative)
    }

    /// Follows one message in `view`.
    pub fn step(&mut self, message: Message, view: &MixerView) -> Step {
        if view.elapsed != self.pending_at {
            self.pending.clear();
            self.pending_at = view.elapsed;
        }
        if self.learning.is_some() {
            return self.learn_from(message, view);
        }
        let control = message.control();
        let Some(bound) = self.bindings.iter().find(|b| b.control == control) else {
            return Step::default();
        };
        let target = bound.target.clone();
        let relative = bound.relative;
        match Param::of(&target, view) {
            // A binding to what this route does not have does nothing: another profile has it.
            Err(_) => Step::default(),
            Ok(param) => self.work(&param, &target, relative, message, view),
        }
    }

    fn learn_from(&mut self, message: Message, view: &MixerView) -> Step {
        if !message.is_press() && matches!(message, Message::Note { .. }) {
            return Step::default();
        }
        let Some(learning) = self.learning.clone() else {
            return Step::default();
        };
        let control = message.control();
        match Param::of(&learning.target, view) {
            Ok(param) if param.takes(control.kind) => {}
            Ok(_) => {
                return Step {
                    note: Some(format!(
                        "{control} is a button and cannot set {}; move a fader or knob",
                        learning.target
                    )),
                    ..Step::default()
                };
            }
            Err(why) => {
                self.learning = None;
                return Step {
                    note: Some(why),
                    ..Step::default()
                };
            }
        }
        self.learning = None;
        // One control moves one thing, and a thing has one control.
        self.bindings
            .retain(|b| b.control != control && b.target != learning.target);
        self.picks.remove(&control);
        self.held.remove(&control);
        let note = format!("{control} -> {}", learning.target);
        let relative = matches!(message, Message::Cc { value, .. } if looks_relative(value));
        self.bindings.push(Bound {
            control,
            target: learning.target,
            relative,
        });
        Step {
            note: Some(note),
            learned: true,
            ..Step::default()
        }
    }

    fn work(
        &mut self,
        param: &Param,
        target: &str,
        relative: bool,
        message: Message,
        view: &MixerView,
    ) -> Step {
        let control = message.control();
        match (param, message) {
            (Param::Gain(binding), Message::Cc { value, .. }) => {
                let current = match binding.read(view) {
                    Some(Value::Gain(db)) => fader_pos(db),
                    _ => return Step::default(),
                };
                let to_value = |pos: f32| OpValue::Number(fader_db(pos).max(GAIN_RANGE_DB.0));
                if relative {
                    self.turn(control, value, current, target, view, to_value)
                } else {
                    let pos = f32::from(value) / 127.0;
                    self.pick_up(control, pos, current, target, view, to_value)
                }
            }
            (Param::Pan(binding), Message::Cc { value, .. }) => {
                let current = match binding.read(view) {
                    Some(Value::Pan(pan)) => f32::midpoint(pan, 1.0),
                    _ => return Step::default(),
                };
                let to_value = |pos: f32| {
                    // The two values around the middle are the middle: 127 has none of its own.
                    let pan = pos * 2.0 - 1.0;
                    OpValue::Number(if pan.abs() < 1.0 / 127.0 { 0.0 } else { pan })
                };
                if relative {
                    self.turn(control, value, current, target, view, to_value)
                } else {
                    let pos = f32::from(value) / 127.0;
                    self.pick_up(control, pos, current, target, view, to_value)
                }
            }
            (Param::Knob(binding, span), Message::Cc { value, .. }) => {
                let current = match binding.read(view) {
                    Some(
                        Value::Amount(number)
                        | Value::Gain(number)
                        | Value::Percent(number)
                        | Value::Boost(number)
                        | Value::Freq(number)
                        | Value::Width(number)
                        | Value::Ceiling(number)
                        | Value::Lookahead(number)
                        | Value::Delay(number),
                    ) => span.pos(number),
                    Some(Value::Duck(change)) => span.pos(change.number()),
                    _ => return Step::default(),
                };
                let to_value = |pos: f32| OpValue::Number(span.value(pos));
                let mut step = if relative {
                    self.turn(control, value, current, target, view, to_value)
                } else {
                    let pos = f32::from(value) / 127.0;
                    self.pick_up(control, pos, current, target, view, to_value)
                };
                // Only a gain or a pan ramps; these glide in the mixer by themselves.
                if let Some(Op::Set { ramp_ms, .. }) = &mut step.op {
                    *ramp_ms = None;
                }
                step
            }
            (Param::Switch(binding), _) => {
                if !self.pressed(control, message) {
                    return Step::default();
                }
                let on = self
                    .pending
                    .get(target)
                    .copied()
                    .unwrap_or_else(|| matches!(binding.read(view), Some(Value::Switch(true))));
                self.pending.insert(target.to_owned(), !on);
                Step {
                    op: Some(Op::Set {
                        address: target.to_owned(),
                        value: OpValue::Switch(!on),
                        ramp_ms: None,
                    }),
                    ..Step::default()
                }
            }
            (Param::Scene(name), _) => {
                if !self.pressed(control, message) {
                    return Step::default();
                }
                Step {
                    op: Some(Op::Scene { name: name.clone() }),
                    ..Step::default()
                }
            }
            // A button bound to a level by hand in the file.
            (Param::Gain(_) | Param::Pan(_) | Param::Knob(..), Message::Note { .. }) => {
                Step::default()
            }
        }
    }

    /// A press that was not one before: a held CC control sends its value again and again.
    fn pressed(&mut self, control: Control, message: Message) -> bool {
        let now = message.is_press();
        if control.kind == ControlKind::Note {
            return now;
        }
        let before = self.held.insert(control, now).unwrap_or(false);
        now && !before
    }

    /// An endless knob moves on from where the value stands, so it has nothing to find. Between two
    /// views its own last step counts: the view does not show that yet.
    fn turn(
        &mut self,
        control: Control,
        value: u8,
        current: f32,
        target: &str,
        view: &MixerView,
        to_value: impl Fn(f32) -> OpValue,
    ) -> Step {
        let pick = self.picks.entry(control).or_default();
        let from = match pick.sent {
            Some((sent, at)) if at == view.elapsed => sent,
            _ => current,
        };
        let steps = self.knob_steps.unwrap_or_else(|| knob_steps(""));
        let pos = (from + relative_step(value) / steps).clamp(0.0, 1.0);
        pick.sent = Some((pos, view.elapsed));
        Step {
            op: Some(Op::Set {
                address: target.to_owned(),
                value: to_value(pos),
                ramp_ms: Some(FADER_RAMP_MS),
            }),
            ..Step::default()
        }
    }

    fn pick_up(
        &mut self,
        control: Control,
        pos: f32,
        current: f32,
        target: &str,
        view: &MixerView,
        value: impl Fn(f32) -> OpValue,
    ) -> Step {
        let pick = self.picks.entry(control).or_default();
        // Something else moved the value (a scene, a client, the window) once a view shows it:
        // the hardware has to find it again.
        if let Some((sent, at)) = pick.sent
            && at != view.elapsed
            && (sent - current).abs() > PICKUP_NEAR
        {
            pick.picked = false;
            pick.told = false;
        }
        if !pick.picked {
            let near = (pos - current).abs() <= PICKUP_NEAR;
            let crossed = pick
                .last
                .is_some_and(|last| (last - current) * (pos - current) < 0.0);
            if !near && !crossed {
                pick.last = Some(pos);
                let told = pick.told;
                pick.told = true;
                return Step {
                    note: (!told)
                        .then(|| format!("{control} waits until it reaches the value of {target}")),
                    ..Step::default()
                };
            }
            pick.picked = true;
        }
        pick.last = Some(pos);
        pick.sent = Some((pos, view.elapsed));
        Step {
            op: Some(Op::Set {
                address: target.to_owned(),
                value: value(pos),
                ramp_ms: Some(FADER_RAMP_MS),
            }),
            ..Step::default()
        }
    }

    /// The messages that bring the buttons up to `view`, at most `most` of them: only what changed,
    /// and the rest with the next call, so a scene does not flood a slow device.
    pub fn lights(&mut self, view: &MixerView, palette: Palette, most: usize) -> Vec<[u8; 3]> {
        let mut wanted: HashMap<Control, u8> = HashMap::new();
        for bound in &self.bindings {
            if bound.control.kind != ControlKind::Note {
                continue;
            }
            let velocity = match Param::of(&bound.target, view) {
                Ok(Param::Switch(binding)) => {
                    let on = matches!(binding.read(view), Some(Value::Switch(true)));
                    let colour = match binding {
                        Binding::Route { .. } => palette.green,
                        Binding::Input(_, InputParam::Solo) => palette.yellow,
                        _ => palette.red,
                    };
                    if on { colour } else { 0 }
                }
                Ok(Param::Scene(_)) => palette.yellow,
                _ => 0,
            };
            wanted.insert(bound.control, velocity);
        }
        // A button that lost its binding goes dark.
        for control in self.lit.keys() {
            wanted.entry(*control).or_insert(0);
        }
        let mut changes: Vec<(Control, u8)> = wanted
            .into_iter()
            .filter(|(control, velocity)| {
                self.lit.get(control).copied().unwrap_or(u8::MAX) != *velocity
            })
            .collect();
        // In a fixed order, so that a limit does not always leave out the same by chance.
        changes.sort_by_key(|(control, _)| (control.channel, control.number));
        changes.truncate(most);
        changes
            .into_iter()
            .map(|(control, velocity)| {
                self.lit.insert(control, velocity);
                let channel = palette.channel.unwrap_or(control.channel) & 0x0F;
                [0x90 | channel, control.number, velocity]
            })
            .collect()
    }

    /// What a client reads, for the device `device`.
    #[must_use]
    pub fn status(&self, device: Option<String>, connected: bool) -> MidiStatus {
        MidiStatus {
            device,
            connected,
            learning: self.learning().map(str::to_owned),
            bindings: self
                .bindings
                .iter()
                .map(|bound| MidiBinding {
                    control: bound.control.to_string(),
                    target: bound.target.clone(),
                })
                .collect(),
            problem: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{Desk, FakeDesk};

    /// The value of a fader at 0 dB.
    const AT_ZERO: u8 = 97;

    fn view() -> MixerView {
        FakeDesk::new().view()
    }

    #[test]
    fn zero_db_is_where_these_tests_say() {
        assert!((f32::from(AT_ZERO) / 127.0 - fader_pos(0.0)).abs() < PICKUP_NEAR);
    }

    /// A later view than `view`.
    fn later(view: &MixerView) -> MixerView {
        let mut next = view.clone();
        next.elapsed += Duration::from_millis(100);
        next
    }

    fn cc(controller: u8, value: u8) -> Message {
        Message::Cc {
            channel: 0,
            controller,
            value,
        }
    }

    fn press(note: u8) -> Message {
        Message::Note {
            channel: 0,
            note,
            velocity: 127,
        }
    }

    fn release(note: u8) -> Message {
        Message::Note {
            channel: 0,
            note,
            velocity: 0,
        }
    }

    fn fader(number: u8) -> Control {
        cc(number, 0).control()
    }

    fn button(number: u8) -> Control {
        press(number).control()
    }

    fn map(bound: &[(Control, &str)]) -> Map {
        Map::new(
            bound
                .iter()
                .map(|(control, target)| Bound {
                    control: *control,
                    target: (*target).to_owned(),
                    relative: false,
                })
                .collect(),
        )
    }

    fn number(step: &Step) -> f32 {
        match step.op {
            Some(Op::Set {
                value: OpValue::Number(number),
                ..
            }) => number,
            _ => panic!("no number: {step:?}"),
        }
    }

    fn switch(step: &Step) -> bool {
        match step.op {
            Some(Op::Set {
                value: OpValue::Switch(on),
                ..
            }) => on,
            _ => panic!("no switch: {step:?}"),
        }
    }

    #[test]
    fn a_knob_spans_its_range_evenly_or_by_octaves() {
        let even = Span::even(EQ_GAIN_DB);
        assert_eq!(even.value(0.0), -24.0);
        assert_eq!(even.value(0.5), 0.0);
        assert_eq!(even.value(1.0), 24.0);
        assert_eq!(even.pos(12.0), 0.75);
        let octaves = Span::octaves(EQ_FREQ_HZ);
        assert_eq!(octaves.value(0.0), 20.0);
        assert_eq!(octaves.value(1.0), 20_000.0);
        // The middle of 20 Hz to 20 kHz by ratio: about 632 Hz.
        assert!((octaves.value(0.5) - 632.5).abs() < 1.0);
        assert!((octaves.pos(632.5) - 0.5).abs() < 1e-3);
        assert_eq!(octaves.value(7.0), 20_000.0);
        assert_eq!(octaves.pos(1.0), 0.0);
    }

    #[test]
    fn the_numbers_of_a_band_take_a_knob_and_its_kind_does_not() {
        let view = FakeDesk::new().view();
        for target in ["bus/A1/eq/2/gain", "bus/A1/eq/2/freq", "bus/A1/eq/2/q"] {
            assert!(
                matches!(Param::of(target, &view), Ok(Param::Knob(..))),
                "{target}"
            );
        }
        assert!(matches!(
            Param::of("bus/A1/limit", &view),
            Ok(Param::Switch(Binding::Bus(0, BusParam::Limit)))
        ));
        assert!(matches!(
            Param::of("bus/A1/limit/ceiling", &view),
            Ok(Param::Knob(..))
        ));
        assert!(matches!(
            Param::of("bus/A1/limit/lookahead", &view),
            Ok(Param::Knob(..))
        ));
        assert!(matches!(
            Param::of("bus/A1/eq", &view),
            Ok(Param::Switch(_))
        ));
        let why = Param::of("bus/A1/eq/2/kind", &view).expect_err("a word");
        assert!(why.contains("no control can say"), "{why}");
    }

    #[test]
    fn messages_read_as_data() {
        assert_eq!(
            Message::parse(&[0x92, 12, 100]),
            Some(Message::Note {
                channel: 2,
                note: 12,
                velocity: 100
            })
        );
        assert_eq!(
            Message::parse(&[0x81, 12, 64]),
            Some(Message::Note {
                channel: 1,
                note: 12,
                velocity: 0
            })
        );
        assert_eq!(Message::parse(&[0xB0, 48, 7]), Some(cc(48, 7)));
        for nothing in [
            &[][..],
            &[0xF8],
            &[0x90, 12],
            &[0xC0, 1, 2],
            &[0xF0, 1, 2],
            &[0x90, 200, 1],
            &[0xB0, 1, 200],
        ] {
            assert_eq!(Message::parse(nothing), None, "{nothing:?}");
        }
    }

    #[test]
    fn targets_are_addresses_and_scenes_and_not_meters() {
        let view = view();
        for good in ["input/1/gain", "bus/A1/mute", "matrix/2/A1", "scene/abend"] {
            assert_eq!(check_target(good, &view), Ok(()), "{good}");
        }
        assert!(
            check_target("input/9/gain", &view)
                .unwrap_err()
                .contains("no input")
        );
        assert!(
            check_target("input/1/meter", &view)
                .unwrap_err()
                .contains("only be read")
        );
        assert!(
            check_target("scene/nichts", &view)
                .unwrap_err()
                .contains("no scene")
        );
    }

    #[test]
    fn learning_binds_the_next_movement_and_keeps_one_to_one() {
        let view = view();
        let mut map = map(&[(fader(48), "input/2/gain")]);
        map.learn("input/1/gain".to_owned());
        assert_eq!(map.learning(), Some("input/1/gain"));
        // A release moves nothing: it is what is left of the press before.
        assert_eq!(map.step(release(1), &view), Step::default());
        assert!(map.learning().is_some());
        let step = map.step(cc(48, 5), &view);
        assert!(step.learned);
        assert_eq!(step.op, None, "the movement that binds moves nothing");
        assert_eq!(step.note.as_deref(), Some("CC 48 -> input/1/gain"));
        assert_eq!(map.learning(), None);
        assert_eq!(map.bindings().len(), 1, "the control left its old target");

        // The other way round: the target moves to another control.
        map.learn("input/1/gain".to_owned());
        map.step(cc(49, 5), &view);
        assert_eq!(map.bindings().len(), 1);
        assert_eq!(map.bindings()[0].control, fader(49));
    }

    #[test]
    fn a_button_cannot_learn_a_level_and_learning_goes_on() {
        let view = view();
        let mut map = Map::default();
        map.learn("input/1/gain".to_owned());
        let step = map.step(press(3), &view);
        assert!(!step.learned);
        assert!(step.note.expect("why").contains("cannot set"));
        assert!(map.learning().is_some());
        assert!(map.step(cc(48, 1), &view).learned);

        // A fader can work a switch.
        map.learn("input/1/mute".to_owned());
        assert!(map.step(cc(50, 100), &view).learned);
    }

    #[test]
    fn learning_gives_up_after_a_while() {
        let mut view = view();
        let mut map = Map::default();
        map.learn("input/1/mute".to_owned());
        assert_eq!(map.expire(&view), None, "the clock starts here");
        view.elapsed += Duration::from_secs(29);
        assert_eq!(map.expire(&view), None);
        view.elapsed += Duration::from_secs(1);
        assert!(map.expire(&view).expect("told").contains("nothing moved"));
        assert_eq!(map.learning(), None);
        assert_eq!(map.expire(&view), None);
    }

    #[test]
    fn a_fader_acts_only_once_it_reached_the_value() {
        let view = view();
        // The first input stands at 0 dB, three quarters up.
        let mut map = map(&[(fader(48), "input/1/gain")]);
        let at_zero = AT_ZERO;

        let first = map.step(cc(48, 10), &view);
        assert_eq!(first.op, None, "far below: no jump");
        assert!(first.note.expect("told").contains("waits"));
        let second = map.step(cc(48, 20), &view);
        assert_eq!(second, Step::default(), "told once");

        // Crossing the value, the hand skipped it.
        let crossed = map.step(cc(48, at_zero + 10), &view);
        assert!(number(&crossed) > 0.0, "{crossed:?}");
        // From now on it follows, down as well.
        let followed = map.step(cc(48, 10), &view);
        assert!(number(&followed) < -30.0, "{followed:?}");
        let Some(Op::Set { ramp_ms, .. }) = followed.op else {
            panic!("no set");
        };
        assert_eq!(ramp_ms, Some(FADER_RAMP_MS));
    }

    #[test]
    fn a_fader_near_its_value_picks_up_at_once_and_its_ends_are_off_and_top() {
        let mut view = view();
        view.inputs[0].gain_db = GAIN_RANGE_DB.0;
        let mut map = map(&[(fader(48), "input/1/gain")]);
        let bottom = map.step(cc(48, 0), &view);
        assert_eq!(
            number(&bottom),
            GAIN_RANGE_DB.0,
            "a finite number the mixer takes"
        );
        let top = map.step(cc(48, 127), &view);
        assert_eq!(number(&top), 12.0);
    }

    #[test]
    fn a_value_moved_by_something_else_has_to_be_found_again() {
        let view = view();
        let mut map = map(&[(fader(48), "input/1/gain")]);
        let at_zero = AT_ZERO;
        assert!(map.step(cc(48, at_zero), &view).op.is_some());

        // The same view does not show a change yet; a value that seems old is no reason to stop.
        assert!(map.step(cc(48, at_zero - 20), &view).op.is_some());

        // A later view shows what the fader set: it stays picked up.
        let mut followed = later(&view);
        followed.inputs[0].gain_db = fader_db(f32::from(at_zero - 20) / 127.0);
        assert!(map.step(cc(48, at_zero - 21), &followed).op.is_some());

        // A scene moved it elsewhere: the next touch does not jump.
        let mut scene = later(&followed);
        scene.inputs[0].gain_db = -40.0;
        let step = map.step(cc(48, at_zero - 22), &scene);
        assert_eq!(step.op, None, "{step:?}");
        assert!(step.note.expect("told").contains("waits"));
    }

    #[test]
    fn an_endless_knob_steps_from_where_the_value_is_and_has_nothing_to_find() {
        let view = view();
        let mut map = Map::default();
        map.learn("input/1/gain".to_owned());
        let learned = map.step(cc(48, 1), &view);
        assert!(learned.learned);
        assert!(map.bindings()[0].relative, "a first value of 1 is a step");

        // Far from anything a fader could have found: it moves at once, by one step.
        let up = number(&map.step(cc(48, 1), &view));
        assert!(up > 0.0 && up < 1.0, "{up}");
        // A second step before a new view adds to the first.
        let more = number(&map.step(cc(48, 2), &view));
        assert!(more > up, "{more} after {up}");
        let down = number(&map.step(cc(48, 127), &view));
        assert!(down < more, "{down} after {more}");

        // A new view is believed: a scene moved it, and the knob goes on from there.
        let mut scene = later(&view);
        scene.inputs[0].gain_db = -20.0;
        let on = number(&map.step(cc(48, 1), &scene));
        assert!(on > -20.0 && on < -18.0, "{on}");

        // The ends hold.
        let mut top = later(&scene);
        top.inputs[0].gain_db = 12.0;
        assert_eq!(number(&map.step(cc(48, 63), &top)), 12.0);
    }

    #[test]
    fn the_device_decides_how_far_a_step_of_a_knob_goes() {
        assert_eq!(knob_steps("nanoKONTROL2"), 127.0);
        assert_eq!(knob_steps("APC Key 25"), 127.0, "not measured");
        assert_eq!(knob_steps("MIDIIN2 (APC Key 25 mk2)"), 400.0);
        let view = view();
        let knob = Bound {
            control: Control {
                kind: ControlKind::Cc,
                channel: 0,
                number: 48,
            },
            target: "input/1/gain".to_owned(),
            relative: true,
        };
        let step = |steps: Option<&str>| {
            let mut map = Map::new(vec![knob.clone()]);
            if let Some(device) = steps {
                map.set_device(device);
            }
            number(&map.step(cc(48, 1), &view))
        };
        let coarse = step(None);
        let fine = step(Some("MIDIIN2 (APC Key 25 mk2)"));
        assert!(fine > 0.0 && fine < coarse, "{fine} {coarse}");
    }

    #[test]
    fn a_first_value_in_the_middle_is_no_step() {
        for value in [8, 64, 100, 120] {
            assert!(!looks_relative(value), "{value}");
        }
        for value in [1, 6, 123, 127] {
            assert!(looks_relative(value), "{value}");
        }
    }

    #[test]
    fn a_pan_has_its_middle_and_picks_up_too() {
        let view = view();
        let mut map = map(&[(fader(50), "input/1/pan")]);
        assert_eq!(number(&map.step(cc(50, 64), &view)), 0.0);
        assert_eq!(number(&map.step(cc(50, 63), &view)), 0.0);
        assert_eq!(number(&map.step(cc(50, 127), &view)), 1.0);
        assert_eq!(number(&map.step(cc(50, 0), &view)), -1.0);

        let mut far = Map::new(map.bindings().to_vec());
        assert_eq!(far.step(cc(50, 0), &view).op, None);
    }

    #[test]
    fn a_button_toggles_on_its_press_only_and_two_presses_are_two() {
        let view = view();
        let mut map = map(&[(button(1), "input/1/mute")]);
        assert!(switch(&map.step(press(1), &view)), "it was off");
        assert_eq!(map.step(release(1), &view), Step::default());
        // Before a new view shows the first press.
        assert!(!switch(&map.step(press(1), &view)));

        // A new view is believed again.
        let mut muted = later(&view);
        muted.inputs[0].muted = true;
        assert!(!switch(&map.step(press(1), &muted)));
    }

    #[test]
    fn a_held_cc_switch_toggles_once() {
        let view = view();
        let mut map = map(&[(fader(60), "bus/A1/mute")]);
        assert!(switch(&map.step(cc(60, 127), &view)));
        assert_eq!(map.step(cc(60, 127), &view), Step::default());
        assert_eq!(map.step(cc(60, 0), &view), Step::default());
        assert!(!switch(&map.step(cc(60, 127), &view)));
    }

    #[test]
    fn a_scene_is_pressed_and_every_op_is_one_the_mixer_takes() {
        let view = view();
        let mut map = map(&[
            (button(1), "scene/abend"),
            (button(2), "matrix/1/A2"),
            (button(3), "input/1/solo"),
            (fader(48), "input/1/gain"),
            (fader(49), "input/1/pan"),
            (fader(50), "input/1/gate"),
            (fader(51), "input/1/comp"),
            (fader(52), "bus/A1/eq/2/gain"),
            (fader(53), "bus/A1/eq/2/freq"),
            (fader(54), "bus/A1/eq/2/q"),
            (button(4), "bus/A1/eq"),
        ]);
        let scene = map.step(press(1), &view);
        assert_eq!(
            scene.op,
            Some(Op::Scene {
                name: "Abend".to_owned()
            })
        );
        assert_eq!(map.step(release(1), &view), Step::default());
        let at_zero = AT_ZERO;
        // The faders start near where the values stand, so that they pick them up.
        for message in [
            press(2),
            press(3),
            press(4),
            cc(48, at_zero),
            cc(49, 64),
            cc(50, 0),
            cc(51, 1),
            cc(52, 64),
            cc(53, 59),
            cc(54, 54),
        ] {
            let op = map.step(message, &view).op.expect("an op");
            assert!(op.action(&view).is_ok(), "{op:?}: {:?}", op.action(&view));
        }
        // An endless knob too.
        let mut turned = Map::new(
            ["input/1/comp", "bus/A1/eq/2/freq"]
                .iter()
                .zip([60, 61])
                .map(|(target, number)| Bound {
                    control: fader(number),
                    target: (*target).to_owned(),
                    relative: true,
                })
                .collect(),
        );
        for number in [60, 61] {
            let op = turned.step(cc(number, 1), &view).op.expect("an op");
            assert!(op.action(&view).is_ok(), "{op:?}: {:?}", op.action(&view));
        }
    }

    #[test]
    fn what_is_not_bound_or_not_in_this_route_does_nothing() {
        let view = view();
        let mut map = map(&[(button(1), "input/9/mute"), (button(2), "scene/weg")]);
        assert_eq!(map.step(press(1), &view), Step::default());
        assert_eq!(map.step(press(2), &view), Step::default());
        assert_eq!(map.step(press(3), &view), Step::default());
        assert_eq!(map.step(cc(48, 1), &view), Step::default());
    }

    #[test]
    fn the_palette_goes_by_the_device() {
        assert_eq!(Palette::for_device("APC MINI").red, 3);
        assert_eq!(Palette::for_device("APC mini mk2 Control").channel, Some(6));
        assert_eq!(Palette::for_device("nanoKONTROL2").red, 127);
    }

    #[test]
    fn lights_say_only_what_changed_within_a_limit() {
        let palette = Palette::for_device("APC MINI");
        let mut view = view();
        let mut map = map(&[
            (button(1), "input/1/mute"),
            (button(2), "matrix/1/A1"),
            (button(3), "input/1/solo"),
            (button(4), "scene/abend"),
            (fader(48), "input/1/gain"),
        ]);
        // All of them the first time, faders never, in pieces when limited.
        let first = map.lights(&view, palette, 2);
        assert_eq!(first, [[0x90, 1, 0], [0x90, 2, 1]]);
        let rest = map.lights(&view, palette, 16);
        assert_eq!(rest, [[0x90, 3, 0], [0x90, 4, 5]]);
        assert_eq!(map.lights(&view, palette, 16), Vec::<[u8; 3]>::new());

        view.inputs[0].muted = true;
        assert_eq!(map.lights(&view, palette, 16), [[0x90, 1, 3]]);

        // A device that came back gets all of it again.
        map.forget_lights();
        assert_eq!(map.lights(&view, palette, 16).len(), 4);

        // A button that lost its binding goes dark, once.
        assert!(map.forget("scene/abend"));
        assert_eq!(map.lights(&view, palette, 16), [[0x90, 4, 0]]);
        assert_eq!(map.lights(&view, palette, 16), Vec::<[u8; 3]>::new());
        assert!(!map.forget("scene/abend"));
    }

    #[test]
    fn the_mk2_takes_its_brightness_from_the_channel() {
        let view = view();
        let mut map = map(&[(button(4), "scene/abend")]);
        let palette = Palette::for_device("APC mini mk2");
        assert_eq!(map.lights(&view, palette, 16), [[0x96, 4, 96]]);
    }

    #[test]
    fn a_status_says_what_is_bound() {
        let mut map = map(&[(fader(48), "input/1/gain")]);
        map.learn("bus/A1/mute".to_owned());
        let status = map.status(Some("APC MINI".to_owned()), true);
        assert_eq!(status.learning.as_deref(), Some("bus/A1/mute"));
        assert_eq!(status.bindings[0].control, "CC 48");
        assert_eq!(status.bindings[0].target, "input/1/gain");
    }
}
