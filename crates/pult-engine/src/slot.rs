//! One device (or program) of a running route, and how it survives being unplugged.
//!
//! A slot owns its device's stream and the state that stream's callback works on: the rings
//! of an input or the mixer of a bus (a [`Feed`]). The callback holds the feed in a
//! [`Returning`], which sends it back to the slot when the stream drops the callback. So a
//! lost stream gives its rings back and a new stream continues on them, without a lock in
//! the callback. A stream counts as lost when its error callback reports a fatal error or
//! when its callbacks stop for [`STALL_AFTER`] (a device that vanished without a word).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use assert_no_alloc::assert_no_alloc;
use cpal::traits::StreamTrait;
use cpal::{Device, ErrorKind, SupportedStreamConfig};
use pult_core::{Capture, Counter, InputStage, Mixer};

use crate::convert;
use crate::process::{PROCESS_CHANNELS, ProcessStream};
use crate::{Error, Role, audio};

/// How long a backend may take to open a stream.
pub(crate) const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// Callbacks run every few milliseconds; none for this long means the stream is dead.
pub(crate) const STALL_AFTER: Duration = Duration::from_secs(2);

/// How often a lost device is looked for again.
pub(crate) const RETRY_EVERY: Duration = Duration::from_secs(1);

/// The least time between two looks for a lost device that the device alarm brings forward.
pub(crate) const EARLY_GAP: Duration = Duration::from_millis(100);

/// How long a dropped stream may take to hand its feed back. Dropping a stream joins its
/// callback thread, so the feed is normally there at once.
const RECLAIM_TIMEOUT: Duration = Duration::from_secs(1);

/// What the callback of an input works on: its stage (PRE meter, gate, compressor), then its
/// rings.
#[derive(Debug)]
pub(crate) struct InputFeed {
    pub(crate) stage: InputStage,
    /// One per bus it feeds.
    pub(crate) captures: Vec<Capture>,
}

/// What one stream's callback works on.
#[derive(Debug)]
pub(crate) enum Feed {
    /// The stage and the captures of an input; boxed, a stage is several times a mixer.
    Input(Box<InputFeed>),
    /// The mixer of a bus.
    Bus(Mixer),
}

/// What one stream's callbacks report to the control thread.
#[derive(Debug, Default)]
pub(crate) struct Health {
    fatal: Mutex<Option<String>>,
    /// Data callbacks so far.
    beats: Counter,
    /// Whether Windows refused the callback thread realtime priority.
    slow: AtomicBool,
}

impl Health {
    /// Records `error` as the stream's fatal error, unless it already has one.
    fn fail(&self, error: String) {
        let mut slot = self.fatal.lock().unwrap_or_else(PoisonError::into_inner);
        slot.get_or_insert(error);
    }

