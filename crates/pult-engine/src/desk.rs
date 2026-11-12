//! A running route as a mixing desk for the interface: [`EngineDesk`] implements
//! [`pult_core::Desk`] over [`Running`], so the window sees the same [`MixerView`] it sees of
//! the demo desk.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Instant;

use pult_core::control::{Command, Target};
use pult_core::{
    Action, BusView, Desk, Dropouts, EventKind, EventView, InputView, MeterPoint, MixerView, Reply,
    SendView, SourceKind, linear_to_db,
};

use crate::{Change, InputKind, Running, profile};

/// A running route behind the [`Desk`] interface. Taking a view also supervises the devices
/// (a lost one is looked for again), so call [`Desk::view`] a few times a second.
pub struct EngineDesk {
    running: Running,
    start: Instant,
    profile: Option<PathBuf>,
    meter_point: MeterPoint,
    /// Which inputs are soloed; the engine only knows who is soloed out.
    solo: Vec<bool>,
    dropouts: Dropouts,
}

impl EngineDesk {
    /// The desk of `running`, which came from `profile` (if any).
    #[must_use]
    pub fn new(running: Running, profile: Option<PathBuf>) -> Self {
        let solo = vec![false; running.inputs.len()];
        Self {
            running,
            start: Instant::now(),
            profile,
            meter_point: MeterPoint::default(),
            solo,
            dropouts: Dropouts::default(),
        }
    }

    /// Reads the profile at `path`, starts its route and returns its desk: what the window
    /// does at start.
    ///
    /// # Errors
    ///
    /// A message when the profile cannot be read or is invalid, or the route does not start
    /// (a device missing, a stream refused).
    pub fn open(path: &Path) -> Result<Self, String> {
        let spec = profile::load(path).map_err(|e| e.to_string())?;
        let running = crate::start(&spec).map_err(|e| e.to_string())?;
        Ok(Self::new(running, Some(path.to_path_buf())))
    }

    /// The route behind the desk.
    #[must_use]
    pub fn running(&self) -> &Running {
        &self.running
    }

    /// The route behind the desk, for what a console does with it directly.
    pub fn running_mut(&mut self) -> &mut Running {
        &mut self.running
    }

    /// Silences every input that is not soloed while any is.
    fn apply_solo(&self) {
        let soloing = self.solo.contains(&true);
        for (input, &solo) in self.running.inputs.iter().zip(&self.solo) {
            input.controls.fade_ms.store(0, Ordering::Relaxed);
            input
                .controls
                .soloed_out
                .store(soloing && !solo, Ordering::Release);
        }
    }

    fn input_index(&self, input: usize) -> Result<usize, String> {
        if input < self.running.inputs.len() {
            Ok(input)
        } else {
            Err(format!(
                "there is no input {} (only {})",
                input + 1,
                self.running.inputs.len()
            ))
        }
    }

    fn bus_index(&self, bus: usize) -> Result<usize, String> {
        if bus < self.running.buses.len() {
            Ok(bus)
        } else {
            Err(format!(
                "there is no bus {} (only {})",
                bus + 1,
                self.running.buses.len()
            ))
        }
    }
}

/// A linear peak in dBFS; `None` for silence.
fn peak_db(linear: f32) -> Option<f32> {
    (linear > 0.0).then(|| linear_to_db(linear))
}

/// Whichever of two corrections lies further from zero.
/// Peak, drift and dropouts of one input, read for a view. Before the fader the input has one
/// meter, its stage's; both meters are taken, so the one switched to starts fresh.
fn levels(input: &crate::RunningInput) -> (f32, f32, u64) {
    let mut peak: f32 = 0.0;
    let mut drift: f32 = 0.0;
    let mut dropouts = 0;
    for route in input.routes.iter().filter(|route| route.is_routed()) {
        peak = peak.max(route.stats.peak.take());
        drift = farther(drift, route.stats.correction_ppm.load());
        dropouts += route.stats.underruns.get() + route.stats.resyncs.get();
    }
    let pre = input.controls.pre_peak.take();
    if input.controls.meter_pre_fader.load(Ordering::Relaxed) {
        peak = pre;
    }
    (peak, drift, dropouts)
}

