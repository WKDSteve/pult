//! Inputs, their rings and the mixer that sums them into one output.
//!
//! Each input device gets a [`Capture`] (its callback pushes into a ring) and a
//! [`MixerInput`] (the output callback reads from that ring). A [`Mixer`] owns the inputs of
//! one bus and fills that bus's output buffer. An input that feeds several buses has one
//! ring per bus, each with its own drift control (every output runs on its own clock), and
//! shares one set of [`InputControls`] across them. Control and statistics go through an
//! [`InputHandle`] and a [`BusHandle`] the control thread keeps.
//!
//! While a bus's output device is gone, [`Mixer::suspend`] closes its rings: the captures
//! discard into them instead of counting a full ring as lost frames. [`Mixer::resume`]
//! empties and reopens them for the device's return.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};

use crate::drift::{DriftControl, Event};
use crate::level::{AtomicF32, Counter, PeakMeter};
use crate::remix::remix_add;

/// Gain, mute and pan of one input, set by the control thread.
#[derive(Debug)]
pub struct InputControls {
    /// Linear gain factor.
    pub gain: AtomicF32,
    /// Muted inputs keep draining their ring but add nothing.
    pub muted: AtomicBool,
    /// -1 (left) to 1 (right); see [`remix_add`].
    pub pan: AtomicF32,
}

impl Default for InputControls {
    fn default() -> Self {
        Self {
            gain: AtomicF32::new(1.0),
            muted: AtomicBool::new(false),
            pan: AtomicF32::new(0.0),
        }
    }
}

/// What happened to one ring of an input, counted by the audio threads.
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
    /// Frames skipped once at the end of a settling phase.
    pub aligned: Counter,
}

/// Gain and mute of one bus, set by the control thread.
#[derive(Debug)]
pub struct BusControls {
    /// Linear gain factor applied to the sum.
    pub gain: AtomicF32,
    /// A muted bus plays silence; its inputs keep draining.
    pub muted: AtomicBool,
}

impl Default for BusControls {
    fn default() -> Self {
        Self {
            gain: AtomicF32::new(1.0),
            muted: AtomicBool::new(false),
        }
    }
}

/// Statistics of one bus's output.
#[derive(Debug, Default)]
pub struct BusStats {
    /// Peak of the output after the bus gain.
    pub peak: PeakMeter,
    /// Output samples beyond full scale (clipped to it) or invalid (replaced by silence).
    pub clipped: Counter,
}

/// The control thread's side of one input ring.
#[derive(Debug, Clone)]
pub struct InputHandle {
    /// Gain, mute and pan, shared by all rings of the input.
    pub controls: Arc<InputControls>,
    /// Peaks and counters of this ring.
    pub stats: Arc<InputStats>,
}

/// The control thread's side of one bus.
#[derive(Debug, Clone)]
pub struct BusHandle {
    /// Gain and mute.
    pub controls: Arc<BusControls>,
    /// Peak and clip counter.
    pub stats: Arc<BusStats>,
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
    /// Frames after each priming in which the drift control measures the start-up offset
    /// instead of correcting (0: none).
    pub settle: usize,
}

/// Creates the ring of one input with its own controls and both of its ends.
///
/// # Panics
///
/// When `channels` is zero.
#[must_use]
pub fn connect(config: InputConfig) -> (Capture, MixerInput, InputHandle) {
    connect_with(config, Arc::new(InputControls::default()))
}

/// Creates one more ring of an input whose `controls` other rings already share.
///
/// # Panics
///
/// When `channels` is zero.
#[must_use]
pub fn connect_with(
    config: InputConfig,
    controls: Arc<InputControls>,
) -> (Capture, MixerInput, InputHandle) {
    assert!(config.channels > 0, "an input needs at least one channel");
    let samples = config.capacity * config.channels;
    let (producer, consumer) = RingBuffer::new(samples);
    let open = Arc::new(AtomicBool::new(true));
    let handle = InputHandle {
        controls,
        stats: Arc::new(InputStats::default()),
    };
    let capture = Capture {
        producer,
        channels: config.channels,
        open: Arc::clone(&open),
        stats: Arc::clone(&handle.stats),
    };
    let input = MixerInput {
        consumer,
        channels: config.channels,
        open,
        drift: DriftControl::new(config.target, config.band)
            .smoothed(config.smoothing)
            .settling(config.settle),
        scratch: vec![0.0; samples],
        controls: Arc::clone(&handle.controls),
        stats: Arc::clone(&handle.stats),
    };
    (capture, input, handle)
}

