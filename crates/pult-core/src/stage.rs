//! The input stage: what an input passes once, before its rings.
//!
//! An input that feeds several buses has one ring per bus ([`crate::mixer`]). What does not
//! depend on the bus runs here, once per input, in the thread that delivers its sound (the
//! device callback, or the thread of the process capture), at the rate of the input:
//!
//! 1. the PRE meter ([`InputControls::pre_peak`]): what arrives, before everything else, at
//!    the samples of the device (before any resampler, so an overshoot of its filter never
//!    shows as a clip of the source), and whether the input is routed anywhere or not;
//! 2. the gate, then the compressor ([`crate::dynamics`]);
//! 3. the level for the automixer, when the input is in a group ([`crate::automix`]);
//! 4. the result into every ring ([`Capture::push`]).
//!
//! Gain, pan, mute, solo and the matrix stay in the rings: they are per bus, or must glide
//! there. Room for what belongs here too, later: noise suppression and a sidechain (an
//! expensive effect does not belong in this thread; it would get its own).
//!
//! Nothing here allocates after [`InputStage::new`]: the sound goes through a buffer of
//! [`STAGE_FRAMES`] frames, a longer callback in several pieces.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::automix::PowerFollower;
use crate::dynamics::{Compressor, Gate};
use crate::mixer::{Capture, InputControls};

/// Frames the stage works on at once; a longer callback goes through in several pieces.
pub const STAGE_FRAMES: usize = 1024;

/// Gate and compressor of one input, and its PRE meter.
#[derive(Debug)]
pub struct InputStage {
    controls: Arc<InputControls>,
    channels: usize,
    gate: Gate,
    compressor: Compressor,
    follower: PowerFollower,
    block: Vec<f32>,
}

impl InputStage {
    /// The stage of an input of `channels` at `rate`, with the controls its rings share.
    ///
    /// # Panics
    ///
    /// When `channels` or `rate` is zero.
    #[must_use]
    pub fn new(controls: Arc<InputControls>, channels: usize, rate: u32) -> Self {
        assert!(channels > 0, "an input needs at least one channel");
        assert!(rate > 0, "an input needs a sample rate");
        Self {
            controls,
            channels,
            gate: Gate::new(rate),
            compressor: Compressor::new(rate),
            follower: PowerFollower::new(rate),
            block: vec![0.0; STAGE_FRAMES * channels],
        }
    }

    /// Channels per frame this stage was made for.
    #[must_use]
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Lets gate and compressor start from rest, as after a new stream of the input.
    pub fn start_over(&mut self) {
        self.gate.start_over();
        self.compressor.start_over();
        self.follower.start_over();
    }

    /// Meters the whole frames of `data`, gates and compresses them and hands them to `sink`,
    /// in pieces of at most [`STAGE_FRAMES`] frames. Samples after the last whole frame are
    /// dropped, as [`Capture::push`] does.
    pub fn feed(&mut self, data: &[f32], mut sink: impl FnMut(&[f32])) {
        let ch = self.channels;
        let whole = data.len() / ch * ch;
        self.controls.pre_peak.record_scaled(&data[..whole], 1.0);
        for piece in data[..whole].chunks(STAGE_FRAMES * ch) {
            let block = &mut self.block[..piece.len()];
            block.copy_from_slice(piece);
            self.gate.process(&self.controls.gate, block, ch);
            self.compressor
                .process(&self.controls.compressor, block, ch);
            self.controls
                .gate_reduction
                .record(self.gate.reduction_db());
            self.controls
                .comp_reduction
                .record(self.compressor.reduction_db());
            self.controls
                .gate_open
                .store(self.gate.is_open(), Ordering::Relaxed);
            if let Some((group, index)) = self.controls.automix.get() {
                group
                    .member(*index)
                    .store_power(self.follower.follow(block, ch));
            }
            sink(block);
        }
    }

