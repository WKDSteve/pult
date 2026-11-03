//! What a user interface shows of a running mixer, and what it can ask of it.
//!
//! The window (`pult-app`) knows only this module: a [`Desk`] hands out a [`MixerView`],
//! a plain snapshot without references into the engine, and takes [`Action`]s. The engine
//! side implements the trait over a running route; [`FakeDesk`] implements it in memory,
//! with moving meters, so the interface can be built and tested without any audio device.
//!
//! Indices count from zero in the order of the route (the console's `[1]` is index 0).

use std::path::PathBuf;
use std::time::Duration;

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
}

/// What an input captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// A capture device (microphone, line in, a virtual cable).
    Device,
    /// The sound of one program (process loopback).
    Program,
}

/// One input as a channel strip shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct InputView {
    /// The device's or program's full name.
    pub name: String,
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
    /// The buses it feeds, as indices into [`MixerView::buses`].
    pub buses: Vec<usize>,
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

/// One bus as its master strip shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct BusView {
    /// The short name (`A1`).
    pub name: String,
    /// The output device's full name.
    pub device: String,
    /// Its sample rate in Hz.
    pub sample_rate: u32,
    /// Gain in dB.
    pub gain_db: f32,
    /// Muted.
    pub muted: bool,
    /// Whether the device is there; false while it is gone.
    pub present: bool,
    /// Peak of the output since the previous view in dBFS; `None` for silence.
    pub peak_db: Option<f32>,
    /// Samples clipped so far.
    pub clipped: u64,
}

/// Something that happened to a device, as one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventView {
    /// When, as time since the route started.
    pub elapsed: Duration,
    /// What, as the console says it (`A1 Focusrite is back`).
    pub text: String,
}

/// Where the input meters measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MeterPoint {
    /// Before gain and mute: what arrives.
    PreFader,
    /// After gain and mute: what goes into the mix.
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
    },
    /// Sets a bus's gain in dB.
    BusGain {
        /// Bus index.
        bus: usize,
        /// Gain in dB.
        db: f32,
    },
    /// Mutes or unmutes a bus.
    BusMute {
        /// Bus index.
        bus: usize,
        /// Muted.
        muted: bool,
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
    /// Saves the current state as a profile; `None` saves to [`MixerView::profile`].
    SaveProfile {
        /// Where to.
        path: Option<PathBuf>,
    },
    /// Moves the input meters before or after the fader.
    SetMeterPoint(MeterPoint),
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
            kind,
            sample_rate,
            gain_db,
            muted,
            solo: false,
            pan: 0.0,
            buses,
            present: true,
            peak_db: None,
            drift_ppm: 0.0,
            dropouts: 0,
        };
        let bus = |name: &str, device: &str, sample_rate| BusView {
            name: name.to_owned(),
            device: device.to_owned(),
            sample_rate,
            gain_db: 0.0,
            muted: false,
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

    fn input_mut(&mut self, input: usize) -> Result<&mut InputView, String> {
        let count = self.view.inputs.len();
        self.view
            .inputs
            .get_mut(input)
            .ok_or_else(|| format!("there is no input {} (only {count})", input + 1))
    }

    fn bus_mut(&mut self, bus: usize) -> Result<&mut BusView, String> {
        let count = self.view.buses.len();
        self.view
            .buses
            .get_mut(bus)
            .ok_or_else(|| format!("there is no bus {} (only {count})", bus + 1))
    }
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
                text: format!("[3] {} is gone (demo)", self.view.inputs[2].name),
            });
        } else if self.frame % 600 == 90 && !self.view.inputs[2].present {
            self.view.inputs[2].present = true;
            self.view.events.push(EventView {
                elapsed: self.view.elapsed,
                text: format!("[3] {} is back", self.view.inputs[2].name),
            });
        }
        let snapshot = self.view.clone();
        self.view.events.clear();
        snapshot
    }

    fn act(&mut self, action: Action) -> Reply {
        match action {
            Action::InputGain { input, db } => {
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
            Action::InputPan { input, pan } => {
                if !(-1.0..=1.0).contains(&pan) {
                    return Err(format!("pan {pan} is outside -1 to 1"));
                }
                self.input_mut(input)?.pan = pan;
                Ok(vec![format!("[{}] pan {pan:.2}", input + 1)])
            }
            Action::BusGain { bus, db } => {
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
                    db: -12.0
                })
                .is_ok()
        );
        assert!(mixer.act(Action::InputGain { input: 0, db: 40.0 }).is_err());
        // The whole range of profiles and commands, down to -96 dB, is accepted.
        assert!(
            mixer
                .act(Action::InputGain {
                    input: 1,
                    db: -90.0
                })
                .is_ok()
        );
        assert!(
            mixer
                .act(Action::InputGain {
                    input: 1,
                    db: -97.0
                })
                .is_err()
        );
        assert!(mixer.act(Action::InputGain { input: 9, db: 0.0 }).is_err());
        assert!(mixer.act(Action::InputPan { input: 0, pan: 2.0 }).is_err());
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
    fn the_fake_loses_and_finds_its_program_and_reports_it_once() {
        let mut mixer = FakeDesk::new();
        let mut events = Vec::new();
        let mut missing = false;
        for _ in 0..700 {
            let view = mixer.view();
            missing |= !view.inputs[2].present;
            events.extend(view.events.into_iter().map(|e| e.text));
        }
        assert!(missing);
        assert_eq!(events.len(), 2, "{events:?}");
        assert!(events[0].ends_with("is gone (demo)"));
        assert!(events[1].ends_with("is back"));
    }

    #[test]
    fn levels_add_as_power() {
        assert_eq!(sum_db(&[]), None);
        let two = sum_db(&[-6.0, -6.0]).unwrap();
        assert!((two + 2.99).abs() < 0.02, "{two}");
    }
}
