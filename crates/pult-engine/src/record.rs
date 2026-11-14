//! Recording a device for a measurement: what an input captures, or what an output plays.
//!
//! Not part of a route. A measurement plays something through a running pult and records where it
//! comes out, to look at the recording afterwards (`pult_core::clicks`). The callback only moves
//! samples into a ring; the caller's thread takes them out.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cpal::traits::StreamTrait;
use pult_core::Counter;
use rtrb::RingBuffer;

use crate::slot::{Health, fault_recorder};
use crate::{Error, Role, audio, convert, find_device};

/// Before the name of an output device: record what it plays (loopback) instead of an input.
pub const LOOPBACK_PREFIX: &str = "out:";

/// What a recording caught.
#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    /// The device's full name.
    pub source: String,
    /// Whether it is what an output played.
    pub loopback: bool,
    /// Frames per second.
    pub sample_rate: u32,
    /// Channels per frame.
    pub channels: u16,
    /// The samples, interleaved.
    pub samples: Vec<f32>,
    /// Frames the device delivered and the recording could not hold: each such gap is a jump in
    /// the samples that the device did not make.
    pub lost_frames: u64,
}

/// Records `source` for `seconds` and returns what came. `source` is an input device, or
/// [`LOOPBACK_PREFIX`] and an output device for what that output plays (Windows: a WASAPI
/// loopback, which delivers nothing while nothing plays there, so the recording is then
/// shorter than `seconds`). Device names work as everywhere: a unique part, ignoring case.
/// `keep_going` is asked a few times a second; false ends the recording early.
///
/// # Errors
///
/// [`Error`] when the device is missing or ambiguous, has an unsupported format, the stream
/// cannot be opened or started, or it fails while recording.
pub fn record(
    source: &str,
    seconds: f64,
    keep_going: impl Fn() -> bool,
) -> Result<Recording, Error> {
    let host = cpal::default_host();
    let (role, wanted) = match source.strip_prefix(LOOPBACK_PREFIX) {
        Some(output) => (Role::Output, output),
        None => (Role::Input, source),
    };
    let (device, name, config) = find_device(&host, role, wanted)?;
    let sample_rate = config.sample_rate();
    let channels = config.channels();
    let frame = usize::from(channels);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let wanted_samples = (seconds.max(0.0) * f64::from(sample_rate)) as usize * frame;
    // Two seconds of room: the taking thread looks every few milliseconds.
    let (mut producer, mut consumer) = RingBuffer::<f32>::new(sample_rate as usize * frame * 2);
    let lost = Arc::new(Counter::default());
    let health = Arc::new(Health::default());
    let stream = convert::build_input(
        &device,
        &config,
        {
            let lost = Arc::clone(&lost);
            move |data: &[f32]| {
                let frames = data.len() / frame;
                let stored = frames.min(producer.slots() / frame);
                if stored < frames {
                    lost.add((frames - stored) as u64);
                }
                if let Ok(chunk) = producer.write_chunk_uninit(stored * frame) {
                    chunk.fill_from_iter(data[..stored * frame].iter().copied());
                }
            }
        },
        fault_recorder(&health, &Arc::new(Counter::default())),
    )?;
    stream.play().map_err(audio(format!("starting {name}")))?;

    let mut samples = Vec::with_capacity(wanted_samples);
    let until = Instant::now() + Duration::from_secs_f64(seconds.max(0.0));
    while samples.len() < wanted_samples && Instant::now() < until && keep_going() {
        std::thread::sleep(Duration::from_millis(5));
        let waiting = consumer.slots().min(wanted_samples - samples.len());
        if let Ok(chunk) = consumer.read_chunk(waiting) {
            let (first, second) = chunk.as_slices();
            samples.extend_from_slice(first);
            samples.extend_from_slice(second);
            chunk.commit_all();
        }
        if let Some(fatal) = health.fatal() {
            return Err(Error::InvalidRoute(format!("{name}: {fatal}")));
        }
    }
    drop(stream);
    samples.truncate(samples.len() - samples.len() % frame);
    Ok(Recording {
        source: name,
        loopback: role == Role::Output,
        sample_rate,
        channels,
        samples,
        lost_frames: lost.get(),
    })
}
