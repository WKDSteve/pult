//! The delay of a bus: holds what leaves it back, against the offset between two outputs that
//! play the same mix (speakers and headphones, a stream and the room).
//!
//! It sits behind the limiter, after everything that changes the sound, so the meters and the
//! loudness see what the device plays. Its line holds [`MAX_BUS_DELAY_MS`] and is made with the
//! bus (nothing is allocated in the callback); it is always written, so a delay that is set
//! plays what really came before. A new delay does not jump: it crossfades over [`FADE_MS`]
//! from the old position in the line to the new one, as the limiter does with its lookahead.

use crate::level::{AtomicF32, within};

/// The longest delay of a bus in milliseconds: the same as an input's.
pub const MAX_BUS_DELAY_MS: f32 = crate::strip::MAX_DELAY_MS;

/// Milliseconds of the crossfade from one delay to another.
pub const FADE_MS: f32 = 5.0;

/// Whether `ms` is a delay a bus takes: 0 to [`MAX_BUS_DELAY_MS`].
///
/// # Errors
///
/// A message naming the range otherwise.
pub fn check_delay(ms: f32) -> Result<f32, String> {
    if (0.0..=MAX_BUS_DELAY_MS).contains(&ms) {
        Ok(ms)
    } else {
        Err(format!(
            "the delay of a bus must be 0 to {MAX_BUS_DELAY_MS} ms, not {ms}"
        ))
    }
}

/// The delay of a bus as the control thread sets it, in milliseconds; 0 at first.
#[derive(Debug, Default)]
pub struct BusDelayControls {
    ms: AtomicF32,
}

impl BusDelayControls {
    /// Sets it, clamped to its range; a NaN becomes 0.
    pub fn set(&self, ms: f32) {
        self.ms.store(within(ms, (0.0, MAX_BUS_DELAY_MS), 0.0));
    }

    /// As set.
    #[must_use]
    pub fn ms(&self) -> f32 {
        self.ms.load()
    }
}

/// The delay line of one bus, in its output callback.
#[derive(Debug)]
pub struct BusDelay {
    rate: f32,
    channels: usize,
    /// Frames the line holds.
    frames: usize,
    line: Vec<f32>,
    /// The frame written next.
    at: usize,
    /// The delay played now, in frames.
    behind: usize,
    /// While a crossfade lasts: the delay it comes from, and frames of it left.
    from: Option<(usize, usize)>,
    fade_frames: usize,
}

impl BusDelay {
    /// The delay of a bus of `channels` at `rate`.
    #[must_use]
    pub fn new(rate: u32, channels: usize) -> Self {
        #[allow(clippy::cast_precision_loss)]
        let rate = rate.max(1) as f32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let frames = (MAX_BUS_DELAY_MS / 1000.0 * rate).ceil() as usize + 1;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let fade_frames = ((FADE_MS / 1000.0 * rate) as usize).max(1);
        let channels = channels.max(1);
        Self {
            rate,
            channels,
            frames,
            line: vec![0.0; frames * channels],
            at: 0,
            behind: 0,
            from: None,
            fade_frames,
        }
    }

    /// Gets ready for an output of `channels` (a device may come back with another layout) and
    /// forgets what the line held. Allocates only when the channels changed, which happens
    /// outside the callback.
    pub fn prepare(&mut self, channels: usize) {
        let channels = channels.max(1);
        if channels == self.channels {
            self.line.fill(0.0);
        } else {
            self.channels = channels;
            self.line = vec![0.0; self.frames * channels];
        }
        self.at = 0;
        self.from = None;
    }

    /// The delay `controls` ask for, in frames of this bus.
    fn wanted(&self, controls: &BusDelayControls) -> usize {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let frames = (controls.ms() / 1000.0 * self.rate).round() as usize;
        frames.min(self.frames - 1)
    }

