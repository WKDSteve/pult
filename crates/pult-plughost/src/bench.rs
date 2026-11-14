//! The measurement of the spike: a host process, an engine thread that plays a tone through the
//! insert at a fixed block size, and a watchdog that restarts the host when it dies or hangs.
//!
//! The engine here is not a device callback. WASAPI's shared mode calls back every 10 ms whatever
//! one asks for, so blocks of 64 to 256 frames are paced by a timer: a thread in the "Pro Audio"
//! class sleeps on a high resolution timer until shortly before each block is due and spins the
//! rest. How late it still was is part of the report, so one can tell a slow host from a slow
//! pacer.

use std::io;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use pult_core::{Clicks, Sine};

use crate::ffi::{Timer, cpu_time};
use crate::layout::{WAKE_BUCKETS, word};
use crate::{Event, Geometry, Insert, InsertStats, ProAudio, Region, now_ns};

/// Frames per second of the measurement.
pub const RATE: u32 = 48_000;
/// The test tone in Hz, the one of the click tests of pult-engine.
pub const TONE: f32 = 103.0;
/// Its level, linear.
pub const LEVEL: f32 = 0.5;
/// Channels of the chain.
pub const CHANNELS: usize = 2;
/// Blocks each ring holds.
pub const BLOCKS: usize = 8;
/// A heartbeat that stands this long means the host hangs.
pub const STALL: Duration = Duration::from_millis(250);
/// How long a new host may take until it says it is ready.
pub const READY_TIMEOUT: Duration = Duration::from_secs(5);
/// Heartbeats after which a host that fell into bypass without dying counts as back.
const BEATS_BACK: u32 = 4;

/// One run of the measurement.
#[derive(Debug, Clone, Copy)]
pub struct Plan {
    /// Frames per block.
    pub frames: usize,
    /// How long to play, in seconds.
    pub seconds: f64,
    /// When to kill the host, in seconds from the start; `None` leaves it alone.
    pub kill_at: Option<f64>,
    /// The gain the host applies, linear.
    pub gain: f32,
}

/// What the kill of a host took.
#[derive(Debug, Clone, Copy, Default)]
pub struct Kill {
    /// From the kill until the new host was ready.
    pub to_ready: Option<Duration>,
    /// From the kill until a wet block played again.
    pub to_wet: Option<Duration>,
}

/// What one run found.
#[derive(Debug, Clone)]
pub struct Report {
    /// Frames per block.
    pub frames: usize,
    /// Blocks played.
    pub blocks: u64,
    /// How long the run took.
    pub wall: Duration,
    /// Blocks whose result was not there in time.
    pub misses: u64,
    /// The most misses in a row.
    pub longest_run: u32,
    /// Times the insert went to bypass.
    pub bypasses: u64,
    /// Blocks that could not be sent.
    pub send_full: u64,
    /// Blocks per wake delay of the host: up to 100, 250, 500, 1000 µs and beyond.
    pub wake: [u32; WAKE_BUCKETS],
    /// The longest wake delay in µs.
    pub wake_max_us: u32,
    /// The mean and the longest time one call of the insert took.
    pub insert_mean: Duration,
    /// See [`Self::insert_mean`].
    pub insert_max: Duration,
    /// The latest the pacer started a block after it was due, and how many blocks started later
    /// than a quarter of a block.
    pub late_max: Duration,
    /// See [`Self::late_max`].
    pub late_blocks: u64,
    /// Whether the engine thread got the "Pro Audio" class.
    pub realtime: bool,
    /// CPU time of all host processes of the run.
    pub host_cpu: Duration,
    /// Hosts started after the first one.
    pub restarts: u32,
    /// Clicks the detector found in what the insert played.
    pub clicks: usize,
    /// The steepest step relative to what the tone itself can do; 2 is the edge of a click.
    pub steepest: f32,
    /// The kill, where one was asked for.
    pub kill: Option<Kill>,
}

