//! Clicks in a test tone: a step from one sample to the next that is steeper than the tone itself
//! can make at its level.
//!
//! A sine of amplitude `A` and frequency `f` at `rate` frames per second moves by at most
//! `A * 2 * sin(pi * f / rate)` from one sample to the next (the difference of two samples is a
//! sine of that amplitude). Anything a mixer does without a jump keeps close to that bound: a gain
//! ramp adds `A / frames of the ramp` per sample (at the shortest ramp of 5 ms at 48 kHz, a
//! quarter of the bound of a 100 Hz tone); a crossfade, an equalizer sliding or a limiter
//! turning down gently change the amplitude just as slowly. A jump in the gain, a skipped piece
//! of the signal or a delay changed at once moves the signal by a part of `A` within one sample:
//! at 100 Hz up to 76 times the bound.
//!
//! So [`Clicks`] takes the level `A` as the largest magnitude within one period of the tone around
//! each step (what comes after a jump counts, so a jump up from silence is measured against where
//! it lands), and flags a step beyond [`MARGIN`] times the bound, plus [`FLOOR`] against the
//! rounding noise of silence. Steps of real signal stay below the bound, the ramps add at most
//! about half of it more; [`MARGIN`] leaves room for both and still flags a jump in the gain by a
//! tenth of the level from about a sixth of a period away from a zero crossing on. A jump right at
//! a zero crossing cannot be told from the tone and is not heard as a click either.
//!
//! The test tone should be low: the bound grows with the frequency, and with it what passes.

use std::collections::VecDeque;
use std::f32::consts::PI;

/// How far a step may go beyond what the tone can do at its level, as a factor of that bound.
/// The click test's clean render of every change pult makes stays below 1.5 (`tests/clicks.rs`
/// in pult-engine says what it reached).
pub const MARGIN: f32 = 2.0;

/// A step this small is never a click, whatever the level: -80 dBFS, far below what one hears
/// against a tone and far above the rounding noise of the filters.
pub const FLOOR: f32 = 1e-4;

/// One step that is too steep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Click {
    /// The frame the step lands on.
    pub frame: usize,
    /// The channel it is in.
    pub channel: usize,
    /// How far the sample moved from the frame before.
    pub step: f32,
    /// How far it could have moved at the level there.
    pub allowed: f32,
}

/// Finds clicks in a test tone of one frequency.
#[derive(Debug, Clone, Copy)]
pub struct Clicks {
    /// The largest step per unit of amplitude: `2 * sin(pi * f / rate)`.
    slope: f32,
    /// Frames on either side of a step in which its level is measured: one period.
    reach: usize,
    margin: f32,
}

impl Clicks {
    /// A detector for a tone of `frequency` Hz at `rate` frames per second.
    ///
    /// # Panics
    ///
    /// When `frequency` is not between 0 and half of `rate`.
    #[must_use]
    pub fn for_tone(frequency: f32, rate: u32) -> Self {
        let rate_f = rate as f32;
        assert!(
            frequency > 0.0 && frequency < rate_f / 2.0,
            "a tone between 0 and half the rate"
        );
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let reach = (rate_f / frequency).ceil() as usize;
        Self {
            slope: 2.0 * (PI * frequency / rate_f).sin(),
            reach,
            margin: MARGIN,
        }
    }

    /// The same with another margin, for a probe that wants to see where the steps stand.
    #[must_use]
    pub fn with_margin(self, margin: f32) -> Self {
        Self { margin, ..self }
    }

    /// The largest step the tone itself makes at `level`.
    #[must_use]
    pub fn bound(&self, level: f32) -> f32 {
        self.slope * level
    }

    /// Every step in `data` (interleaved, `channels` per frame) beyond what is allowed there, in
    /// order of frames and channels.
    #[must_use]
    pub fn find(&self, data: &[f32], channels: usize) -> Vec<Click> {
        let mut clicks = Vec::new();
        self.walk(data, channels, |click, _| {
            if click.step > click.allowed {
                clicks.push(click);
            }
        });
        clicks.sort_by_key(|click| (click.frame, click.channel));
        clicks
    }

    /// The steepest step in `data` relative to the bound of its level: 1 is as steep as the tone
    /// can be, [`MARGIN`] the edge of a click. Steps within [`FLOOR`] do not count.
    #[must_use]
    pub fn steepest(&self, data: &[f32], channels: usize) -> f32 {
        let mut steepest = 0.0_f32;
        self.walk(data, channels, |click, level| {
            if click.step > FLOOR && level > 0.0 {
                steepest = steepest.max(click.step / self.bound(level));
            }
        });
        steepest
    }

    /// Calls `each` with every step of `data` and the level around it.
    fn walk(&self, data: &[f32], channels: usize, mut each: impl FnMut(Click, f32)) {
        if channels == 0 {
            return;
        }
        let frames = data.len() / channels;
        let mut magnitudes = Vec::with_capacity(frames);
        for channel in 0..channels {
            magnitudes.clear();
            magnitudes.extend((0..frames).map(|frame| data[frame * channels + channel].abs()));
            let levels = window_max(&magnitudes, self.reach);
            for frame in 1..frames {
                let step = (data[frame * channels + channel]
                    - data[(frame - 1) * channels + channel])
                    .abs();
                let level = levels[frame].max(levels[frame - 1]);
                let allowed = self.margin * self.bound(level) + FLOOR;
                each(
                    Click {
                        frame,
                        channel,
                        step,
                        allowed,
                    },
                    level,
                );
            }
        }
    }
}

