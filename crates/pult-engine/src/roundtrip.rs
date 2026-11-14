//! Measuring how long a way takes: bursts played on an output while an input (or what an output
//! plays) is recorded, with every callback's time and frame counter noted on both sides.
//!
//! Not part of a route. The callbacks only write bursts, move samples and note marks into rings;
//! the caller's thread takes them out, and `pult_core::roundtrip` works the times out.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use assert_no_alloc::assert_no_alloc;
use cpal::traits::StreamTrait;
use pult_core::roundtrip::{Mark, Seen, burst, burst_frames};
use pult_core::{Counter, db_to_linear};
use rtrb::RingBuffer;

use crate::record::LOOPBACK_PREFIX;
use crate::slot::{Health, fault_recorder};
use crate::{Error, Role, audio, convert, find_device};

/// Silence before the first burst, so both streams run and their clocks are known.
const LEAD: Duration = Duration::from_secs(1);
/// The shortest time between two bursts: a way may take a while, and the bursts must stay apart.
pub const SHORTEST_GAP_MS: u32 = 50;
/// Marks a ring holds: the taking thread looks every few milliseconds, a callback comes about
/// every ten.
const MARKS: usize = 4096;

/// What to play and where to listen.
#[derive(Debug, Clone, PartialEq)]
pub struct RoundTripSpec {
    /// The output the bursts play on.
    pub sink: String,
    /// The input that hears them, or [`LOOPBACK_PREFIX`] and an output for what it plays.
    pub source: String,
    /// How many bursts.
    pub count: u32,
    /// Milliseconds from one burst to the next; at least [`SHORTEST_GAP_MS`].
    pub gap_ms: u32,
    /// The level of the bursts in dBFS.
    pub gain_db: f32,
}

/// What a measurement saw.
#[derive(Debug, Clone, PartialEq)]
pub struct RoundTrip {
    /// The output's full name.
    pub sink: String,
    /// The recorded device's full name.
    pub source: String,
    /// Whether the recording is what an output played.
    pub loopback: bool,
    /// The output's frames per second.
    pub out_rate: u32,
    /// The recording's frames per second.
    pub in_rate: u32,
    /// Channels per frame of the recording.
    pub channels: u16,
    /// The output frames at which the bursts began, those that were played to their end.
    pub played: Vec<u64>,
    /// The callbacks of the output.
    pub out_marks: Vec<Mark>,
    /// The callbacks of the input.
    pub in_marks: Vec<Mark>,
    /// The recording, interleaved.
    pub samples: Vec<f32>,
    /// Frames or marks the rings could not hold: the times of this run cannot be trusted.
    pub lost: u64,
    /// Seconds from one burst to the next.
    pub gap_s: f64,
}

impl RoundTrip {
    /// The measurement as `pult_core::roundtrip::latencies` takes it.
    #[must_use]
    pub fn seen(&self) -> Seen<'_> {
        Seen {
            played: &self.played,
            out_marks: &self.out_marks,
            out_rate: self.out_rate,
            samples: &self.samples,
            channels: usize::from(self.channels),
            in_marks: &self.in_marks,
            in_rate: self.in_rate,
            gap_s: self.gap_s,
        }
    }
}

/// Moves everything that waits in `ring` to the end of `all`.
fn take<T: Copy>(ring: &mut rtrb::Consumer<T>, all: &mut Vec<T>) {
    if let Ok(chunk) = ring.read_chunk(ring.slots()) {
        let (first, second) = chunk.as_slices();
        all.extend_from_slice(first);
        all.extend_from_slice(second);
        chunk.commit_all();
    }
}