/// A name for the mapping of one chain that no other run of this machine has: this process and
/// a counter. The event that wakes the host is this name plus `-wake`.
#[must_use]
pub fn unique_name() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        "Local\\pult-plughost-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// The name of the event that wakes the host of the mapping `name`.
#[must_use]
pub fn wake_name(name: &str) -> String {
    format!("{name}-wake")
}

/// Starts the program `exe` as the host of the mapping `name`, with `state` (the bits of a gain)
/// as what a host before it left behind.
///
/// # Errors
///
/// When the program does not start.
pub fn spawn_host(exe: &Path, name: &str, geometry: Geometry, state: u32) -> io::Result<Child> {
    Command::new(exe)
        .arg("host")
        .args(["--name", name])
        .args(["--channels", &geometry.channels.to_string()])
        .args(["--frames", &geometry.frames.to_string()])
        .args(["--blocks", &geometry.blocks.to_string()])
        .args(["--state", &state.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
}

/// Waits until the host `child` says it is ready.
///
/// # Errors
///
/// When it ends before, or takes longer than [`READY_TIMEOUT`].
pub fn wait_ready(region: &Region, child: &mut Child) -> io::Result<()> {
    let until = Instant::now() + READY_TIMEOUT;
    while region.get(word::HOST_READY) == 0 {
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!(
                "the host ended before it was ready: {status}"
            )));
        }
        if Instant::now() > until {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the host did not get ready",
            ));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

/// What the engine thread brings back.
struct Played {
    out: Vec<f32>,
    insert_sum: Duration,
    insert_max: Duration,
    late_max: Duration,
    late_blocks: u64,
    realtime: bool,
}

/// Plays `blocks` blocks of the tone through `insert`, each when it is due.
fn play(mut insert: Insert, frames: usize, blocks: u64) -> Played {
    let pro = ProAudio::join();
    let timer = Timer::new();
    let period_ns = frames as u64 * 1_000_000_000 / u64::from(RATE);
    let mut tone = Sine::new(TONE, RATE, LEVEL);
    let mut block = vec![0.0; frames * CHANNELS];
    let mut played = Played {
        out: Vec::with_capacity(usize::try_from(blocks).unwrap_or(0) * frames * CHANNELS),
        insert_sum: Duration::ZERO,
        insert_max: Duration::ZERO,
        late_max: Duration::ZERO,
        late_blocks: 0,
        realtime: pro.is_some(),
    };
    let start = now_ns() + period_ns;
    for n in 0..blocks {
        let due = start + n * period_ns;
        let late = loop {
            let now = now_ns();
            if now >= due {
                break now - due;
            }
            let left = due - now;
            match &timer {
                // The timer may oversleep by about half a millisecond: the rest is spun.
                Some(timer) if left > 700_000 => {
                    timer.sleep(Duration::from_nanos(left - 600_000));
                }
                _ => std::hint::spin_loop(),
            }
        };
        played.late_max = played.late_max.max(Duration::from_nanos(late));
        if late > period_ns / 4 {
            played.late_blocks += 1;
        }
        tone.fill(&mut block, CHANNELS);
        let before = now_ns();
        insert.process(&mut block, before);
        let took = Duration::from_nanos(now_ns().saturating_sub(before));
        played.insert_sum += took;
        played.insert_max = played.insert_max.max(took);
        played.out.extend_from_slice(&block);
    }
    drop(pro);
    played
}

/// The host of a run and what the watchdog knows of it.
struct Watched<'a> {
    exe: &'a Path,
    name: &'a str,
    geometry: Geometry,
    region: &'a Region,
    stats: &'a InsertStats,
    child: Child,
    /// The heartbeat as last seen, and when it last moved.
    beat: u32,
    moved: Instant,
    /// What the host last said it would need to come back.
    state: u32,
    /// The heartbeat when the insert was first seen in bypass, until it is rearmed.
    bypass_beat: Option<u32>,
    cpu: Duration,
    restarts: u32,
}