/// The send of `spec` to each of `buses` buses, as a view shows them.
fn sends_of(spec: &crate::InputSpec, buses: usize) -> Vec<SendView> {
    (0..buses)
        .map(|bus| {
            let send = spec.send(bus);
            SendView {
                db: send.gain_db,
                pre: send.pre,
            }
        })
        .collect()
}

fn farther(a: f32, b: f32) -> f32 {
    if b.abs() > a.abs() { b } else { a }
}

impl Desk for EngineDesk {
    fn view(&mut self) -> MixerView {
        let elapsed = self.start.elapsed();
        let events = self
            .running
            .supervise()
            .into_iter()
            .map(|change| EventView {
                elapsed,
                kind: match change {
                    Change::Lost { .. } => EventKind::Lost,
                    Change::Back { .. } => EventKind::Back,
                    Change::Refused { .. } => EventKind::Refused,
                    Change::Slow { .. } => EventKind::Slow,
                },
                text: change.to_string(),
            })
            .collect();
        let running = &self.running;
        // Over every ring, routed or not, so that routing does not move the sum.
        let raw = running.xruns()
            + running
                .inputs
                .iter()
                .flat_map(|input| &input.routes)
                .map(|route| route.stats.underruns.get() + route.stats.resyncs.get())
                .sum::<u64>();
        self.dropouts.update(raw, elapsed);
        let inputs = running
            .inputs
            .iter()
            .zip(&running.spec.inputs)
            .zip(&self.solo)
            .map(|((input, spec), &solo)| {
                let (peak, drift, dropouts) = levels(input);
                InputView {
                    name: input.name.clone(),
                    label: spec.label.clone(),
                    kind: match spec.kind {
                        InputKind::Device => SourceKind::Device,
                        InputKind::Process => SourceKind::Program,
                    },
                    sample_rate: input.sample_rate,
                    gain_db: spec.gain_db,
                    muted: spec.muted,
                    solo,
                    pan: spec.pan,
                    gate: spec.gate,
                    comp: spec.comp,
                    buses: input
                        .routes
                        .iter()
                        .filter(|route| route.is_routed())
                        .map(|route| route.bus)
                        .collect(),
                    sends: sends_of(spec, input.routes.len()),
                    present: input.present,
                    peak_db: peak_db(peak),
                    drift_ppm: drift,
                    dropouts,
                }
            })
            .collect();
        let buses = running
            .buses
            .iter()
            .zip(&running.spec.buses)
            .map(|(bus, spec)| BusView {
                name: bus.name.clone(),
                label: spec.label.clone(),
                device: bus.device.clone(),
                sample_rate: bus.sample_rate,
                gain_db: spec.gain_db,
                muted: spec.muted,
                eq_on: spec.eq_on,
                eq: spec.eq,
                limit_on: spec.limit_on,
                limit_db: spec.limit_db,
                limit_lookahead_ms: spec.limit_lookahead_ms,
                monitor: spec.monitor,
                minus: spec.minus,
                limited_db: bus.handle.stats.limited_db.take(),
                present: bus.present,
                peak_db: peak_db(bus.handle.stats.peak.take()),
                clipped: bus.handle.stats.clipped.get(),
            })
            .collect();
        MixerView {
            elapsed,
            dropouts_per_hour: self.dropouts.per_hour(elapsed),
            glitches: running.xruns(),
            meter_point: self.meter_point,
            inputs,
            buses,
            events,
            profile: self.profile.clone(),
            scenes: running.spec.scenes.iter().map(|s| s.name.clone()).collect(),
            midi: None,
        }
    }