/// Plays the bursts of `spec` and records its source meanwhile. `keep_going` is asked a few
/// times a second; false ends the measurement early.
///
/// # Errors
///
/// [`Error`] when a device is missing or ambiguous, has an unsupported format, a stream cannot
/// be opened or started, or fails meanwhile; [`Error::InvalidRoute`] for no bursts or a gap
/// below [`SHORTEST_GAP_MS`].
#[allow(clippy::too_many_lines)] // Two streams and their rings, set up in one place.
pub fn roundtrip(spec: &RoundTripSpec, keep_going: impl Fn() -> bool) -> Result<RoundTrip, Error> {
    if spec.count == 0 || spec.gap_ms < SHORTEST_GAP_MS {
        return Err(Error::InvalidRoute(format!(
            "a round trip needs at least one burst and {SHORTEST_GAP_MS} ms between two"
        )));
    }
    let host = cpal::default_host();
    let (role, wanted) = match spec.source.strip_prefix(LOOPBACK_PREFIX) {
        Some(output) => (Role::Output, output),
        None => (Role::Input, spec.source.as_str()),
    };
    let (in_device, source, in_config) = find_device(&host, role, wanted)?;
    let (out_device, sink, out_config) = find_device(&host, Role::Output, &spec.sink)?;
    let in_rate = in_config.sample_rate();
    let channels = in_config.channels();
    let frame = usize::from(channels);
    let out_rate = out_config.sample_rate();
    let out_frame = usize::from(out_config.channels());

    let epoch = Instant::now();
    let lost = Arc::new(Counter::default());
    let in_health = Arc::new(Health::default());
    let out_health = Arc::new(Health::default());

    // Two seconds of room for the samples.
    let (mut samples_in, mut samples_out) = RingBuffer::<f32>::new(in_rate as usize * frame * 2);
    let (mut in_marks_in, mut in_marks_out) = RingBuffer::<Mark>::new(MARKS);
    let in_stream = convert::build_input(
        &in_device,
        &in_config,
        {
            let lost = Arc::clone(&lost);
            let mut delivered = 0_u64;
            move |data: &[f32]| {
                assert_no_alloc(|| {
                    let frames = data.len() / frame;
                    let stored = frames.min(samples_in.slots() / frame);
                    if stored < frames {
                        lost.add((frames - stored) as u64);
                    }
                    if let Ok(chunk) = samples_in.write_chunk_uninit(stored * frame) {
                        chunk.fill_from_iter(data[..stored * frame].iter().copied());
                    }
                    delivered += frames as u64;
                    let mark = Mark {
                        at: epoch.elapsed().as_secs_f64(),
                        frames: delivered,
                    };
                    if in_marks_in.push(mark).is_err() {
                        lost.add(1);
                    }
                });
            }
        },
        fault_recorder(&in_health, &Arc::new(Counter::default())),
    )?;
    in_stream
        .play()
        .map_err(audio(format!("starting {source}")))?;

    let lead = u64::from(out_rate) * LEAD.as_secs();
    let gap = u64::from(out_rate) * u64::from(spec.gap_ms) / 1000;
    let count = u64::from(spec.count);
    let level = db_to_linear(spec.gain_db);
    let written = Arc::new(AtomicU64::new(0));
    let (mut out_marks_in, mut out_marks_out) = RingBuffer::<Mark>::new(MARKS);
    let out_stream = convert::build_output(
        &out_device,
        &out_config,
        {
            let lost = Arc::clone(&lost);
            let written = Arc::clone(&written);
            let mut at_frame = 0_u64;
            move |data: &mut [f32]| {
                assert_no_alloc(|| {
                    let mark = Mark {
                        at: epoch.elapsed().as_secs_f64(),
                        frames: at_frame,
                    };
                    if out_marks_in.push(mark).is_err() {
                        lost.add(1);
                    }
                    for values in data.chunks_exact_mut(out_frame) {
                        let value = match at_frame.checked_sub(lead) {
                            Some(since) if since / gap < count => {
                                level * burst(since % gap, out_rate)
                            }
                            _ => 0.0,
                        };
                        values.fill(value);
                        at_frame += 1;
                    }
                    written.store(at_frame, Ordering::Release);
                });
            }
        },
        |_| {},
        fault_recorder(&out_health, &Arc::new(Counter::default())),
    )?;
    out_stream
        .play()
        .map_err(audio(format!("starting {sink}")))?;

    // The last burst gets as long as every other to arrive.
    let gap_s = f64::from(spec.gap_ms) / 1000.0;
    let total = LEAD + Duration::from_secs_f64(gap_s * f64::from(spec.count + 1));
    let mut samples = Vec::with_capacity(in_rate as usize * frame * 2);
    let mut in_marks = Vec::with_capacity(MARKS);
    let mut out_marks = Vec::with_capacity(MARKS);
    // From the start of the output on: opening the streams took its time.
    let started = Instant::now();
    while started.elapsed() < total && keep_going() {
        std::thread::sleep(Duration::from_millis(5));
        take(&mut samples_out, &mut samples);
        take(&mut in_marks_out, &mut in_marks);
        take(&mut out_marks_out, &mut out_marks);
        for (health, name) in [(&in_health, &source), (&out_health, &sink)] {
            if let Some(fatal) = health.fatal() {
                return Err(Error::InvalidRoute(format!("{name}: {fatal}")));
            }
        }
    }
    drop(out_stream);
    drop(in_stream);
    take(&mut samples_out, &mut samples);
    take(&mut in_marks_out, &mut in_marks);
    take(&mut out_marks_out, &mut out_marks);
    samples.truncate(samples.len() - samples.len() % frame);

    let written = written.load(Ordering::Acquire);
    let played = (0..count)
        .map(|n| lead + n * gap)
        .filter(|start| start + burst_frames(out_rate) <= written)
        .collect();
    Ok(RoundTrip {
        sink,
        source,
        loopback: role == Role::Output,
        out_rate,
        in_rate,
        channels,
        played,
        out_marks,
        in_marks,
        samples,
        lost: lost.get(),
        gap_s,
    })
}