impl Watched<'_> {
    /// One look at the host: notes its heartbeat and state, restarts it when it ended or hangs,
    /// and rearms the insert once a host is there again. True when it restarted the host.
    fn look(&mut self) -> io::Result<bool> {
        let beat = self.region.get(word::HOST_BEAT);
        if beat != self.beat {
            self.beat = beat;
            self.moved = Instant::now();
            self.state = self.region.get(word::HOST_STATE);
        }
        let bypassed = self.stats.bypassed.load(Ordering::Acquire);
        if bypassed && self.bypass_beat.is_none() {
            self.bypass_beat = Some(beat);
        }
        let gone = self.child.try_wait()?.is_some();
        if gone || self.moved.elapsed() > STALL {
            self.restart()?;
            return Ok(true);
        }
        // A host that only stumbled: it works again, so the insert may listen again.
        if let Some(since) = self.bypass_beat.filter(|_| bypassed)
            && beat.wrapping_sub(since) >= BEATS_BACK
        {
            self.rearm();
        }
        Ok(false)
    }

    /// Ends the host for good and starts a new one with the state of the old.
    fn restart(&mut self) -> io::Result<()> {
        self.reap();
        self.region.set(word::HOST_READY, 0);
        self.child = spawn_host(self.exe, self.name, self.geometry, self.state)?;
        self.restarts += 1;
        wait_ready(self.region, &mut self.child)?;
        self.beat = self.region.get(word::HOST_BEAT);
        self.moved = Instant::now();
        self.rearm();
        Ok(())
    }

    fn rearm(&mut self) {
        self.bypass_beat = None;
        self.stats.rearm.store(true, Ordering::Release);
    }

    /// Kills the host (its own child, by its handle), waits for its end and adds up its CPU time.
    fn reap(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.cpu += cpu_time(&self.child).unwrap_or_default();
    }

    /// Asks the host to end, and kills it when it does not within two seconds.
    fn finish(&mut self, event: &Event) {
        self.region.set(word::STOP, 1);
        event.set();
        let until = Instant::now() + Duration::from_secs(2);
        while Instant::now() < until && matches!(self.child.try_wait(), Ok(None)) {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.reap();
    }
}

/// Runs `plan` with the program `exe` as the host.
///
/// # Errors
///
/// When the shared memory, the event or a host cannot be made, or the engine thread fails.
pub fn run(exe: &Path, plan: &Plan) -> io::Result<Report> {
    let geometry = Geometry::new(CHANNELS, plan.frames, BLOCKS).map_err(io::Error::other)?;
    let name = unique_name();
    let region = Arc::new(Region::create(&name, geometry)?);
    region.stamp(geometry);
    let event = Arc::new(Event::create(&wake_name(&name))?);
    let wake = Arc::clone(&event);
    let (insert, stats) = Insert::new(
        Arc::clone(&region),
        geometry,
        RATE,
        Box::new(move || wake.set()),
    )
    .map_err(io::Error::other)?;

    let mut child = spawn_host(Path::new(exe), &name, geometry, plan.gain.to_bits())?;
    wait_ready(&region, &mut child)?;
    let mut watched = Watched {
        exe,
        name: &name,
        geometry,
        region: &region,
        stats: &stats,
        child,
        beat: region.get(word::HOST_BEAT),
        moved: Instant::now(),
        state: plan.gain.to_bits(),
        bypass_beat: None,
        cpu: Duration::ZERO,
        restarts: 0,
    };

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let blocks = (plan.seconds.max(0.0) * f64::from(RATE) / plan.frames as f64).ceil() as u64;
    let frames = plan.frames;
    let started = Instant::now();
    let engine = std::thread::Builder::new()
        .name("engine".to_owned())
        .spawn(move || play(insert, frames, blocks))?;

    let mut kill = plan.kill_at.map(|_| Kill::default());
    let mut killed: Option<Instant> = None;
    // Wet blocks played when the new host was ready: one more, and the insert is back.
    let mut wet_at_ready: Option<u64> = None;
    while !engine.is_finished() {
        std::thread::sleep(Duration::from_millis(1));
        if let Some(at) = plan.kill_at
            && killed.is_none()
            && started.elapsed().as_secs_f64() >= at
        {
            let _ = watched.child.kill();
            killed = Some(Instant::now());
        }
        let restarted = watched.look()?;
        if let (Some(kill), Some(at)) = (&mut kill, killed) {
            if restarted && kill.to_ready.is_none() {
                kill.to_ready = Some(at.elapsed());
                wet_at_ready = Some(stats.wet_blocks.get());
            }
            if kill.to_wet.is_none()
                && wet_at_ready.is_some_and(|before| stats.wet_blocks.get() > before)
            {
                kill.to_wet = Some(at.elapsed());
            }
        }
    }
    let played = engine
        .join()
        .map_err(|_| io::Error::other("the engine thread panicked"))?;
    let wall = started.elapsed();
    watched.finish(&event);

    let detector = Clicks::for_tone(TONE, RATE);
    Ok(Report {
        frames,
        blocks,
        wall,
        misses: stats.misses.get(),
        longest_run: stats.longest_run.load(Ordering::Relaxed),
        bypasses: stats.bypasses.get(),
        send_full: stats.send_full.get(),
        wake: std::array::from_fn(|bucket| region.get(word::WAKE_BUCKET + bucket)),
        wake_max_us: region.get(word::WAKE_MAX_US),
        insert_mean: played
            .insert_sum
            .checked_div(u32::try_from(blocks).unwrap_or(u32::MAX).max(1))
            .unwrap_or_default(),
        insert_max: played.insert_max,
        late_max: played.late_max,
        late_blocks: played.late_blocks,
        realtime: played.realtime,
        host_cpu: watched.cpu,
        restarts: watched.restarts,
        clicks: detector.find(&played.out, CHANNELS).len(),
        steepest: detector.steepest(&played.out, CHANNELS),
        kill,
    })
}

