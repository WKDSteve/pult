//! How long sound takes on a way: short bursts played on one stream, found again in a recording
//! of another, and the time between both. No devices here; the engine plays and records
//! (`pult_engine::roundtrip`) and hands over what its callbacks saw.
//!
//! The time base is the frame counter of each stream, not the time of a single callback: a
//! callback can come late, never early, so the earliest relation of callback time to frame
//! counter that a stream showed is its clock ([`time_of`]). That takes the jitter of the
//! callbacks out of the result.
//!
//! What comes out contains the buffers of the measuring streams themselves. Measure twice, once
//! straight (output over a cable to input) and once with the way in question in between; the
//! difference is that way.

use std::f32::consts::TAU;

/// The pitch of a burst: one whole wave of it is played.
pub const BURST_HZ: f32 = 1000.0;

/// From which part of a recording's peak a burst counts as begun.
const ONSET_PART: f32 = 0.25;

/// How far around a frame the callbacks count for its time, in seconds: near enough that the
/// clocks of two devices do not drift apart meanwhile (100 ppm over a second are 0.1 ms).
const CLOCK_WINDOW_S: f64 = 1.0;

/// A stream that fell behind its counter by more than this between two callbacks stood still
/// (a loopback delivers nothing while nothing plays): its clock starts anew.
const STALL_S: f64 = 0.03;

/// One callback of a stream: when it ran and where its frame counter stood.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mark {
    /// Seconds since the measurement started.
    pub at: f64,
    /// For an output the frames written before this callback (it asks for the next ones now),
    /// for an input the frames delivered up to and including it (they are complete now).
    pub frames: u64,
}

/// How many frames a burst lasts at `rate`.
#[must_use]
pub fn burst_frames(rate: u32) -> u64 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let frames = (f64::from(rate) / f64::from(BURST_HZ)).round() as u64;
    frames.max(2)
}

/// Frame `n` of a burst at `rate`, between -1 and 1: one wave of a sine, silence after it.
#[must_use]
pub fn burst(n: u64, rate: u32) -> f32 {
    let frames = burst_frames(rate);
    if n >= frames {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let turn = n as f32 / frames as f32;
    (TAU * turn).sin()
}

/// How many frames into a burst it reaches the part of its peak that [`onsets`] waits for: the
/// played burst counts as begun there too, so both ends are read alike.
fn onset_into_burst(rate: u32) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let frames = burst_frames(rate) as f64;
    f64::from(ONSET_PART).asin() / std::f64::consts::TAU * frames
}

/// The frames at which a burst begins in `samples` (interleaved, `channels` per frame): where
/// the loudest channel first reaches a quarter of the recording's peak after having stayed
/// below it for `apart` frames. Empty when the recording is silent or has no whole frame.
#[must_use]
pub fn onsets(samples: &[f32], channels: usize, apart: usize) -> Vec<usize> {
    if channels == 0 {
        return Vec::new();
    }
    let peak = samples.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
    if !(peak > 0.0 && peak.is_finite()) {
        return Vec::new();
    }
    let level = peak * ONSET_PART;
    let mut found = Vec::new();
    // Far enough back that the first burst may begin with the recording.
    let mut loud_at: Option<usize> = None;
    for (frame, values) in samples.chunks_exact(channels).enumerate() {
        if values.iter().any(|s| s.abs() >= level) {
            if loud_at.is_none_or(|last| frame - last > apart) {
                found.push(frame);
            }
            loud_at = Some(frame);
        }
    }
    found
}