    fn act(&mut self, action: Action) -> Reply {
        match action {
            Action::InputGain { input, db, ramp_ms } => {
                let target = Target::Input(self.input_index(input)?);
                self.running.apply(&Command::Gain(target, db, ramp_ms))
            }
            Action::InputMute { input, muted } => {
                let target = Target::Input(self.input_index(input)?);
                self.running.apply(&Command::Mute(target, muted))
            }
            Action::InputPan {
                input,
                pan,
                ramp_ms,
            } => {
                let index = self.input_index(input)?;
                self.running.apply(&Command::Pan(index, pan, ramp_ms))
            }
            Action::InputGate { input, amount } => {
                let index = self.input_index(input)?;
                self.running.apply(&Command::Gate(index, amount))
            }
            Action::InputComp { input, amount } => {
                let index = self.input_index(input)?;
                self.running.apply(&Command::Comp(index, amount))
            }
            Action::BusGain { bus, db, ramp_ms } => {
                let target = Target::Bus(self.bus_index(bus)?);
                self.running.apply(&Command::Gain(target, db, ramp_ms))
            }
            Action::BusEq { bus, on } => {
                let index = self.bus_index(bus)?;
                self.running.apply(&Command::Eq(index, on))
            }
            Action::BusEqBand { bus, band, change } => {
                let index = self.bus_index(bus)?;
                self.running.apply(&Command::EqBand(index, band, change))
            }
            Action::BusLimit { bus, change } => {
                let index = self.bus_index(bus)?;
                self.running.apply(&Command::Limit(index, change))
            }
            Action::BusMute { bus, muted } => {
                let target = Target::Bus(self.bus_index(bus)?);
                self.running.apply(&Command::Mute(target, muted))
            }
            Action::InputSolo { input, solo } => {
                let index = self.input_index(input)?;
                self.solo[index] = solo;
                self.apply_solo();
                let state = if solo { "solo" } else { "solo off" };
                Ok(vec![format!(
                    "[{}] {}: {state}",
                    index + 1,
                    self.running.inputs[index].name
                )])
            }
            Action::SetMeterPoint(point) => {
                self.meter_point = point;
                let pre = point == MeterPoint::PreFader;
                for input in &self.running.inputs {
                    input.controls.meter_pre_fader.store(pre, Ordering::Relaxed);
                }
                let place = if pre { "before" } else { "after" };
                Ok(vec![format!("input meters measure {place} the fader")])
            }
            Action::SaveProfile { path } => {
                let path = path
                    .or_else(|| self.profile.clone())
                    .ok_or("no profile file yet; name one")?;
                // The path as it is, not through a command string: Windows paths need not
                // be valid Unicode.
                let lines = self.running.save_profile(&path)?;
                self.profile = Some(path);
                Ok(lines)
            }
            Action::Route { input, bus, on } => self.running.route(input, bus, on),
            Action::BusMonitor { bus, on } => self.running.monitor(bus, on),
            Action::Send {
                input,
                bus,
                db,
                ramp_ms,
            } => self.running.send(input, bus, db, ramp_ms),
            Action::SendPre { input, bus, pre } => self.running.send_pre(input, bus, pre),
            Action::Scene { name } => self.running.scene(&name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_names_a_profile_it_cannot_read() {
        match EngineDesk::open(Path::new("no-such-profile.toml")) {
            Ok(_) => panic!("opened a missing profile"),
            Err(why) => assert!(why.contains("cannot read no-such-profile.toml"), "{why}"),
        }
    }

    #[test]
    fn peaks_and_corrections_read_as_the_view_wants_them() {
        assert_eq!(peak_db(0.0), None);
        assert!((peak_db(0.5).unwrap() + 6.02).abs() < 0.01);
        assert!((farther(3.0, -5.0) + 5.0).abs() < f32::EPSILON);
        assert!((farther(-5.0, 3.0) + 5.0).abs() < f32::EPSILON);
    }
}