/// The reports as a Markdown table, one row per run, and a line per kill below it.
#[must_use]
pub fn table(reports: &[Report]) -> String {
    use std::fmt::Write as _;
    let mut text = String::from(
        "| frames | block ms | blocks | misses | per min | longest run | bypasses | wake <=100 µs \
         | <=250 | <=500 | <=1000 | >1000 | wake max µs | insert mean/max µs | pacer late max µs \
         (blocks) | host CPU % | restarts | clicks | steepest |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for r in reports {
        let minutes = r.wall.as_secs_f64() / 60.0;
        let woken: u32 = r.wake.iter().sum();
        let share = |count: u32| 100.0 * f64::from(count) / f64::from(woken.max(1));
        let _ = writeln!(
            text,
            "| {} | {:.2} | {} | {} | {:.1} | {} | {} | {:.2} % | {:.2} % | {:.2} % | {:.2} % | {:.3} % \
             | {} | {:.1} / {:.0} | {:.0} ({}) | {:.2} | {} | {} | {:.2} |",
            r.frames,
            r.frames as f64 * 1000.0 / f64::from(RATE),
            r.blocks,
            r.misses,
            r.misses as f64 / minutes.max(1e-9),
            r.longest_run,
            r.bypasses,
            share(r.wake[0]),
            share(r.wake[1]),
            share(r.wake[2]),
            share(r.wake[3]),
            share(r.wake[4]),
            r.wake_max_us,
            r.insert_mean.as_secs_f64() * 1e6,
            r.insert_max.as_secs_f64() * 1e6,
            r.late_max.as_secs_f64() * 1e6,
            r.late_blocks,
            100.0 * r.host_cpu.as_secs_f64() / r.wall.as_secs_f64().max(1e-9),
            r.restarts,
            r.clicks,
            r.steepest,
        );
    }
    for r in reports {
        if !r.realtime {
            let _ = writeln!(
                text,
                "\n{} frames: the engine thread did not get the Pro Audio class.",
                r.frames
            );
        }
        if let Some(kill) = r.kill {
            let ms = |time: Option<Duration>| {
                time.map_or_else(
                    || "never".to_owned(),
                    |t| format!("{:.1} ms", t.as_secs_f64() * 1e3),
                )
            };
            let _ = writeln!(
                text,
                "\n{} frames, host killed: new host ready after {}, wet again after {}.",
                r.frames,
                ms(kill.to_ready),
                ms(kill.to_wet),
            );
        }
    }
    text
}