    /// The first fatal error of the stream, if any.
    pub(crate) fn fatal(&self) -> Option<String> {
        self.fatal
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The realtime priority of a callback thread (MMCSS "Pro Audio"): taken on the callback's
/// first call, given back when the callback is dropped, which the audio system does on the
/// same thread when the stream ends. Without it, a build that takes every core delays the
/// callbacks until the rings run over.
#[derive(Debug, Default)]
struct Realtime {
    tried: bool,
    #[cfg(windows)]
    joined: Option<crate::ffi::ProAudio>,
}

impl Realtime {
    /// Raises the calling thread the first time; a refusal is kept in `health`.
    fn raise(&mut self, health: &Health) {
        if std::mem::replace(&mut self.tried, true) {
            return;
        }
        #[cfg(windows)]
        {
            self.joined = crate::ffi::ProAudio::join();
        }
        if !self.raised() {
            health.slow.store(true, Ordering::Relaxed);
        }
    }

    /// Whether the thread runs at realtime priority.
    #[cfg_attr(
        not(windows),
        expect(clippy::unused_self, reason = "no realtime priority there yet")
    )]
    fn raised(&self) -> bool {
        #[cfg(windows)]
        {
            self.joined.is_some()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
}

/// The error callback of one stream. A glitch, a followed device change or a refused
/// realtime priority keeps the stream running and is not fatal; anything else is.
///
/// A glitch before the stream's first data is no glitch: WASAPI's discontinuity flag is
/// undefined on the first packet after a start. cpal leaves it out only at device position 0,
/// so whoever joins a capture device others are reading got one on every start.
pub(crate) fn fault_recorder(
    health: &Arc<Health>,
    xruns: &Arc<Counter>,
) -> impl FnMut(cpal::Error) + Send + 'static {
    let health = Arc::clone(health);
    let xruns = Arc::clone(xruns);
    move |err| {
        if err.kind() == ErrorKind::Xrun {
            if health.beats.get() > 0 {
                xruns.add(1);
            }
        } else if is_fatal(err.kind()) {
            health.fail(err.to_string());
        }
    }
}

/// Whether a stream error ends the stream.
pub(crate) fn is_fatal(kind: ErrorKind) -> bool {
    !matches!(
        kind,
        ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied
    )
}

/// A value a callback owns until the callback is dropped, then sent `home`.
struct Returning<T> {
    value: Option<T>,
    home: Sender<T>,
}

impl<T> Returning<T> {
    /// The value. Callbacks reach it only through this method: a closure that named the
    /// field would capture just the field (a copy, for a `Copy` value) and leave the
    /// sender behind, so dropping the callback would send nothing.
    fn get_mut(&mut self) -> Option<&mut T> {
        self.value.as_mut()
    }
}

impl<T> Drop for Returning<T> {
    fn drop(&mut self) {
        if let Some(value) = self.value.take() {
            // The slot may be gone already (the whole route stopping); then nobody needs it.
            let _ = self.home.send(value);
        }
    }
}

/// Notices callbacks that stopped without an error.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Watch {
    beats: u64,
    since: Instant,
}

impl Watch {
    /// A watch that starts counting at `now`.
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            beats: 0,
            since: now,
        }
    }

    /// Whether the callback count `beats` has stood still for [`STALL_AFTER`] up to `now`.
    pub(crate) fn stalled(&mut self, beats: u64, now: Instant) -> bool {
        if beats != self.beats {
            self.beats = beats;
            self.since = now;
            return false;
        }
        now.saturating_duration_since(self.since) >= STALL_AFTER
    }
}

/// What a slot opens: a device in its shared-mode format, or a program's sound.
pub(crate) enum Source<'a> {
    /// An input or output device.
    Device {
        device: &'a Device,
        config: &'a SupportedStreamConfig,
    },
    /// A process tree, captured at the route's rate.
    Process {
        pid: u32,
        name: &'a str,
        sample_rate: u32,
    },
}

/// The running stream of a slot, held so that dropping it stops the stream.
#[expect(dead_code, reason = "the streams are only kept alive, never read")]
enum Stream {
    Device(cpal::Stream),
    Process(ProcessStream),
}

/// One device of a route: its stream while it plays, its feed while it does not.
pub(crate) struct Slot {
    /// Input or output.
    pub(crate) role: Role,
    /// The name the user gave, looked for again after a loss.
    pub(crate) wanted: String,
    /// The device's full name when it was last opened.
    pub(crate) name: String,
    stream: Option<Stream>,
    /// The feed while no stream holds it.
    held: Option<Feed>,
    home: Sender<Feed>,
    back: Receiver<Feed>,
    health: Arc<Health>,
    watch: Watch,
    next_try: Instant,
    /// The last reason a retry failed, reported once per reason.
    pub(crate) refused: Option<String>,
    /// Whether the open stream's missing realtime priority was reported.
    told_slow: bool,
}

impl Slot {
    /// A closed slot for the device `wanted` names, holding `feed`.
    pub(crate) fn new(role: Role, wanted: &str, feed: Feed, now: Instant) -> Self {
        let (home, back) = mpsc::channel();
        Self {
            role,
            wanted: wanted.to_owned(),
            name: wanted.to_owned(),
            stream: None,
            held: Some(feed),
            home,
            back,
            health: Arc::new(Health::default()),
            watch: Watch::new(now),
            next_try: now,
            refused: None,
            told_slow: false,
        }
    }