    /// Delays `data` (interleaved, `channels` per frame) in place as `controls` say.
    pub fn process(&mut self, controls: &BusDelayControls, data: &mut [f32], channels: usize) {
        if channels != self.channels {
            // Not prepared for this layout: pass it on rather than mix up the channels.
            return;
        }
        let wanted = self.wanted(controls);
        if wanted != self.behind {
            // From where it plays now: the old delay, or the mix of a crossfade cut short
            // (taken as the delay it was going to).
            self.from = Some((self.behind, self.fade_frames));
            self.behind = wanted;
        }
        if self.behind == 0 && self.from.is_none() {
            // Nothing to hold back; the line is still written for a delay to come.
            for frame in data.chunks_exact(channels) {
                let write = self.at * channels;
                self.line[write..write + channels].copy_from_slice(frame);
                self.at = (self.at + 1) % self.frames;
            }
            return;
        }
        for frame in data.chunks_exact_mut(channels) {
            let write = self.at * channels;
            self.line[write..write + channels].copy_from_slice(frame);
            let read = |behind: usize| (self.at + self.frames - behind) % self.frames * channels;
            let new = read(self.behind);
            match &mut self.from {
                Some((old, left)) => {
                    #[allow(clippy::cast_precision_loss)]
                    let weight = 1.0 - *left as f32 / self.fade_frames as f32;
                    let old = read(*old);
                    for (c, sample) in frame.iter_mut().enumerate() {
                        *sample = self.line[old + c] * (1.0 - weight) + self.line[new + c] * weight;
                    }
                    *left -= 1;
                    if *left == 0 {
                        self.from = None;
                    }
                }
                None => frame.copy_from_slice(&self.line[new..new + channels]),
            }
            self.at = (self.at + 1) % self.frames;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ramp 1, 2, 3, ... on every channel, so each sample says which frame it was.
    fn ramp(frames: usize, channels: usize, first: usize) -> Vec<f32> {
        #[allow(clippy::cast_precision_loss)]
        (first..first + frames)
            .flat_map(|n| std::iter::repeat_n(n as f32, channels))
            .collect()
    }

    #[test]
    fn it_holds_the_sound_back_by_the_frames_asked() {
        let controls = BusDelayControls::default();
        let mut delay = BusDelay::new(48_000, 2);
        let mut first = ramp(480, 2, 1);
        delay.process(&controls, &mut first, 2);
        assert_eq!(first, ramp(480, 2, 1), "no delay: as it comes");
        // 10 ms at 48 kHz: 480 frames, after a crossfade of 5 ms from the undelayed sound.
        controls.set(10.0);
        let mut out = Vec::new();
        for callback in 1..4 {
            let mut data = ramp(480, 2, 1 + callback * 480);
            delay.process(&controls, &mut data, 2);
            out.extend(data);
        }
        // The last callback started at frame 1 + 3 * 480 and plays 480 frames before it.
        let last = &out[out.len() - 2 * 480..];
        assert!(
            last == ramp(480, 2, 1 + 2 * 480).as_slice(),
            "starts at {}",
            last[0]
        );
    }

    #[test]
    fn a_new_delay_crossfades_instead_of_jumping() {
        let controls = BusDelayControls::default();
        let mut delay = BusDelay::new(48_000, 1);
        let step = std::f32::consts::TAU * 100.0 / 48_000.0;
        let mut phase = 0.0_f32;
        let mut previous = 0.0_f32;
        let mut steepest = 0.0_f32;
        for callback in 0..60 {
            if callback == 20 {
                controls.set(7.3);
            }
            if callback == 40 {
                controls.set(0.0);
            }
            let mut data: Vec<f32> = (0..480)
                .map(|_| {
                    phase += step;
                    0.5 * phase.sin()
                })
                .collect();
            delay.process(&controls, &mut data, 1);
            for sample in data {
                steepest = steepest.max((sample - previous).abs());
                previous = sample;
            }
        }
        // The tone's own steepest step is 0.5 * step (0.0065); a jump would be up to 1.
        assert!(steepest < 0.05, "a step of {steepest}");
    }

    #[test]
    fn the_delay_is_clamped_and_checked() {
        let controls = BusDelayControls::default();
        controls.set(10_000.0);
        assert_eq!(controls.ms(), MAX_BUS_DELAY_MS);
        controls.set(f32::NAN);
        assert_eq!(controls.ms(), 0.0);
        assert!(check_delay(12.0).is_ok());
        assert!(check_delay(-1.0).is_err());
        assert!(check_delay(MAX_BUS_DELAY_MS + 1.0).is_err());
    }

    #[test]
    fn delaying_does_not_allocate() {
        let controls = BusDelayControls::default();
        let mut delay = BusDelay::new(48_000, 2);
        let mut data = vec![0.25_f32; 960];
        assert_no_alloc::assert_no_alloc(|| {
            delay.process(&controls, &mut data, 2);
            controls.set(20.0);
            delay.process(&controls, &mut data, 2);
        });
    }
}