/// For every index, the largest value within `reach` on either side.
fn window_max(values: &[f32], reach: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(values.len());
    // Indices whose values fall from front to back: the front is the largest in the window.
    let mut deque: VecDeque<usize> = VecDeque::new();
    let mut next = 0;
    for index in 0..values.len() {
        let end = (index + reach).min(values.len() - 1);
        while next <= end {
            while deque
                .back()
                .is_some_and(|&back| values[back] <= values[next])
            {
                deque.pop_back();
            }
            deque.push_back(next);
            next += 1;
        }
        while deque.front().is_some_and(|&front| front + reach < index) {
            deque.pop_front();
        }
        out.push(deque.front().map_or(0.0, |&front| values[front]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const TONE: f32 = 100.0;

    /// `frames` of the tone at amplitude `gain(frame)`, starting at `phase` (radians).
    fn tone(frames: usize, phase: f32, gain: impl Fn(usize) -> f32) -> Vec<f32> {
        // In f64: an f32 phase a second long is too coarse for the bound to hold exactly.
        let step = std::f64::consts::TAU * f64::from(TONE) / f64::from(RATE);
        #[allow(clippy::cast_possible_truncation)]
        (0..frames)
            .map(|frame| gain(frame) * (f64::from(phase) + step * frame as f64).sin() as f32)
            .collect()
    }

    #[test]
    fn a_steady_tone_has_no_clicks_and_is_as_steep_as_its_bound() {
        let detector = Clicks::for_tone(TONE, RATE);
        for level in [1e-3, 0.1, 0.5, 1.0] {
            let data = tone(RATE as usize, 0.3, |_| level);
            assert!(detector.find(&data, 1).is_empty(), "{level}");
            let steepest = detector.steepest(&data, 1);
            // A tone this quiet moves by less than the floor: no step counts.
            let expected = if level < 0.01 {
                0.0..=0.0
            } else {
                0.99..=1.0001
            };
            assert!(expected.contains(&steepest), "{level}: {steepest}");
        }
    }

    #[test]
    fn the_shortest_ramp_passes_and_a_jump_in_the_gain_does_not() {
        let detector = Clicks::for_tone(TONE, RATE);
        // 5 ms from full level to silence, the shortest ramp of the mixer.
        let ramp = 240;
        let ramped = tone(9600, 0.0, |frame| {
            1.0 - (frame.saturating_sub(4800).min(ramp) as f32 / ramp as f32)
        });
        assert_eq!(detector.find(&ramped, 1), Vec::new());
        assert!(detector.steepest(&ramped, 1) < 1.5);

        // The same change at once, at every phase from a sixth of a period away from a zero
        // crossing: a period is 480 frames, a sixth 80.
        for at in (4800 + 80..4800 + 160).step_by(7) {
            let jumped = tone(9600, 0.0, |frame| if frame < at { 1.0 } else { 0.9 });
            let clicks = detector.find(&jumped, 1);
            assert_eq!(clicks.len(), 1, "at {at}: {clicks:?}");
            assert_eq!(clicks[0].frame, at);
        }
    }

    #[test]
    fn a_tone_that_starts_at_once_clicks_and_one_that_fades_in_does_not() {
        let detector = Clicks::for_tone(TONE, RATE);
        let onset = tone(4800, 1.0, |frame| if frame < 2400 { 0.0 } else { 0.25 });
        let clicks = detector.find(&onset, 1);
        assert_eq!(clicks.first().map(|c| c.frame), Some(2400), "{clicks:?}");
        let faded = tone(4800, 1.0, |frame| {
            0.25 * (frame.saturating_sub(2400).min(240) as f32 / 240.0)
        });
        assert_eq!(detector.find(&faded, 1), Vec::new());
    }

    #[test]
    fn a_skipped_or_delayed_piece_of_signal_clicks() {
        let detector = Clicks::for_tone(TONE, RATE);
        // A delay that changes by 1 ms at once, as a limiter's lookahead without its crossfade.
        let step = 2.0 * PI * TONE / RATE as f32;
        let delayed: Vec<f32> = (0..9600)
            .map(|frame| {
                let shift = if frame < 4810 { 0.0 } else { 48.0 };
                0.5 * (step * (frame as f32 - shift)).sin()
            })
            .collect();
        let clicks = detector.find(&delayed, 1);
        assert_eq!(clicks.len(), 1, "{clicks:?}");
        assert_eq!(clicks[0].frame, 4810);
    }

    #[test]
    fn channels_are_looked_at_on_their_own_and_quiet_noise_is_no_click() {
        let detector = Clicks::for_tone(TONE, RATE);
        let left = tone(4800, 0.0, |_| 0.5);
        let right = tone(4800, 0.0, |frame| if frame < 1000 { 0.5 } else { 0.25 });
        let mut data: Vec<f32> = left
            .iter()
            .zip(&right)
            .flat_map(|(l, r)| [*l, *r])
            .collect();
        let clicks = detector.find(&data, 2);
        assert_eq!(clicks.len(), 1, "{clicks:?}");
        assert_eq!((clicks[0].frame, clicks[0].channel), (1000, 1));

        // Rounding noise in silence stays below the floor.
        for (index, sample) in data.iter_mut().enumerate() {
            *sample = if index % 2 == 0 { 3e-5 } else { -3e-5 };
        }
        assert_eq!(detector.find(&data, 2), Vec::new());
    }

    #[test]
    fn the_level_is_the_largest_magnitude_within_reach() {
        let values = [0.0, 3.0, 1.0, 0.0, 0.0, 2.0, 0.0];
        assert_eq!(
            window_max(&values, 1),
            vec![3.0, 3.0, 3.0, 1.0, 2.0, 2.0, 2.0]
        );
        assert_eq!(window_max(&values, 0), values.to_vec());
        assert_eq!(window_max(&[], 3), Vec::<f32>::new());
    }
}
