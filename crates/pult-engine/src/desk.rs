//! A running route as a mixing desk for the interface: [`EngineDesk`] implements
//! [`pult_core::Desk`] over [`Running`], so the window sees the same [`MixerView`] it sees of
//! the demo desk.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Instant;

use pult_core::control::{Command, Target};
use pult_core::{
    Action, BusView, Desk, EventView, InputView, MeterPoint, MixerView, Reply, SourceKind,
    linear_to_db,
};

use crate::{InputKind, Running, profile};

/// A running route behind the [`Desk`] interface. Taking a view also supervises the devices
/// (a lost one is looked for again), so call [`Desk::view`] a few times a second.
pub struct EngineDesk {
    running: Running,
    start: Instant,
    profile: Option<PathBuf>,
    meter_point: MeterPoint,
    /// Which inputs are soloed; the engine only knows who is soloed out.
    solo: Vec<bool>,
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

    /// Silences every input that is not soloed while any is.
    fn apply_solo(&self) {
        let soloing = self.solo.contains(&true);
        for (input, &solo) in self.running.inputs.iter().zip(&self.solo) {
            input
                .controls
                .soloed_out
                .store(soloing && !solo, Ordering::Relaxed);
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
                text: change.to_string(),
            })
            .collect();
        let running = &self.running;
        let mut dropouts_total = running.xruns();
        let inputs = running
            .inputs
            .iter()
            .zip(&running.spec.inputs)
            .zip(&self.solo)
            .map(|((input, spec), &solo)| {
                let mut peak: f32 = 0.0;
                let mut drift: f32 = 0.0;
                let mut dropouts = 0;
                for route in input.routes.iter().filter(|route| route.is_routed()) {
                    peak = peak.max(route.stats.peak.take());
                    drift = farther(drift, route.stats.correction_ppm.load());
                    dropouts += route.stats.underruns.get() + route.stats.resyncs.get();
                }
                dropouts_total += dropouts;
                InputView {
                    name: input.name.clone(),
                    kind: match spec.kind {
                        InputKind::Device => SourceKind::Device,
                        InputKind::Process => SourceKind::Program,
                    },
                    sample_rate: input.sample_rate,
                    gain_db: spec.gain_db,
                    muted: spec.muted,
                    solo,
                    pan: spec.pan,
                    buses: input
                        .routes
                        .iter()
                        .filter(|route| route.is_routed())
                        .map(|route| route.bus)
                        .collect(),
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
                device: bus.device.clone(),
                sample_rate: bus.sample_rate,
                gain_db: spec.gain_db,
                muted: spec.muted,
                present: bus.present,
                peak_db: peak_db(bus.handle.stats.peak.take()),
                clipped: bus.handle.stats.clipped.get(),
            })
            .collect();
        let seconds = elapsed.as_secs_f64();
        #[allow(clippy::cast_precision_loss)]
        let dropouts_per_hour = if seconds < 1.0 {
            0.0
        } else {
            dropouts_total as f64 * 3600.0 / seconds
        };
        MixerView {
            elapsed,
            dropouts_per_hour,
            glitches: running.xruns(),
            meter_point: self.meter_point,
            inputs,
            buses,
            events,
            profile: self.profile.clone(),
        }
    }

    fn act(&mut self, action: Action) -> Reply {
        match action {
            Action::InputGain { input, db } => {
                let target = Target::Input(self.input_index(input)?);
                self.running.apply(&Command::Gain(target, db))
            }
            Action::InputMute { input, muted } => {
                let target = Target::Input(self.input_index(input)?);
                self.running.apply(&Command::Mute(target, muted))
            }
            Action::InputPan { input, pan } => {
                let index = self.input_index(input)?;
                self.running.apply(&Command::Pan(index, pan))
            }
            Action::BusGain { bus, db } => {
                let target = Target::Bus(self.bus_index(bus)?);
                self.running.apply(&Command::Gain(target, db))
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
                let lines = self
                    .running
                    .apply(&Command::Save(path.to_string_lossy().into_owned()))?;
                self.profile = Some(path);
                Ok(lines)
            }
            Action::Route { input, bus, on } => self.running.route(input, bus, on),
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
