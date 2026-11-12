//! A route without devices: the same rings, mixers and controls as [`crate::start`] builds, fed
//! and drained by the caller instead of by audio callbacks. Profiles, scenes and commands work on
//! it through [`Offline::running`] exactly as on a route that plays, so what the buses put out can
//! be checked sample by sample (the click test of `tests/clicks.rs` does). Inputs may run at
//! other rates than the buses ([`Offline::with_rates`]) and deliver any number of frames per
//! callback (a clock that drifts, an input that runs dry), and a bus can lose its device for a
//! while ([`Offline::suspend`], [`Offline::resume`]).

use std::sync::Arc;

use pult_core::{Counter, Mixer, MixerInput};

use crate::slot::InputFeed;
use crate::{Error, Rings, RouteSpec, Running, automix_group, make_rings, set_up_bus};

/// A route whose inputs and buses are buffers.
#[derive(Debug)]
pub struct Offline {
    running: Running,
    /// Per input, one capture per bus, as its device callback would push them.
    inputs_fed: Vec<InputFeed>,
    /// The loudness meters of the buses, drained after every callback: no thread here.
    meters: Vec<pult_core::LoudnessMeter>,
    /// Per bus, its mixer, as its device callback would run it.
    mixers: Vec<Mixer>,
    /// Per bus, the output of the latest [`Self::process`].
    outputs: Vec<Vec<f32>>,
    channels: usize,
    /// Per bus, whether its device is gone: its mixer is not called and it plays silence.
    suspended: Vec<bool>,
}

impl Offline {
    /// Builds the route of `spec` as if every input delivered `channels` channels at `rate` Hz and
    /// every bus played them at that rate, and opens the buses as their first callback would.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRoute`] when the spec does not hold together, or `channels` is zero.
    pub fn new(spec: &RouteSpec, rate: u32, channels: u16) -> Result<Self, Error> {
        Self::with_rates(spec, &vec![rate; spec.inputs.len()], rate, channels)
    }

    /// Like [`Self::new`], but input `i` delivers at `input_rates[i]` Hz and every bus plays at
    /// `bus_rate`: each ring resamples from its input's rate to its bus's, as with real devices.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRoute`] when the spec does not hold together, `channels` or a rate is zero,
    /// or `input_rates` does not have one rate per input.
    pub fn with_rates(
        spec: &RouteSpec,
        input_rates: &[u32],
        bus_rate: u32,
        channels: u16,
    ) -> Result<Self, Error> {
        spec.validate()?;
        if channels == 0 || bus_rate == 0 || input_rates.contains(&0) {
            return Err(Error::InvalidRoute(
                "a route without devices needs channels and a rate".to_owned(),
            ));
        }
        if input_rates.len() != spec.inputs.len() {
            return Err(Error::InvalidRoute(format!(
                "{} input rates for {} inputs",
                input_rates.len(),
                spec.inputs.len()
            )));
        }
        let rate = bus_rate;
        let bus_rates = vec![rate; spec.buses.len()];
        let mut bus_inputs: Vec<Vec<MixerInput>> = spec.buses.iter().map(|_| Vec::new()).collect();
        let mut rings = Rings {
            bus_inputs: &mut bus_inputs,
            bus_rates: &bus_rates,
            latency_ms: spec.latency_ms,
            automix: automix_group(spec),
            automix_next: 0,
        };
        let mut captures = Vec::with_capacity(spec.inputs.len());
        let mut inputs = Vec::with_capacity(spec.inputs.len());
        for (input, &input_rate) in spec.inputs.iter().zip(input_rates) {
            let (capture, mut running) = make_rings(
                input,
                input.device.clone(),
                channels,
                input_rate,
                &mut rings,
            );
            running.present = true;
            captures.push(capture);
            inputs.push(running);
        }
        let channels = usize::from(channels);
        let mut mixers = Vec::with_capacity(spec.buses.len());
        let mut buses = Vec::with_capacity(spec.buses.len());
        let mut meters = Vec::with_capacity(spec.buses.len());
        for (bus, mixer_inputs) in spec.buses.iter().zip(bus_inputs) {
            let (mut mixer, handle) = Mixer::new(mixer_inputs, channels);
            let loudness = match pult_core::loudness(channels, rate, 2.0) {
                Ok((tap, meter, stats)) => {
                    mixer.set_loudness(tap);
                    meters.push(meter);
                    Some(stats)
                }
                Err(_) => None,
            };
            set_up_bus(&handle, bus);
            // What `start` and the slot of the bus do before its first callback.
            mixer.suspend();
            mixer.resume(channels);
            mixers.push(mixer);
            buses.push(crate::RunningBus {
                name: bus.name.clone(),
                device: bus.device.clone(),
                handle,
                sample_rate: rate,
                present: true,
                outages: 0,
                loudness,
            });
        }
        let running = Running {
            host: None,
            slots: Vec::new(),
            inputs,
            buses,
            xruns: Arc::new(Counter::default()),
            spec: spec.clone(),
            _loudness: None,
            alarm: crate::alarm::Alarm::silent(),
        };
        running.apply_monitors();
        let outputs = spec.buses.iter().map(|_| Vec::new()).collect();
        let suspended = vec![false; spec.buses.len()];
        Ok(Self {
            running,
            inputs_fed: captures,
            meters,
            mixers,
            outputs,
            channels,
            suspended,
        })
    }

    /// The route, to change it with [`Running::apply`] or [`Running::scene`] between two calls of
    /// [`Self::process`], as the control thread does between two callbacks.
    pub fn running(&mut self) -> &mut Running {
        &mut self.running
    }

    /// The device of `bus` is gone: its mixer closes its rings (the captures discard into them)
    /// and is not called any more, the bus plays silence, as [`crate::start`] does with a lost
    /// output until it comes back.
    ///
    /// # Panics
    ///
    /// When there is no bus `bus`.
    pub fn suspend(&mut self, bus: usize) {
        self.mixers[bus].suspend();
        self.suspended[bus] = true;
        self.running.buses[bus].present = false;
    }

    /// The device of `bus` is back with the channels of [`Self::new`]: its next callback empties
    /// the routed rings, primes them and fades them in.
    ///
    /// # Panics
    ///
    /// When there is no bus `bus`.
    pub fn resume(&mut self, bus: usize) {
        self.mixers[bus].resume(self.channels);
        self.suspended[bus] = false;
        self.running.buses[bus].present = true;
    }

    /// One callback of every device: passes `inputs[i]` (interleaved, the channels of
    /// [`Self::new`], any number of frames at the input's own rate, none for one that delivers
    /// nothing) through the stage of input `i` into every ring of it, then lets every bus mix
    /// `frames` frames; a suspended bus puts out silence.
    /// Returns what the buses put out, one interleaved buffer each.
    ///
    /// # Panics
    ///
    /// When `inputs` does not have one buffer per input.
    pub fn process(&mut self, inputs: &[&[f32]], frames: usize) -> &[Vec<f32>] {
        assert_eq!(inputs.len(), self.inputs_fed.len(), "one buffer per input");
        for (InputFeed { stage, captures }, data) in self.inputs_fed.iter_mut().zip(inputs) {
            stage.feed_captures(data, captures);
        }
        for ((mixer, out), &gone) in self
            .mixers
            .iter_mut()
            .zip(&mut self.outputs)
            .zip(&self.suspended)
        {
            out.resize(frames * self.channels, 0.0);
            if gone {
                out.fill(0.0);
            } else {
                mixer.process(out);
            }
        }
        for meter in &mut self.meters {
            meter.drain();
        }
        &self.outputs
    }
}