/// When the stream of `marks` (in the order they were taken) at `rate` frames per second
/// passed `frame`, in seconds since the measurement started: by the earliest relation of
/// callback time to frame counter among the callbacks around that frame. `None` without a
/// callback near it.
#[must_use]
pub fn time_of(marks: &[Mark], rate: u32, frame: f64) -> Option<f64> {
    let rate = f64::from(rate);
    #[allow(clippy::cast_precision_loss)]
    let offset = |mark: &Mark| mark.at - mark.frames as f64 / rate;
    #[allow(clippy::cast_precision_loss)]
    let near = |mark: &Mark| (mark.frames as f64 - frame).abs() <= CLOCK_WINDOW_S * rate;
    // The stretch without a stall that holds the frame: the last stall at or before it starts
    // it, the first one behind it ends it.
    let mut from = 0;
    let mut to = marks.len();
    for (index, pair) in marks.windows(2).enumerate() {
        if offset(&pair[1]) - offset(&pair[0]) > STALL_S {
            #[allow(clippy::cast_precision_loss)]
            let before = (pair[0].frames as f64) < frame;
            if before {
                from = index + 1;
            } else {
                to = index + 1;
                break;
            }
        }
    }
    marks[from..to]
        .iter()
        .filter(|mark| near(mark))
        .map(offset)
        .min_by(f64::total_cmp)
        .map(|earliest| earliest + frame / rate)
}

/// What one measurement saw, as the engine hands it over.
#[derive(Debug, Clone, Copy)]
pub struct Seen<'a> {
    /// The output frames at which the bursts began.
    pub played: &'a [u64],
    /// The callbacks of the output.
    pub out_marks: &'a [Mark],
    /// The output's frames per second.
    pub out_rate: u32,
    /// The recording, interleaved.
    pub samples: &'a [f32],
    /// Channels per frame of the recording.
    pub channels: usize,
    /// The callbacks of the input.
    pub in_marks: &'a [Mark],
    /// The input's frames per second.
    pub in_rate: u32,
    /// The time from one burst to the next, in seconds: a burst is looked for until the next
    /// one is played.
    pub gap_s: f64,
}

/// Per played burst how long it took until it was recorded, in milliseconds; `None` for one
/// that did not arrive before the next was played (or whose time cannot be told).
#[must_use]
pub fn latencies(seen: &Seen) -> Vec<Option<f64>> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let apart = (seen.gap_s * f64::from(seen.in_rate) / 2.0) as usize;
    let arrived: Vec<f64> = onsets(seen.samples, seen.channels, apart.max(1))
        .into_iter()
        .filter_map(|frame| {
            #[allow(clippy::cast_precision_loss)]
            let frame = frame as f64;
            time_of(seen.in_marks, seen.in_rate, frame)
        })
        .collect();
    let into = onset_into_burst(seen.out_rate);
    seen.played
        .iter()
        .map(|&frame| {
            #[allow(clippy::cast_precision_loss)]
            let left = time_of(seen.out_marks, seen.out_rate, frame as f64 + into)?;
            arrived
                .iter()
                .find(|&&at| at >= left && at < left + seen.gap_s)
                .map(|at| (at - left) * 1000.0)
        })
        .collect()
}

/// The middle, the ends and the spread of a row of times.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    /// The middle one (of an even number the mean of the two in the middle).
    pub median: f64,
    /// The shortest.
    pub min: f64,
    /// The longest.
    pub max: f64,
    /// The standard deviation.
    pub spread: f64,
}

