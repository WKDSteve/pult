//! A running route as a mixing desk for the interface: [`EngineDesk`] implements
//! [`pult_core::Desk`] over [`Running`], so the window sees the same [`MixerView`] it sees of
//! the demo desk.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Instant;

use pult_core::control::{Command, Target};
use pult_core::{
    Action, BusView, Desk, Dropouts, EventKind, EventView, InputView, LoudnessView, MeterPoint,
    MixerView, Reply, RingView, SendView, SourceKind, linear_to_db,
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
            input.controls.soloed.store(solo, Ordering::Release);
        }
    }

    /// What the route does itself: every action but the three of the desk (solo, the meter
    /// point, saving a profile).
    fn route_action(&mut self, action: Action) -> Reply {
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
            Action::Route { input, bus, on } => self.running.route(input, bus, on),
            Action::BusMonitor { bus, on } => self.running.monitor(bus, on),
            Action::BusDim { bus, on } => self.running.dim(bus, Some(on), None),
            Action::BusDimLevel { bus, db } => self.running.dim(bus, None, Some(db)),
            Action::InputTalkback { input, on } => self.running.talkback(input, on),
            Action::InputEq { input, on } => self.running.apply(&Command::InputEq(input, on)),
            Action::InputEqBand {
                input,
                band,
                change,
            } => self
                .running
                .apply(&Command::InputEqBand(input, band, change)),
            Action::InputEqPre { input, pre } => {
                self.running.apply(&Command::InputEqPre(input, pre))
            }
            Action::InputLimit { input, change } => {
                self.running.apply(&Command::InputLimit(input, change))
            }
            Action::Talk { on } => Ok(self.running.talk(on)),
            Action::Pfl { on } => Ok(self.running.pfl(on)),
            Action::BusComp { bus, amount } => self.running.bus_comp(bus, Some(amount), None, None),
            Action::BusCompMix { bus, percent } => {
                self.running.bus_comp(bus, None, Some(percent), None)
            }
            Action::BusCompHpf { bus, hz } => self.running.bus_comp(bus, None, None, Some(hz)),
            Action::BusMultiband { bus, change } => self.running.multiband(bus, change),
            Action::BusDelay { bus, ms } => self.running.bus_delay(bus, ms),
            Action::InputAutomix { input, on } => self.running.automix(input, Some(on), None),
            Action::InputStrip { input, change } => self.running.strip(input, change),
            Action::InputDuck { input, change } => self.running.duck(input, change),
            Action::InputSidechain { input, change } => self.running.sidechain(input, change),
            Action::InputDeess { input, change } => self.running.deess(input, change),
            Action::InputAutomixWeight { input, db } => self.running.automix(input, None, Some(db)),
            Action::Send {
                input,
                bus,
                db,
                ramp_ms,
            } => self.running.send(input, bus, db, ramp_ms),
            Action::SendPre { input, bus, pre } => self.running.send_pre(input, bus, pre),
            Action::Scene { name } => self.running.scene(&name),
            Action::InputSolo { .. } | Action::SetMeterPoint(_) | Action::SaveProfile { .. } => {
                Err("that is for the desk, not for the route".to_owned())
            }
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

/// One input as the view shows it; takes its meters.
fn input_view(input: &crate::RunningInput, spec: &crate::InputSpec, solo: bool) -> InputView {
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
        automix: spec.automix,
        automix_weight_db: spec.automix_weight_db,
        automix_gain_db: automix_gain_db(input, spec),
        gate_reduction_db: input.controls.gate_reduction.take(),
        gate_open: input.controls.gate_open.load(Ordering::Relaxed),
        comp_reduction_db: input.controls.comp_reduction.take(),
        strip: spec.strip,
        talkback: spec.talkback,
        eq_on: spec.eq_on,
        eq: spec.eq,
        eq_now_db: input.controls.eq.gains_now(),
        eq_pre: spec.eq_pre,
        limit_on: spec.limit_on,
        limit_db: spec.limit_db,
        limited_db: input.controls.limited.take(),
        duck: spec.duck,
        sidechain: spec.sidechain,
        deess: spec.deess,
        deess_reduction_db: input.controls.deess_reduction.take(),
        duck_reduction_db: input.controls.duck_reduction.take(),
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
}

/// What the automixer gives `input` now in dB; 0 outside it.
fn automix_gain_db(input: &crate::RunningInput, spec: &crate::InputSpec) -> f32 {
    if !spec.automix {
        return 0.0;
    }
    input.controls.automix.get().map_or(0.0, |(group, index)| {
        20.0 * group.gain(*index).max(1e-6).log10()
    })
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

/// What of each input that feeds `bus` waits in the ring between them.
fn rings_of(inputs: &[crate::RunningInput], bus: usize) -> Vec<RingView> {
    inputs
        .iter()
        .enumerate()
        .flat_map(|(input, running)| {
            running
                .routes
                .iter()
                .filter(move |route| route.bus == bus && route.is_routed())
                .map(move |route| RingView {
                    input,
                    ms: route.stats.buffered_ms.load(),
                })
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
            .map(|((input, spec), &solo)| input_view(input, spec, solo))
            .collect();
        let buses = running
            .buses
            .iter()
            .zip(&running.spec.buses)
            .enumerate()
            .map(|(index, (bus, spec))| BusView {
                name: bus.name.clone(),
                label: spec.label.clone(),
                device: bus.device.clone(),
                sample_rate: bus.sample_rate,
                gain_db: spec.gain_db,
                muted: spec.muted,
                eq_on: spec.eq_on,
                eq: spec.eq,
                eq_now_db: bus.handle.controls.eq.gains_now(),
                limit: spec.limit,
                monitor: spec.monitor,
                minus: spec.minus,
                comp: spec.comp,
                comp_mix: spec.comp_mix,
                comp_hpf_hz: spec.comp_hpf_hz,
                delay_ms: spec.delay_ms,
                rings: rings_of(&running.inputs, index),
                device_latency_ms: bus
                    .present
                    .then(|| bus.handle.stats.device_latency.get())
                    .flatten(),
                dim: running.is_dimmed(index),
                dim_db: spec.dim_db,
                comp_reduction_db: bus.handle.controls.comp.reduction.take(),
                multiband: spec.multiband,
                multiband_reduction_db: std::array::from_fn(|band| {
                    bus.handle.controls.multiband.reduction[band].take()
                }),
                loudness: bus
                    .loudness
                    .as_deref()
                    .map_or_else(LoudnessView::default, |l| LoudnessView {
                        momentary_lufs: l.momentary(),
                        short_term_lufs: l.short_term(),
                        integrated_lufs: l.integrated(),
                        range_lu: l.range(),
                        true_peak_db: l.true_peak(),
                    }),
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
            talk: running.is_talking(),
            pfl: running.is_pfl(),
        }
    }

    fn act(&mut self, action: Action) -> Reply {
        match action {
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
            other => self.route_action(other),
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