    /// Opens and starts a stream on `source`, handing it the feed. A bus's mixer starts on
    /// empty rings; an input must keep its channel count, since its rings were made for it.
    /// The stall watch starts once the stream plays, since opening several devices in a
    /// row can take seconds.
    pub(crate) fn open(&mut self, source: &Source<'_>, xruns: &Arc<Counter>) -> Result<(), Error> {
        let (name, channels) = match source {
            Source::Device { device, config } => {
                (device.to_string(), usize::from(config.channels()))
            }
            Source::Process { name, .. } => ((*name).to_owned(), usize::from(PROCESS_CHANNELS)),
        };
        let Some(mut feed) = self.held.take() else {
            return Err(Error::Stranded { device: name });
        };
        match &mut feed {
            Feed::Input(input) => {
                let expected = input.stage.channels();
                if channels != expected {
                    self.held = Some(feed);
                    return Err(Error::LayoutChanged {
                        device: name,
                        channels,
                        expected,
                    });
                }
                // A new stream: gate and compressor start from rest, not from where the old
                // one left them.
                input.stage.start_over();
            }
            Feed::Bus(mixer) => mixer.resume(channels),
        }
        let health = Arc::new(Health::default());
        let returning = Returning {
            value: Some(feed),
            home: self.home.clone(),
        };
        // A failed build or start drops the callback, which sends the feed back.
        let started = match source {
            Source::Device { device, config } => {
                Self::start_device(self.role, device, config, &health, xruns, returning)
                    .map(Stream::Device)
            }
            Source::Process {
                pid, sample_rate, ..
            } => {
                let failed = Arc::clone(&health);
                ProcessStream::start(
                    *pid,
                    *sample_rate,
                    input_callback(Arc::clone(&health), returning),
                    move |error| failed.fail(error),
                )
                .map(Stream::Process)
                .map_err(|message| Error::Process {
                    context: format!("capturing {name}"),
                    message,
                })
            }
        };
        match started {
            Ok(stream) => {
                self.stream = Some(stream);
                self.health = health;
                self.watch = Watch::new(Instant::now());
                self.name = name;
                self.refused = None;
                self.told_slow = false;
                Ok(())
            }
            Err(err) => {
                self.reclaim();
                Err(err)
            }
        }
    }

    /// Builds and plays a device stream whose callback owns `returning`.
    fn start_device(
        role: Role,
        device: &Device,
        config: &SupportedStreamConfig,
        health: &Arc<Health>,
        xruns: &Arc<Counter>,
        mut returning: Returning<Feed>,
    ) -> Result<cpal::Stream, Error> {
        let name = device.to_string();
        let stream = if role == Role::Output {
            let beats = Arc::clone(health);
            // The callback owns the mixer; what the device says of its latency reaches the
            // stats of the bus through this cell, which the callback reads next.
            let said = Arc::new(pult_core::AtomicF32::new(f32::NAN));
            let heard = Arc::clone(&said);
            let mut realtime = Realtime::default();
            convert::build_output(
                device,
                config,
                move |data| {
                    realtime.raise(&beats);
                    assert_no_alloc(|| {
                        beats.beats.add(1);
                        match returning.get_mut() {
                            Some(Feed::Bus(mixer)) => {
                                mixer.note_device_latency_ms(heard.load());
                                mixer.process(data);
                            }
                            _ => data.fill(0.0),
                        }
                    });
                },
                move |latency| said.store(latency.as_secs_f32() * 1e3),
                fault_recorder(health, xruns),
            )?
        } else {
            convert::build_input(
                device,
                config,
                input_callback(Arc::clone(health), returning),
                fault_recorder(health, xruns),
            )?
        };
        // A stream that does not start is dropped here, which hands the feed back.
        stream.play().map_err(audio(format!("starting {name}")))?;
        Ok(stream)
    }

    /// Why the stream should be given up, if it should: a fatal error it reported, or
    /// callbacks that stopped.
    pub(crate) fn trouble(&mut self, now: Instant) -> Option<String> {
        self.stream.as_ref()?;
        if let Some(fatal) = self.health.fatal() {
            return Some(fatal);
        }
        if self.watch.stalled(self.health.beats.get(), now) {
            return Some(format!("no audio from it for {} s", STALL_AFTER.as_secs()));
        }
        None
    }

