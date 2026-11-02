//! Inputs, their rings and the mixer that sums them into one output.
//!
//! Each input device gets a [`Capture`] (its callback pushes into a ring) and a
//! [`MixerInput`] (the output callback reads from that ring). The [`Mixer`] owns the inputs
//! and fills the output buffer. Control and statistics go through an [`InputHandle`] the
//! control thread keeps.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};

use crate::drift::{DriftControl, Event};
use crate::level::{AtomicF32, Counter, PeakMeter};
use crate::remix::remix_add;

/// Gain and mute of one input, set by the control thread.
#[derive(Debug)]
pub struct InputControls {
    /// Linear gain factor.
    pub gain: AtomicF32,
    /// Muted inputs keep draining their ring but add nothing.
    pub muted: AtomicBool,
}

impl Default for InputControls {
    fn default() -> Self {
        Self {
            gain: AtomicF32::new(1.0),
            muted: AtomicBool::new(false),
        }
    }
}

/// What happened to one input, counted by the audio threads.
#[derive(Debug, Default)]
pub struct InputStats {
    /// Peak after gain, as fed into the mix.
    pub peak: PeakMeter,
    /// Frames the capture could not store because the ring was full.
    pub dropped_at_capture: Counter,
    /// Times the ring ran dry during playback.
    pub underruns: Counter,
    /// Hard skips back to the target level.
    pub resyncs: Counter,
    /// Single frames dropped because the level ran above the band.
    pub drops: Counter,
    /// Single frames repeated because the level ran below the band.
    pub repeats: Counter,
}

/// Statistics of the mixed output.
#[derive(Debug, Default)]
pub struct MasterStats {
    /// Peak of the mix.
    pub peak: PeakMeter,
    /// Output samples beyond full scale, clipped to it.
    pub clipped: Counter,
}

/// The control thread's side of one input.
#[derive(Debug, Clone)]
pub struct InputHandle {
    /// Gain and mute.
    pub controls: Arc<InputControls>,
    /// Peaks and counters.
    pub stats: Arc<InputStats>,
}

/// Layout and buffering of one input, in frames.
#[derive(Debug, Clone, Copy)]
pub struct InputConfig {
    /// Channels per frame of the input device.
    pub channels: usize,
    /// Ring size; also the largest output callback the input can serve.
    pub capacity: usize,
    /// Frames the control keeps buffered: the added latency.
    pub target: usize,
    /// Tolerance around the target before a correction.
    pub band: usize,
    /// Weight of each callback in the level average of the drift control (1.0: none).
    pub smoothing: f64,
}

/// Creates the ring of one input and both of its ends.
///
/// # Panics
///
/// When `channels` is zero.
#[must_use]
pub fn connect(config: InputConfig) -> (Capture, MixerInput, InputHandle) {
    assert!(config.channels > 0, "an input needs at least one channel");
    let samples = config.capacity * config.channels;
    let (producer, consumer) = RingBuffer::new(samples);
    let handle = InputHandle {
        controls: Arc::new(InputControls::default()),
        stats: Arc::new(InputStats::default()),
    };
    let capture = Capture {
        producer,
        channels: config.channels,
        stats: Arc::clone(&handle.stats),
    };
    let input = MixerInput {
        consumer,
        channels: config.channels,
        drift: DriftControl::new(config.target, config.band).smoothed(config.smoothing),
        scratch: vec![0.0; samples],
        controls: Arc::clone(&handle.controls),
        stats: Arc::clone(&handle.stats),
    };
    (capture, input, handle)
}

/// The capture end of an input: its device callback pushes here.
#[derive(Debug)]
pub struct Capture {
    producer: Producer<f32>,
    channels: usize,
    stats: Arc<InputStats>,
}

impl Capture {
    /// Stores the whole frames of `data` that fit; the rest is counted as dropped.
    pub fn push(&mut self, data: &[f32]) {
        let frames = data.len() / self.channels;
        let stored = frames.min(self.producer.slots() / self.channels);
        if stored < frames {
            self.stats.dropped_at_capture.add((frames - stored) as u64);
        }
        let samples = stored * self.channels;
        if let Ok(chunk) = self.producer.write_chunk_uninit(samples) {
            chunk.fill_from_iter(data[..samples].iter().copied());
        }
    }
}

/// The playback end of an input, owned by the mixer.
#[derive(Debug)]
pub struct MixerInput {
    consumer: Consumer<f32>,
    channels: usize,
    drift: DriftControl,
    scratch: Vec<f32>,
    controls: Arc<InputControls>,
    stats: Arc<InputStats>,
}

impl MixerInput {
    /// Reads `frames` frames per the drift plan and adds them into `out`.
    fn mix_into(&mut self, out: &mut [f32], out_channels: usize, frames: usize) {
        let ch = self.channels;
        let frames = frames.min(self.scratch.len() / ch);
        let available = self.consumer.slots() / ch;
        let (plan, event) = self.drift.plan(available, frames);
        match event {
            Event::Underrun => self.stats.underruns.add(1),
            Event::Resync => self.stats.resyncs.add(1),
            Event::Dropped => self.stats.drops.add(1),
            Event::Repeated => self.stats.repeats.add(1),
            Event::None | Event::Priming => {}
        }
        if let Ok(chunk) = self.consumer.read_chunk(plan.skip * ch) {
            chunk.commit_all();
        }
        let read = plan.read * ch;
        if let Ok(chunk) = self.consumer.read_chunk(read) {
            let (first, second) = chunk.as_slices();
            self.scratch[..first.len()].copy_from_slice(first);
            self.scratch[first.len()..read].copy_from_slice(second);
            chunk.commit_all();
        }
        let mut filled = read;
        for _ in 0..plan.repeat {
            if filled >= ch {
                self.scratch.copy_within(filled - ch..filled, filled);
            } else {
                self.scratch[filled..filled + ch].fill(0.0);
            }
            filled += ch;
        }
        let end = frames * ch;
        self.scratch[filled..end].fill(0.0);

        let gain = if self.controls.muted.load(Ordering::Relaxed) {
            0.0
        } else {
            self.controls.gain.load()
        };
        let block = &self.scratch[..end];
        self.stats.peak.record_scaled(block, gain);
        remix_add(block, ch, out, out_channels, gain);
    }
}