/// Sums `values` up; `None` for none.
#[must_use]
pub fn summarize(values: &[f64]) -> Option<Summary> {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let (first, last) = (*sorted.first()?, *sorted.last()?);
    let middle = sorted.len() / 2;
    let median = if sorted.len() % 2 == 1 {
        sorted[middle]
    } else {
        f64::midpoint(sorted[middle - 1], sorted[middle])
    };
    #[allow(clippy::cast_precision_loss)]
    let count = sorted.len() as f64;
    let mean = sorted.iter().sum::<f64>() / count;
    let variance = sorted.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / count;
    Some(Summary {
        median,
        min: first,
        max: last,
        spread: variance.sqrt(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// A stream's callbacks every `period` frames from `start_s` on, each `late(n)` seconds
    /// late; an output's marks when `output`, else an input's.
    fn marks(
        start_s: f64,
        period: u64,
        count: u64,
        output: bool,
        late: impl Fn(u64) -> f64,
    ) -> Vec<Mark> {
        (0..count)
            .map(|n| {
                let frames = if output { n * period } else { (n + 1) * period };
                #[allow(clippy::cast_precision_loss)]
                let at = start_s + frames as f64 / f64::from(RATE) + late(n);
                Mark { at, frames }
            })
            .collect()
    }

    /// A recording of `frames` frames in stereo with a burst at half scale from each of `at`.
    fn recording(frames: usize, at: &[usize], scale: f32) -> Vec<f32> {
        let mut samples = vec![0.0_f32; frames * 2];
        for &start in at {
            for n in 0..burst_frames(RATE) {
                #[allow(clippy::cast_possible_truncation)]
                let frame = start + n as usize;
                if frame < frames {
                    samples[frame * 2] = scale * burst(n, RATE);
                    samples[frame * 2 + 1] = scale * burst(n, RATE);
                }
            }
        }
        samples
    }

    #[test]
    fn a_burst_is_one_wave_and_then_silence() {
        assert_eq!(burst_frames(RATE), 48);
        assert!(burst(0, RATE).abs() < 1e-6);
        assert!((burst(12, RATE) - 1.0).abs() < 1e-6);
        assert!((burst(36, RATE) + 1.0).abs() < 1e-6);
        assert!(burst(48, RATE).abs() < f32::EPSILON);
        assert!(burst(10_000, RATE).abs() < f32::EPSILON);
    }

    #[test]
    fn onsets_are_where_each_burst_begins() {
        let samples = recording(48_000, &[1000, 13_000, 25_000], 0.5);
        // A quarter of the peak is reached two frames into the wave.
        assert_eq!(onsets(&samples, 2, 6000), vec![1002, 13_002, 25_002]);
    }

    #[test]
    fn noise_far_below_the_bursts_is_no_onset() {
        let mut samples = recording(48_000, &[1000, 25_000], 0.5);
        for (n, sample) in samples.iter_mut().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let wobble = 0.01 * (n as f32 * 0.37).sin();
            *sample += wobble;
        }
        let found = onsets(&samples, 2, 6000);
        assert_eq!(found.len(), 2);
        assert!(found[0].abs_diff(1002) <= 1 && found[1].abs_diff(25_002) <= 1);
    }

    #[test]
    fn a_silent_or_broken_recording_has_no_onsets() {
        assert_eq!(onsets(&[0.0; 960], 2, 100), [] as [usize; 0]);
        assert_eq!(onsets(&[], 2, 100), [] as [usize; 0]);
        assert_eq!(onsets(&[0.5; 8], 0, 100), [] as [usize; 0]);
        assert_eq!(onsets(&[f32::INFINITY, 0.5], 1, 100), [] as [usize; 0]);
    }

    #[test]
    fn a_burst_that_rings_on_is_one_onset() {
        // What a filter makes of a burst: it swings on for some milliseconds.
        let mut samples = recording(48_000, &[1000], 0.5);
        for n in 0..960 {
            #[allow(clippy::cast_precision_loss)]
            let ring = 0.3 * (n as f32 * 0.13).sin();
            samples[(1048 + n) * 2] = ring;
        }
        assert_eq!(onsets(&samples, 2, 6000), vec![1002]);
    }

    #[test]
    fn late_callbacks_do_not_move_the_clock() {
        // Every callback is late by something, a few by a lot; one in ten is on time.
        let jittery = marks(2.0, 480, 400, true, |n| {
            if n % 10 == 0 {
                0.0
            } else {
                0.004 + 0.001 * (n % 7) as f64
            }
        });
        let at = time_of(&jittery, RATE, 96_000.0).expect("callbacks around it");
        assert!((at - 4.0).abs() < 1e-9, "{at}");
        // A single callback's time would be off by milliseconds.
        assert!((jittery[201].at - 4.01 - 0.004).abs() < 0.01);
    }

    #[test]
    fn a_frame_far_from_every_callback_has_no_time() {
        let few = marks(0.0, 480, 10, true, |_| 0.0);
        assert!(time_of(&few, RATE, 480_000.0).is_none());
        assert!(time_of(&[], RATE, 0.0).is_none());
    }

    #[test]
    fn a_stream_that_stood_still_starts_its_clock_anew() {
        // A loopback that delivered nothing for 2 s: its counter went on where it had stopped.
        let mut stalled = marks(0.0, 480, 100, false, |_| 0.0);
        stalled.extend(
            marks(0.0, 480, 100, false, |_| 0.0)
                .into_iter()
                .map(|mark| Mark {
                    at: mark.at + 3.0,
                    frames: mark.frames + 48_000,
                }),
        );
        // Frame 60000 came 0.25 s after the stream went on at 3 s (frame 48000).
        let at = time_of(&stalled, RATE, 60_000.0).expect("callbacks around it");
        assert!((at - 3.25).abs() < 1e-9, "{at}");
        // Before the stall the old clock holds.
        let at = time_of(&stalled, RATE, 24_000.0).expect("callbacks around it");
        assert!((at - 0.5).abs() < 1e-9, "{at}");
    }

    #[test]
    fn the_way_is_the_time_between_playing_and_recording() {
        // The output starts at 0.2 s, the input at 0.05 s; a burst played at output frame p is
        // recorded 12.5 ms later.
        let out_marks = marks(0.2, 480, 300, true, |n| 0.002 * (n % 3) as f64);
        let in_marks = marks(0.05, 441, 400, false, |n| 0.003 * (n % 4) as f64);
        let played = [24_000_u64, 48_000, 72_000];
        let delay_s = 0.0125;
        let at: Vec<usize> = played
            .iter()
            .map(|&p| {
                #[allow(clippy::cast_precision_loss)]
                let seconds = 0.2 + p as f64 / 48_000.0 + delay_s - 0.05;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let frame = (seconds * 48_000.0).round() as usize;
                frame
            })
            .collect();
        let samples = recording(160_000, &at, 0.3);
        let seen = Seen {
            played: &played,
            out_marks: &out_marks,
            out_rate: RATE,
            samples: &samples,
            channels: 2,
            in_marks: &in_marks,
            in_rate: RATE,
            gap_s: 0.5,
        };
        let found = latencies(&seen);
        assert_eq!(found.len(), 3);
        for way in found {
            let way = way.expect("every burst arrived");
            // Within a frame: the onset is read at whole frames.
            assert!((way - 12.5).abs() < 0.03, "{way}");
        }
    }

    #[test]
    fn a_burst_that_never_arrives_is_lost_and_the_others_keep_their_place() {
        let out_marks = marks(0.0, 480, 300, true, |_| 0.0);
        let in_marks = marks(0.0, 480, 300, false, |_| 0.0);
        let played = [24_000_u64, 48_000, 72_000];
        // The second one is missing; the others take 5 ms.
        let samples = recording(140_000, &[24_240, 72_240], 0.3);
        let seen = Seen {
            played: &played,
            out_marks: &out_marks,
            out_rate: RATE,
            samples: &samples,
            channels: 2,
            in_marks: &in_marks,
            in_rate: RATE,
            gap_s: 0.5,
        };
        let found = latencies(&seen);
        assert!(found[0].is_some_and(|way| (way - 5.0).abs() < 0.03));
        assert_eq!(found[1], None);
        assert!(found[2].is_some_and(|way| (way - 5.0).abs() < 0.03));
    }

    #[test]
    fn a_summary_has_the_middle_the_ends_and_the_spread() {
        assert_eq!(summarize(&[]), None);
        let odd = summarize(&[3.0, 1.0, 2.0]).expect("values");
        assert!((odd.median - 2.0).abs() < 1e-12);
        assert!((odd.min - 1.0).abs() < 1e-12 && (odd.max - 3.0).abs() < 1e-12);
        assert!((odd.spread - (2.0_f64 / 3.0).sqrt()).abs() < 1e-12);
        let even = summarize(&[4.0, 1.0, 2.0, 3.0]).expect("values");
        assert!((even.median - 2.5).abs() < 1e-12);
        let one = summarize(&[7.0]).expect("a value");
        assert!((one.median - 7.0).abs() < 1e-12 && one.spread.abs() < 1e-12);
    }
}