    /// Whether the open stream runs without realtime priority and nobody was told yet;
    /// true once per stream.
    pub(crate) fn newly_slow(&mut self) -> bool {
        if self.stream.is_none() || self.told_slow || !self.health.slow.load(Ordering::Relaxed) {
            return false;
        }
        self.told_slow = true;
        true
    }

    /// Drops the stream and takes the feed back; the first retry comes after
    /// [`RETRY_EVERY`].
    pub(crate) fn close(&mut self, now: Instant) {
        if self.stream.take().is_some() {
            self.reclaim();
        }
        self.next_try = now + RETRY_EVERY;
        self.refused = None;
    }

    /// Brings the next look of a closed slot forward to `now` (the device alarm rang), but no
    /// closer than [`EARLY_GAP`] to the last one: a flood of notifications (a device that keeps
    /// changing its state) must not have the control thread enumerate devices on every tick. An
    /// open slot is left alone.
    pub(crate) fn look_now(&mut self, now: Instant) {
        if self.stream.is_none() {
            // The last look was RETRY_EVERY before the next one.
            let earliest = self
                .next_try
                .checked_sub(RETRY_EVERY.saturating_sub(EARLY_GAP))
                .unwrap_or(now);
            self.next_try = self.next_try.min(now.max(earliest));
        }
    }

    /// Whether a closed slot should look for its device now; if so, the next look is
    /// [`RETRY_EVERY`] later.
    pub(crate) fn retry_due(&mut self, now: Instant) -> bool {
        if self.stream.is_some() || now < self.next_try {
            return false;
        }
        self.next_try = now + RETRY_EVERY;
        true
    }

    /// Takes the feed back from a dropped stream's callback. A bus's mixer closes its
    /// rings, since nothing drains them until the device returns.
    fn reclaim(&mut self) {
        if let Ok(mut feed) = self.back.recv_timeout(RECLAIM_TIMEOUT) {
            if let Feed::Bus(mixer) = &mut feed {
                mixer.suspend();
            }
            self.held = Some(feed);
        }
    }
}