    /// [`Self::feed`] into every ring of the input.
    pub fn feed_captures(&mut self, data: &[f32], captures: &mut [Capture]) {
        self.feed(data, |block| {
            for capture in captures.iter_mut() {
                capture.push(block);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamics::{CompressorParams, GateParams};
    use crate::level::{db_to_linear, linear_to_db};
    use crate::mixer::{InputConfig, Mixer, connect_with};

    const PERIOD: usize = 480;

    fn config(channels: usize) -> InputConfig {
        InputConfig {
            channels,
            input_rate: 48_000,
            output_rate: 48_000,
            capacity: 16_384,
            target: 0,
            band: 64,
            smoothing: 1.0,
            settle: 0,
        }
    }

    /// One mono input with its stage, feeding `buses` buses, each with its own mixer.
    struct Desk {
        controls: Arc<InputControls>,
        stage: InputStage,
        captures: Vec<Capture>,
        mixers: Vec<Mixer>,
        handles: Vec<crate::mixer::InputHandle>,
    }

    fn desk(buses: usize) -> Desk {
        let controls = Arc::new(InputControls::default());
        let stage = InputStage::new(Arc::clone(&controls), 1, 48_000);
        let (mut captures, mut mixers, mut handles) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..buses {
            let (capture, input, handle) = connect_with(config(1), Arc::clone(&controls));
            let (mixer, _) = Mixer::new(vec![input], 1);
            captures.push(capture);
            mixers.push(mixer);
            handles.push(handle);
        }
        Desk {
            controls,
            stage,
            captures,
            mixers,
            handles,
        }
    }

    impl Desk {
        /// `callbacks` periods of the constant `value` through the stage and every bus;
        /// the last output of each bus.
        fn run(&mut self, value: f32, callbacks: usize) -> Vec<Vec<f32>> {
            let data = vec![value; PERIOD];
            let mut outs = vec![vec![0.0; PERIOD]; self.mixers.len()];
            for _ in 0..callbacks {
                self.stage.feed_captures(&data, &mut self.captures);
                for (mixer, out) in self.mixers.iter_mut().zip(&mut outs) {
                    mixer.process(out);
                }
            }
            outs
        }
    }

    #[test]
    fn an_input_passes_its_compressor_before_its_fader_on_every_bus() {
        let mut d = desk(2);
        d.controls.compressor.set(CompressorParams {
            threshold_db: -26.0,
            ratio: 4.0,
            ..CompressorParams::default()
        });
        d.controls.compressor.switch(true);
        // The fader does not move what the compressor sees: half the gain, half of its output.
        d.controls.gain.store(0.5);
        let outs = d.run(0.5, 200);
        // -6 dB is 20 dB above the threshold; three quarters of that go.
        let compressed = 0.5 * db_to_linear(-0.75 * (linear_to_db(0.5) + 26.0));
        for out in &outs {
            let last = out[out.len() - 1];
            assert!((last - 0.5 * compressed).abs() < 2e-3, "{last}");
        }
        // The stage says how much the compressor takes: 15 dB (three quarters of 20).
        let taken = d.controls.comp_reduction.take();
        assert!((taken - 15.0).abs() < 0.5, "comp takes {taken} dB");
        // The ring meters read behind compressor and fader.
        for handle in &d.handles {
            handle.stats.peak.take();
        }
        d.run(0.5, 1);
        for handle in &d.handles {
            assert!((handle.stats.peak.take() - 0.5 * compressed).abs() < 2e-3);
        }
    }

    #[test]
    fn the_pre_meter_shows_what_arrives_before_the_gate() {
        let mut d = desk(1);
        d.controls.gate.set(GateParams {
            threshold_db: -20.0,
            hold_ms: 0.0,
            release_ms: 5.0,
            ..GateParams::default()
        });
        d.controls.gate.switch(true);
        let outs = d.run(0.01, 20);
        assert_eq!(outs[0][PERIOD - 1], 0.0, "the gate is closed");
        // After the fader nothing goes into the mix; before the gate the level is there.
        assert!(
            !d.controls.gate_open.load(Ordering::Relaxed),
            "the stage says it is shut"
        );
        assert!(
            d.controls.gate_reduction.take() > 40.0,
            "a shut gate takes a lot"
        );
        d.handles[0].stats.peak.take();
        d.controls.pre_peak.take();
        d.run(0.01, 2);
        assert_eq!(d.handles[0].stats.peak.take(), 0.0);
        assert!((d.controls.pre_peak.take() - 0.01).abs() < 1e-4);
    }

    #[test]
    fn the_pre_meter_reads_an_input_that_is_routed_nowhere() {
        let controls = Arc::new(InputControls::default());
        let mut stage = InputStage::new(Arc::clone(&controls), 2, 44_100);
        let (capture, input, handle) = connect_with(config(2), Arc::clone(&controls));
        let (mut mixer, _) = Mixer::new(vec![input.unrouted()], 2);
        let mut captures = vec![capture];
        let mut out = vec![0.0; PERIOD * 2];
        for _ in 0..5 {
            stage.feed_captures(&[0.3, -0.6].repeat(PERIOD), &mut captures);
            mixer.process(&mut out);
        }
        assert!(
            (controls.pre_peak.take() - 0.6).abs() < 1e-6,
            "the level to set it by"
        );
        assert_eq!(handle.stats.peak.take(), 0.0, "nothing goes into a mix");
    }

    #[test]
    fn a_gate_that_closed_keeps_a_ring_that_starts_silent() {
        let mut d = desk(1);
        d.controls.gate.set(GateParams {
            threshold_db: -20.0,
            ..GateParams::default()
        });
        d.controls.gate.switch(true);
        d.handles[0]
            .routed
            .store(false, std::sync::atomic::Ordering::Release);
        d.run(0.01, 2);
        // Routed with a level below the threshold: it never sounds, not even at its start,
        // since the gate stands before the ring and was closed already.
        d.handles[0]
            .routed
            .store(true, std::sync::atomic::Ordering::Release);
        let outs = d.run(0.01, 20);
        assert!(outs[0].iter().all(|&o| o.abs() < 1e-4));
        // Above it, it opens, and says so.
        let outs = d.run(0.5, 20);
        assert!((outs[0][PERIOD - 1] - 0.5).abs() < 2e-3);
        assert!(d.controls.gate_open.load(Ordering::Relaxed));
        d.controls.gate_reduction.take();
        d.run(0.5, 1);
        assert_eq!(
            d.controls.gate_reduction.take(),
            0.0,
            "an open gate takes nothing"
        );
    }

    #[test]
    fn a_long_callback_goes_through_in_pieces_and_a_partial_frame_is_dropped() {
        let controls = Arc::new(InputControls::default());
        let mut stage = InputStage::new(controls, 2, 48_000);
        let data: Vec<f32> = (0..=(STAGE_FRAMES * 2 + 300) * 2)
            .map(|i| i as f32)
            .collect();
        let mut seen = Vec::new();
        let mut pieces = 0;
        stage.feed(&data, |block| {
            pieces += 1;
            assert!(block.len() <= STAGE_FRAMES * 2);
            seen.extend_from_slice(block);
        });
        assert_eq!(pieces, 3);
        assert_eq!(
            seen,
            data[..data.len() - 1],
            "unchanged with gate and compressor off"
        );
    }

    #[test]
    fn the_stage_does_not_allocate() {
        let controls = Arc::new(InputControls::default());
        controls.gate.set(GateParams::simple(8.0));
        controls.gate.switch(true);
        controls.compressor.set(CompressorParams::simple(8.0));
        controls.compressor.switch(true);
        let group = Arc::new(crate::automix::AutomixGroup::new(2));
        controls.automix.set((Arc::clone(&group), 1)).unwrap();
        let mut stage = InputStage::new(Arc::clone(&controls), 2, 48_000);
        let (capture, _input, _handle) = connect_with(config(2), Arc::clone(&controls));
        let mut captures = vec![capture];
        let short = vec![0.2; PERIOD * 2];
        let long = vec![0.2; 5000 * 2];
        assert_no_alloc::assert_no_alloc(|| {
            for _ in 0..10 {
                stage.feed_captures(&short, &mut captures);
            }
            stage.feed_captures(&long, &mut captures);
            stage.start_over();
            controls.gate.switch(false);
            stage.feed_captures(&short, &mut captures);
        });
    }

    /// What gate and compressor cost for one stereo input on `buses` buses, per second of sound,
    /// in ms: the stage and the rings' mixing. A measurement, not a check; see
    /// `what_the_dsp_costs` in the mixer for how to run it.
    fn time_buses(buses: usize, dynamics: bool) -> f64 {
        let controls = Arc::new(InputControls::default());
        if dynamics {
            controls.gate.set_simple(5.0);
            controls.compressor.set_simple(8.0);
        }
        let mut stage = InputStage::new(Arc::clone(&controls), 2, 48_000);
        let (mut captures, mut mixers) = (Vec::new(), Vec::new());
        for _ in 0..buses {
            let (capture, input, _) = connect_with(
                InputConfig {
                    target: 960,
                    ..config(2)
                },
                Arc::clone(&controls),
            );
            captures.push(capture);
            mixers.push(Mixer::new(vec![input], 2).0);
        }
        let step = std::f32::consts::TAU * 440.0 / 48_000.0;
        let mut phase = 0.0_f32;
        let mut data = vec![0.0; PERIOD * 2];
        let mut out = vec![0.0; PERIOD * 2];
        let mut spent = std::time::Duration::ZERO;
        for _ in 0..1000 {
            for frame in data.as_chunks_mut::<2>().0 {
                let sample = 0.5 * phase.sin();
                *frame = [sample, sample];
                phase = (phase + step) % std::f32::consts::TAU;
            }
            let begun = std::time::Instant::now();
            stage.feed_captures(&data, &mut captures);
            for mixer in &mut mixers {
                mixer.process(&mut out);
            }
            spent += begun.elapsed();
        }
        spent.as_secs_f64() * 100.0
    }

    #[test]
    #[ignore = "a measurement, not a check"]
    fn what_the_stage_saves() {
        println!("per second of sound, one stereo input, gate and compressor once in its stage:");
        for buses in [1, 2, 4] {
            let plain = time_buses(buses, false);
            let dynamics = time_buses(buses, true);
            println!(
                "  {buses} bus(es): plain {plain:.2} ms, with gate and compressor {dynamics:.2} ms (+{:.2})",
                dynamics - plain
            );
        }
    }
}
