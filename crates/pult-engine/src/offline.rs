//! A route without devices: the same rings, mixers and controls as [`crate::start`] builds, fed
//! and drained by the caller instead of by audio callbacks. Profiles, scenes and commands work on
//! it through [`Offline::running`] exactly as on a route that plays, so what the buses put out can
//! be checked sample by sample (the click test of `tests/clicks.rs` does).

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
    /// Per bus, its mixer, as its device callback would run it.
    mixers: Vec<Mixer>,
    /// Per bus, the output of the latest [`Self::process`].
    outputs: Vec<Vec<f32>>,
    channels: usize,
}

impl Offline {
    /// Builds the route of `spec` as if every input delivered `channels` channels at `rate` Hz and
    /// every bus played them at that rate, and opens the buses as their first callback would.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRoute`] when the spec does not hold together, or `channels` is zero.
    pub fn new(spec: &RouteSpec, rate: u32, channels: u16) -> Result<Self, Error> {
        spec.validate()?;
        if channels == 0 || rate == 0 {
            return Err(Error::InvalidRoute(
                "a route without devices needs channels and a rate".to_owned(),
            ));
        }
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
        for input in &spec.inputs {
            let (capture, mut running) =
                make_rings(input, input.device.clone(), channels, rate, &mut rings);
            running.present = true;
            captures.push(capture);
            inputs.push(running);
        }
        let channels = usize::from(channels);
        let mut mixers = Vec::with_capacity(spec.buses.len());
        let mut buses = Vec::with_capacity(spec.buses.len());
        for (bus, mixer_inputs) in spec.buses.iter().zip(bus_inputs) {
            let (mut mixer, handle) = Mixer::new(mixer_inputs, channels);
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
            });
        }
        let running = Running {
            host: None,
            slots: Vec::new(),
            inputs,
            buses,
            xruns: Arc::new(Counter::default()),
            spec: spec.clone(),
        };
        let outputs = spec.buses.iter().map(|_| Vec::new()).collect();
        Ok(Self {
            running,
            inputs_fed: captures,
            mixers,
            outputs,
            channels,
        })
    }

    /// The route, to change it with [`Running::apply`] or [`Running::scene`] between two calls of
    /// [`Self::process`], as the control thread does between two callbacks.
    pub fn running(&mut self) -> &mut Running {
        &mut self.running
    }

    /// One callback of every device: passes `inputs[i]` (interleaved, the channels of
    /// [`Self::new`]) through the stage of input `i` into every ring of it, then lets every bus
    /// mix `frames` frames.
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
        for (mixer, out) in self.mixers.iter_mut().zip(&mut self.outputs) {
            out.resize(frames * self.channels, 0.0);
            mixer.process(out);
        }
        &self.outputs
    }
}