/// The data callback of an input: counts the call and pushes `data` into every ring.
fn input_callback(
    health: Arc<Health>,
    mut returning: Returning<Feed>,
) -> impl FnMut(&[f32]) + Send + 'static {
    let mut realtime = Realtime::default();
    move |data| {
        realtime.raise(&health);
        assert_no_alloc(|| {
            health.beats.add(1);
            if let Some(Feed::Input(input)) = returning.get_mut() {
                let InputFeed { stage, captures } = &mut **input;
                stage.feed_captures(data, captures);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_device_alarm_only_brings_the_next_look_forward() {
        let ms = Duration::from_millis;
        let now = Instant::now();
        let (mixer, _handle) = Mixer::new(Vec::new(), 2);
        let mut slot = Slot::new(Role::Output, "speakers", Feed::Bus(mixer), now);
        // Without the alarm, as always: the first look at once, then one every second.
        assert!(slot.retry_due(now));
        assert!(!slot.retry_due(now + ms(10)));
        assert!(!slot.retry_due(now + RETRY_EVERY.saturating_sub(ms(1))));
        assert!(slot.retry_due(now + RETRY_EVERY));
        // The alarm rings: the next look now, once, then the second again.
        let rang = now + RETRY_EVERY + ms(100);
        slot.look_now(rang);
        assert!(slot.retry_due(rang));
        assert!(!slot.retry_due(rang + ms(1)));
        // A flood of rings: at most one early look every EARLY_GAP, none lost.
        slot.look_now(rang + ms(1));
        assert!(!slot.retry_due(rang + ms(50)));
        assert!(slot.retry_due(rang + EARLY_GAP));
        let rang = rang + EARLY_GAP;
        assert!(!slot.retry_due(rang + ms(1)));
        assert!(slot.retry_due(rang + RETRY_EVERY));
        // A look that was due already stays due.
        let late = rang + 3 * RETRY_EVERY;
        slot.look_now(rang + ms(2500));
        assert!(slot.retry_due(late));
    }

    #[test]
    fn a_callback_thread_gets_realtime_priority_and_gives_it_back() {
        // Windows (the audio service runs MMCSS) grants "Pro Audio" to any thread. Streams
        // come and go with every unplugging, each on a new thread, so many in a row must all
        // work.
        for _ in 0..200 {
            let health = Arc::new(Health::default());
            let on_thread = Arc::clone(&health);
            let raised = std::thread::spawn(move || {
                let mut realtime = Realtime::default();
                realtime.raise(&on_thread);
                realtime.raise(&on_thread);
                realtime.raised()
            })
            .join()
            .expect("join the callback thread");
            assert_eq!(raised, cfg!(windows));
            assert_eq!(health.slow.load(Ordering::Relaxed), !cfg!(windows));
        }
        // On one thread too: given back, it can be taken again.
        #[cfg(windows)]
        {
            let mut first = Realtime::default();
            first.raise(&Health::default());
            drop(first);
        }
        let mut second = Realtime::default();
        second.raise(&Health::default());
        assert_eq!(second.raised(), cfg!(windows));
    }

    #[test]
    fn only_lost_devices_and_real_failures_are_fatal() {
        for kind in [
            ErrorKind::Xrun,
            ErrorKind::DeviceChanged,
            ErrorKind::RealtimeDenied,
        ] {
            assert!(!is_fatal(kind), "{kind:?}");
        }
        for kind in [
            ErrorKind::DeviceNotAvailable,
            ErrorKind::StreamInvalidated,
            ErrorKind::BackendError,
        ] {
            assert!(is_fatal(kind), "{kind:?}");
        }
    }

    #[test]
    fn fault_recorder_keeps_the_first_fatal_error_and_counts_glitches() {
        let health = Arc::new(Health::default());
        let xruns = Arc::new(Counter::default());
        let mut record = fault_recorder(&health, &xruns);
        health.beats.add(1);
        record(ErrorKind::Xrun.into());
        record(ErrorKind::DeviceChanged.into());
        assert_eq!(health.fatal(), None);
        record(ErrorKind::DeviceNotAvailable.into());
        let first = health.fatal().expect("a fatal error");
        record(ErrorKind::BackendError.into());
        assert_eq!(health.fatal(), Some(first));
        assert_eq!(xruns.get(), 1);
    }

    #[test]
    fn a_glitch_before_the_first_data_does_not_count() {
        let health = Arc::new(Health::default());
        let xruns = Arc::new(Counter::default());
        let mut record = fault_recorder(&health, &xruns);
        record(ErrorKind::Xrun.into());
        assert_eq!(xruns.get(), 0);
        health.beats.add(1);
        record(ErrorKind::Xrun.into());
        assert_eq!(xruns.get(), 1);
    }

    #[test]
    fn returning_sends_its_value_home_when_dropped() {
        let (home, back) = mpsc::channel();
        let mut returning = Returning {
            value: Some(7),
            home,
        };
        let mut callback = move || {
            if let Some(value) = returning.get_mut() {
                *value += 1;
            }
        };
        callback();
        assert!(back.try_recv().is_err());
        drop(callback);
        assert_eq!(back.try_recv(), Ok(8));
    }

    #[test]
    fn watch_flags_callbacks_that_stopped() {
        let start = Instant::now();
        let mut watch = Watch::new(start);
        assert!(!watch.stalled(0, start + STALL_AFTER / 2));
        assert!(!watch.stalled(5, start + STALL_AFTER));
        assert!(!watch.stalled(5, start + STALL_AFTER + STALL_AFTER / 2));
        assert!(watch.stalled(5, start + 2 * STALL_AFTER));
        assert!(!watch.stalled(6, start + 3 * STALL_AFTER));
    }

    #[test]
    fn a_stream_that_never_called_back_stalls_too() {
        let start = Instant::now();
        let mut watch = Watch::new(start);
        assert!(watch.stalled(0, start + STALL_AFTER));
    }
}