/// Sums all inputs into the output buffer; runs inside the output device's callback.
#[derive(Debug)]
pub struct Mixer {
    inputs: Vec<MixerInput>,
    out_channels: usize,
    stats: Arc<MasterStats>,
}

impl Mixer {
    /// A mixer for an output with `out_channels` channels.
    #[must_use]
    pub fn new(inputs: Vec<MixerInput>, out_channels: usize) -> (Self, Arc<MasterStats>) {
        let stats = Arc::new(MasterStats::default());
        let mixer = Self {
            inputs,
            out_channels,
            stats: Arc::clone(&stats),
        };
        (mixer, stats)
    }

    /// Overwrites `out` (interleaved) with the mix of all inputs, clipped to full scale.
    pub fn process(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        if self.out_channels == 0 {
            return;
        }
        let frames = out.len() / self.out_channels;
        for input in &mut self.inputs {
            input.mix_into(out, self.out_channels, frames);
        }
        let mut clipped = 0;
        for sample in out.iter_mut() {
            if sample.abs() > 1.0 {
                clipped += 1;
                *sample = sample.clamp(-1.0, 1.0);
            }
        }
        if clipped > 0 {
            self.stats.clipped.add(clipped);
        }
        self.stats.peak.record_scaled(out, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(channels: usize, target: usize) -> InputConfig {
        InputConfig {
            channels,
            capacity: 64,
            target,
            band: 2,
            smoothing: 1.0,
        }
    }

    #[test]
    fn plays_silence_until_primed_then_the_captured_frames() {
        let (mut capture, input, _handle) = connect(config(2, 2));
        let (mut mixer, _) = Mixer::new(vec![input], 2);
        let mut out = [9.0; 4];
        mixer.process(&mut out);
        assert_eq!(out, [0.0; 4]);

        capture.push(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]);
        mixer.process(&mut out);
        assert_eq!(out, [0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn sums_inputs_with_gain_and_mute() {
        let (mut a, input_a, handle_a) = connect(config(1, 0));
        let (mut b, input_b, handle_b) = connect(config(2, 0));
        let (mut mixer, _) = Mixer::new(vec![input_a, input_b], 2);
        handle_a.controls.gain.store(0.5);
        a.push(&[0.4, 0.4]);
        b.push(&[0.1, 0.2, 0.1, 0.2]);
        let mut out = [0.0; 4];
        mixer.process(&mut out);
        let expected = [0.3, 0.4, 0.3, 0.4];
        assert!(out.iter().zip(expected).all(|(o, e)| (o - e).abs() < 1e-6));
        assert!((handle_a.stats.peak.take() - 0.2).abs() < 1e-6);

        handle_b.controls.muted.store(true, Ordering::Relaxed);
        a.push(&[0.4, 0.4]);
        b.push(&[0.1, 0.2, 0.1, 0.2]);
        mixer.process(&mut out);
        assert!(out.iter().all(|o| (o - 0.2).abs() < 1e-6));
    }

    #[test]
    fn counts_underruns_and_full_rings() {
        let (mut capture, input, handle) = connect(InputConfig {
            channels: 1,
            capacity: 4,
            target: 0,
            band: 10,
            smoothing: 1.0,
        });
        capture.push(&[0.1; 6]);
        assert_eq!(handle.stats.dropped_at_capture.get(), 2);

        let (mut mixer, _) = Mixer::new(vec![input], 1);
        let mut out = [0.0; 3];
        mixer.process(&mut out);
        mixer.process(&mut out);
        assert_eq!(handle.stats.underruns.get(), 1);
        assert_eq!(out, [0.1, 0.0, 0.0]);
    }

    #[test]
    fn repeats_the_last_frame_when_running_low() {
        let (mut capture, input, handle) = connect(InputConfig {
            channels: 2,
            capacity: 64,
            target: 4,
            band: 1,
            smoothing: 1.0,
        });
        let (mut mixer, _) = Mixer::new(vec![input], 2);
        // Six frames: the first callback primes and reads two, the second finds the level
        // below the band and repeats one frame.
        let frames: Vec<f32> = (1..=12).map(|i| i as f32 / 100.0).collect();
        capture.push(&frames);
        let mut out = [0.0; 4];
        mixer.process(&mut out);
        mixer.process(&mut out);
        assert_eq!(out, [0.05, 0.06, 0.05, 0.06]);
        assert_eq!(handle.stats.repeats.get(), 1);
        assert_eq!(handle.stats.drops.get(), 0);
    }

    #[test]
    fn clips_the_mix_to_full_scale() {
        let (mut capture, input, _) = connect(config(1, 0));
        let (mut mixer, master) = Mixer::new(vec![input], 1);
        capture.push(&[1.5, -2.0]);
        let mut out = [0.0; 2];
        mixer.process(&mut out);
        assert_eq!(out, [1.0, -1.0]);
        assert_eq!(master.clipped.get(), 2);
        assert_eq!(master.peak.take(), 1.0);
    }
}