/// The capture end of an input ring: its device callback pushes here.
#[derive(Debug)]
pub struct Capture {
    producer: Producer<f32>,
    channels: usize,
    /// Closed while the ring's bus has no device; then `push` discards.
    open: Arc<AtomicBool>,
    stats: Arc<InputStats>,
}

impl Capture {
    /// Channels per frame this ring was made for.
    #[must_use]
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Stores the whole frames of `data` that fit; the rest is counted as dropped. A closed
    /// ring discards everything without counting.
    pub fn push(&mut self, data: &[f32]) {
        if !self.open.load(Ordering::Acquire) {
            return;
        }
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

/// The playback end of an input ring, owned by the mixer of one bus.
#[derive(Debug)]
pub struct MixerInput {
    consumer: Consumer<f32>,
    channels: usize,
    open: Arc<AtomicBool>,
    drift: DriftControl,
    scratch: Vec<f32>,
    controls: Arc<InputControls>,
    stats: Arc<InputStats>,
}

impl MixerInput {
    /// Empties the ring and lets the drift control prime again.
    fn restart(&mut self) {
        let stale = self.consumer.slots();
        if let Ok(chunk) = self.consumer.read_chunk(stale) {
            chunk.commit_all();
        }
        self.drift.reset();
    }

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
            Event::Aligned => self.stats.aligned.add(plan.skip as u64),
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
        let position = self.controls.pan.load();
        remix_add(block, ch, out, out_channels, gain, position);
    }
}

/// Sums the inputs of one bus into its output buffer; runs inside the output device's
/// callback.
#[derive(Debug)]
pub struct Mixer {
    inputs: Vec<MixerInput>,
    out_channels: usize,
    controls: Arc<BusControls>,
    stats: Arc<BusStats>,
}

impl Mixer {
    /// A mixer for an output with `out_channels` channels.
    #[must_use]
    pub fn new(inputs: Vec<MixerInput>, out_channels: usize) -> (Self, BusHandle) {
        let handle = BusHandle {
            controls: Arc::new(BusControls::default()),
            stats: Arc::new(BusStats::default()),
        };
        let mixer = Self {
            inputs,
            out_channels,
            controls: Arc::clone(&handle.controls),
            stats: Arc::clone(&handle.stats),
        };
        (mixer, handle)
    }

    /// Closes the rings of all inputs: their captures discard until [`Self::resume`]. For
    /// a bus whose output device is gone and whose mixer therefore no longer drains them.
    pub fn suspend(&mut self) {
        for input in &self.inputs {
            input.open.store(false, Ordering::Release);
        }
    }

    /// Empties the rings, lets every input prime again and reopens them, for an output with
    /// `out_channels` channels (a device may come back with another layout).
    pub fn resume(&mut self, out_channels: usize) {
        self.out_channels = out_channels;
        for input in &mut self.inputs {
            input.restart();
            input.open.store(true, Ordering::Release);
        }
    }

    /// Overwrites `out` (interleaved) with the mix of all inputs times the bus gain, clipped
    /// to full scale. A NaN (a broken input) becomes silence: a device may turn it into
    /// full-scale noise.
    pub fn process(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        if self.out_channels == 0 {
            return;
        }
        let frames = out.len() / self.out_channels;
        for input in &mut self.inputs {
            input.mix_into(out, self.out_channels, frames);
        }
        let gain = if self.controls.muted.load(Ordering::Relaxed) {
            0.0
        } else {
            self.controls.gain.load()
        };
        let mut clipped = 0;
        for sample in out.iter_mut() {
            let value = *sample * gain;
            if value.is_nan() {
                clipped += 1;
                *sample = 0.0;
            } else if value.abs() > 1.0 {
                clipped += 1;
                *sample = value.clamp(-1.0, 1.0);
            } else {
                *sample = value;
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
            settle: 0,
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
    fn one_input_feeds_two_buses_with_shared_controls() {
        let controls = Arc::new(InputControls::default());
        let (mut to_a, ring_a, handle_a) = connect_with(config(1, 0), Arc::clone(&controls));
        let (mut to_b, ring_b, handle_b) = connect_with(config(1, 0), Arc::clone(&controls));
        let (mut bus_a, _) = Mixer::new(vec![ring_a], 2);
        let (mut bus_b, bus_b_handle) = Mixer::new(vec![ring_b], 2);
        controls.pan.store(1.0);
        bus_b_handle.controls.gain.store(0.5);
        for capture in [&mut to_a, &mut to_b] {
            capture.push(&[0.8]);
        }
        let (mut out_a, mut out_b) = ([0.0; 2], [0.0; 2]);
        bus_a.process(&mut out_a);
        bus_b.process(&mut out_b);
        assert_eq!(out_a, [0.0, 0.8]);
        assert_eq!(out_b, [0.0, 0.4]);
        assert!(Arc::ptr_eq(&handle_a.controls, &handle_b.controls));
        assert!(!Arc::ptr_eq(&handle_a.stats, &handle_b.stats));
        assert_eq!(bus_b_handle.stats.peak.take(), 0.4);
    }

    #[test]
    fn a_muted_bus_is_silent_but_drains_its_inputs() {
        let (mut capture, input, handle) = connect(config(1, 0));
        let (mut mixer, bus) = Mixer::new(vec![input], 1);
        bus.controls.muted.store(true, Ordering::Relaxed);
        capture.push(&[0.5, 0.5]);
        let mut out = [9.0; 2];
        mixer.process(&mut out);
        assert_eq!(out, [0.0; 2]);
        assert_eq!(handle.stats.peak.take(), 0.5);
        bus.controls.muted.store(false, Ordering::Relaxed);
        mixer.process(&mut out);
        assert_eq!(handle.stats.underruns.get(), 1);
    }

    #[test]
    fn a_suspended_bus_discards_without_counting_and_resumes_fresh() {
        let (mut capture, input, handle) = connect(config(1, 1));
        let (mut mixer, _) = Mixer::new(vec![input], 1);
        capture.push(&[0.1, 0.2, 0.3]);
        mixer.suspend();
        capture.push(&[0.9; 100]);
        assert_eq!(handle.stats.dropped_at_capture.get(), 0);

        mixer.resume(2);
        // The stale frames are gone: priming needs target + needed frames again.
        let mut out = [9.0; 2];
        mixer.process(&mut out);
        assert_eq!(out, [0.0; 2]);
        capture.push(&[0.5, 0.6]);
        mixer.process(&mut out);
        assert_eq!(out, [0.5, 0.5]);
        assert_eq!(capture.channels(), 1);
    }

    #[test]
    fn counts_underruns_and_full_rings() {
        let (mut capture, input, handle) = connect(InputConfig {
            channels: 1,
            capacity: 4,
            target: 0,
            band: 10,
            smoothing: 1.0,
            settle: 0,
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
            settle: 0,
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
    fn never_passes_nan_or_infinity_to_the_device() {
        let (mut capture, input, _) = connect(config(1, 0));
        let (mut mixer, bus) = Mixer::new(vec![input], 1);
        capture.push(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5]);
        let mut out = [0.0; 4];
        mixer.process(&mut out);
        assert_eq!(out, [0.0, 1.0, -1.0, 0.5]);
        assert_eq!(bus.stats.clipped.get(), 3);
    }

    #[test]
    fn clips_the_mix_to_full_scale() {
        let (mut capture, input, _) = connect(config(1, 0));
        let (mut mixer, bus) = Mixer::new(vec![input], 1);
        capture.push(&[1.5, -2.0]);
        let mut out = [0.0; 2];
        mixer.process(&mut out);
        assert_eq!(out, [1.0, -1.0]);
        assert_eq!(bus.stats.clipped.get(), 2);
        assert_eq!(bus.stats.peak.take(), 1.0);
    }
}
