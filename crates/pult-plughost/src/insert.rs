//! The engine's side of a chain: what an audio callback would call at an insert point.
//!
//! Per block it takes what the host made of block N-1, then writes block N for the host and
//! wakes it without waiting. In that order: a host that is woken may be done with block N before
//! the insert looks again, and what it made of N must then still lie there for the next call.
//! So the insert costs one block of latency, always, also while the host is away: then the dry
//! signal of block N-1 plays instead. It never waits, takes no lock and allocates nothing.
//!
//! A block that did not come back in time is a miss. After [`BYPASS_AFTER`] misses in a row the
//! insert goes to bypass and stays there until someone outside the callback says the host is
//! there again ([`InsertStats::rearm`]); meanwhile it keeps sending, so a host that only stumbled
//! shows by its heartbeat that it is back.
//!
//! Where the sound changes between wet and dry there is no second signal to cross over from: the
//! wet block is exactly what is missing. Instead the first frame is held where the previous output
//! was heading (the last frame plus its last step), and that offset dies away over
//! [`DECLICK_MS`]. Nothing jumps; what remains is a short, low bump.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use pult_core::Counter;

use crate::layout::{Geometry, word};
use crate::region::Region;
use crate::ring::Ring;

/// Misses in a row after which the insert goes to bypass.
pub const BYPASS_AFTER: u32 = 8;
/// The time constant of the offset that bridges a change between wet and dry, in milliseconds.
pub const DECLICK_MS: f32 = 2.0;
/// What comes back from the host is held within this, linear: the host is not trusted, and the
/// limiter of the bus should not meet a sample of any size.
pub const WET_LIMIT: f32 = 8.0;

/// What happened at one insert, counted in the callback and read outside.
#[derive(Debug, Default)]
pub struct InsertStats {
    /// Blocks that went through.
    pub blocks: Counter,
    /// Blocks that played what the host made.
    pub wet_blocks: Counter,
    /// Blocks whose result was not there in time, while not in bypass.
    pub misses: Counter,
    /// The most misses in a row so far.
    pub longest_run: AtomicU32,
    /// Times the insert went to bypass.
    pub bypasses: Counter,
    /// Blocks that could not be sent because the host did not read, while not in bypass.
    pub send_full: Counter,
    /// Whether the insert is in bypass.
    pub bypassed: AtomicBool,
    /// Set from outside the callback once the host is there again: the next block leaves bypass.
    pub rearm: AtomicBool,
}

/// One insert point, owned by the callback of its bus.
pub struct Insert {
    region: Arc<Region>,
    send: Ring,
    back: Ring,
    geometry: Geometry,
    stats: Arc<InsertStats>,
    /// Wakes the host; must not block.
    wake: Box<dyn Fn() + Send>,
    /// The number of the block the next call sends.
    sequence: u64,
    /// The block before, as it came in.
    dry: Vec<f32>,
    /// What the host made of a block.
    wet: Vec<f32>,
    /// The block going out.
    out: Vec<f32>,
    /// Per channel the offset that bridges the last change, dying away.
    offset: Vec<f32>,
    /// Per channel the last frame that went out, and the one before it.
    last: Vec<f32>,
    before_last: Vec<f32>,
    /// How much of the offset is left a frame later.
    decay: f32,
    was_wet: bool,
    bypassed: bool,
    misses_in_a_row: u32,
    /// The block before this one was not sent to a listening host: its result cannot be there.
    grace: bool,
}

impl std::fmt::Debug for Insert {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Insert")
            .field("geometry", &self.geometry)
            .field("sequence", &self.sequence)
            .field("bypassed", &self.bypassed)
            .finish_non_exhaustive()
    }
}

impl Insert {
    /// An insert over `region`, for blocks at `rate` frames per second; `wake` is called after
    /// each block is sent and must not block. Everything it will ever need is made here.
    ///
    /// # Errors
    ///
    /// When the region is too small for `geometry`.
    pub fn new(
        region: Arc<Region>,
        geometry: Geometry,
        rate: u32,
        wake: Box<dyn Fn() + Send>,
    ) -> Result<(Self, Arc<InsertStats>), String> {
        let send = Ring::new(Arc::clone(&region), geometry.send_base(), geometry)?;
        let back = Ring::new(Arc::clone(&region), geometry.return_base(), geometry)?;
        let stats = Arc::new(InsertStats::default());
        let samples = geometry.samples();
        let insert = Self {
            region,
            send,
            back,
            geometry,
            stats: Arc::clone(&stats),
            wake,
            sequence: 0,
            dry: vec![0.0; samples],
            wet: vec![0.0; samples],
            out: vec![0.0; samples],
            offset: vec![0.0; geometry.channels],
            last: vec![0.0; geometry.channels],
            before_last: vec![0.0; geometry.channels],
            decay: (-1.0 / (DECLICK_MS / 1000.0 * rate as f32)).exp(),
            was_wet: false,
            bypassed: false,
            misses_in_a_row: 0,
            grace: true,
        };
        Ok((insert, stats))
    }

    /// The latency of the insert in frames: one block.
    #[must_use]
    pub fn latency_frames(&self) -> usize {
        self.geometry.frames
    }

    /// Sends `block` to the host and replaces it by the block before: as the host made it, or dry
    /// where that is missing. `now_ns` is the time on the clock both processes read. A `block`
    /// that is not one block of the geometry is left as it is.
    pub fn process(&mut self, block: &mut [f32], now_ns: u64) {
        if block.len() != self.geometry.samples() {
            return;
        }
        self.region.bump(word::ENGINE_BEAT);
        self.stats.blocks.add(1);
        if self.bypassed && self.stats.rearm.swap(false, Ordering::AcqRel) {
            self.bypassed = false;
            self.stats.bypassed.store(false, Ordering::Release);
            self.misses_in_a_row = 0;
            self.grace = true;
        }
        // Before the host hears of this block: see the head of this file.
        let wet = self.take_previous() && !self.bypassed;
        if self.send.push(self.sequence, now_ns, block) {
            (self.wake)();
        } else if !self.bypassed {
            self.stats.send_full.add(1);
        }
        self.count(wet);
        self.sequence = self.sequence.wrapping_add(1);
        self.play(wet);
        self.dry.copy_from_slice(block);
        block.copy_from_slice(&self.out);
    }

    /// Reads what came back until the block before this one turns up in `self.wet`; older ones
    /// came too late and are dropped. Never more reads than a ring holds, whatever the host does.
    fn take_previous(&mut self) -> bool {
        for _ in 0..self.geometry.blocks {
            match self.back.pop(&mut self.wet) {
                Some((sequence, _)) if sequence.wrapping_add(1) == self.sequence => {
                    for sample in &mut self.wet {
                        *sample = if sample.is_finite() {
                            sample.clamp(-WET_LIMIT, WET_LIMIT)
                        } else {
                            0.0
                        };
                    }
                    return true;
                }
                Some(_) => {}
                None => return false,
            }
        }
        false
    }

    /// Counts this block as wet or as a miss, and goes to bypass after too many in a row.
    fn count(&mut self, wet: bool) {
        if self.bypassed {
            return;
        }
        if wet {
            self.stats.wet_blocks.add(1);
            self.misses_in_a_row = 0;
            self.grace = false;
        } else if std::mem::take(&mut self.grace) {
            // Nothing was sent before this block that could have come back.
        } else {
            self.stats.misses.add(1);
            self.misses_in_a_row += 1;
            self.stats
                .longest_run
                .fetch_max(self.misses_in_a_row, Ordering::Relaxed);
            if self.misses_in_a_row >= BYPASS_AFTER {
                self.bypassed = true;
                self.stats.bypassed.store(true, Ordering::Release);
                self.stats.bypasses.add(1);
            }
        }
    }

    /// Fills `self.out` from the wet or the dry block before, bridging a change between them.
    fn play(&mut self, wet: bool) {
        let channels = self.geometry.channels;
        let source: &[f32] = if wet { &self.wet } else { &self.dry };
        if wet != self.was_wet {
            let before = self.last.iter().zip(&self.before_last);
            for ((offset, first), (last, before_last)) in
                self.offset.iter_mut().zip(source).zip(before)
            {
                *offset = 2.0 * last - before_last - first;
            }
        }
        self.was_wet = wet;
        for (from, to) in source
            .chunks_exact(channels)
            .zip(self.out.chunks_exact_mut(channels))
        {
            for ((to, from), offset) in to.iter_mut().zip(from).zip(&mut self.offset) {
                *to = from + *offset;
                *offset *= self.decay;
            }
        }
        for offset in &mut self.offset {
            if offset.abs() < 1e-7 {
                *offset = 0.0;
            }
        }
        let samples = self.out.len();
        self.before_last
            .copy_from_slice(&self.out[samples - 2 * channels..samples - channels]);
        self.last.copy_from_slice(&self.out[samples - channels..]);
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // Samples that were only copied compare exactly.
mod tests {
    use std::sync::atomic::AtomicU64;

    use assert_no_alloc::{AllocDisabler, assert_no_alloc};
    use pult_core::{Clicks, Sine};

    use super::*;
    use crate::host::Host;

    #[cfg(debug_assertions)]
    #[global_allocator]
    static ALLOC: AllocDisabler = AllocDisabler;

    const RATE: u32 = 48_000;
    const TONE: f32 = 103.0;
    const LEVEL: f32 = 0.5;
    const GAIN: f32 = 0.5;

    struct Chain {
        insert: Insert,
        stats: Arc<InsertStats>,
        host: Host,
        tone: Sine,
        geometry: Geometry,
        woken: Arc<AtomicU64>,
        out: Vec<f32>,
    }

    fn chain(frames: usize) -> Chain {
        let geometry = Geometry::new(2, frames, 8).expect("sizes");
        let region = Arc::new(Region::local(geometry));
        region.stamp(geometry);
        let woken = Arc::new(AtomicU64::new(0));
        let count = Arc::clone(&woken);
        let (insert, stats) = Insert::new(
            Arc::clone(&region),
            geometry,
            RATE,
            Box::new(move || {
                count.fetch_add(1, Ordering::Relaxed);
            }),
        )
        .expect("fits");
        let host = Host::attach(region, geometry, Some(GAIN)).expect("attaches");
        Chain {
            insert,
            stats,
            host,
            tone: Sine::new(TONE, RATE, LEVEL),
            geometry,
            woken,
            out: Vec::new(),
        }
    }

    impl Chain {
        /// `blocks` blocks of the tone through the insert; the host works after each one while
        /// `host_works`.
        fn run(&mut self, blocks: usize, host_works: bool) {
            let mut block = vec![0.0; self.geometry.samples()];
            for _ in 0..blocks {
                self.tone.fill(&mut block, self.geometry.channels);
                assert_no_alloc(|| self.insert.process(&mut block, 0));
                self.out.extend_from_slice(&block);
                if host_works {
                    self.host.work(0);
                }
            }
        }

        /// The largest magnitude in the last `blocks` blocks that went out.
        fn level_of_last(&self, blocks: usize) -> f32 {
            let from = self.out.len() - blocks * self.geometry.samples();
            self.out[from..].iter().fold(0.0, |max, s| max.max(s.abs()))
        }

        fn clicks(&self) -> Vec<pult_core::Click> {
            Clicks::for_tone(TONE, RATE).find(&self.out, self.geometry.channels)
        }
    }

    #[test]
    fn the_wet_signal_comes_back_one_block_later() {
        let mut chain = chain(64);
        chain.run(200, true);
        // The tone at the host's gain, one block late.
        let mut tone = Sine::new(TONE, RATE, LEVEL * GAIN);
        let mut expected = vec![0.0; 199 * chain.geometry.samples()];
        tone.fill(&mut expected, 2);
        let from = chain.geometry.samples();
        // Behind the bump of the start, which has died away after 100 blocks.
        let late = 100 * chain.geometry.samples();
        for (i, (got, want)) in chain.out[from..]
            .iter()
            .zip(&expected)
            .enumerate()
            .skip(late)
        {
            assert!((got - want).abs() < 1e-5, "sample {i}: {got} for {want}");
        }
        assert_eq!(chain.insert.latency_frames(), 64);
        assert_eq!(chain.stats.misses.get(), 0);
        assert_eq!(chain.stats.wet_blocks.get(), 199);
        assert_eq!(chain.woken.load(Ordering::Relaxed), 200);
        assert_eq!(chain.clicks(), Vec::new());
    }

    #[test]
    fn a_host_that_is_done_before_the_insert_looks_again_loses_nothing() {
        // The fastest host there can be: it works inside the wake itself.
        let geometry = Geometry::new(2, 64, 8).expect("sizes");
        let region = Arc::new(Region::local(geometry));
        region.stamp(geometry);
        let host = Host::attach(Arc::clone(&region), geometry, Some(GAIN)).expect("attaches");
        let host = std::sync::Mutex::new(host);
        let (mut insert, stats) = Insert::new(
            region,
            geometry,
            RATE,
            Box::new(move || {
                if let Ok(mut host) = host.lock() {
                    host.work(0);
                }
            }),
        )
        .expect("fits");
        let mut tone = Sine::new(TONE, RATE, LEVEL);
        let mut block = vec![0.0; geometry.samples()];
        for _ in 0..200 {
            tone.fill(&mut block, geometry.channels);
            insert.process(&mut block, 0);
        }
        assert_eq!(stats.misses.get(), 0);
        assert_eq!(stats.wet_blocks.get(), 199);
        let level = block.iter().fold(0.0_f32, |max, s| max.max(s.abs()));
        assert!((level - LEVEL * GAIN).abs() < 1e-3, "{level}");
    }

    #[test]
    fn a_host_that_falls_silent_is_bypassed_after_eight_misses_without_a_click() {
        let mut chain = chain(64);
        chain.run(300, true);
        chain.run(7, false);
        // The last result was still there for the first of them.
        assert_eq!(chain.stats.misses.get(), 6);
        assert!(!chain.stats.bypassed.load(Ordering::Acquire));
        chain.run(300, false);
        assert_eq!(chain.stats.misses.get(), u64::from(BYPASS_AFTER));
        assert_eq!(chain.stats.bypasses.get(), 1);
        assert!(chain.stats.bypassed.load(Ordering::Acquire));
        assert_eq!(
            chain.stats.longest_run.load(Ordering::Relaxed),
            BYPASS_AFTER
        );
        // Dry now: the tone at its own level.
        assert!((chain.level_of_last(100) - LEVEL).abs() < 1e-3);
        assert_eq!(chain.clicks(), Vec::new());
    }

    #[test]
    fn a_rearmed_insert_plays_wet_again_without_a_click() {
        let mut chain = chain(128);
        chain.run(200, true);
        chain.run(200, false);
        assert!(chain.stats.bypassed.load(Ordering::Acquire));
        // The host is back, but nobody said so: still dry, and no more misses.
        let misses = chain.stats.misses.get();
        chain.run(100, true);
        assert!(chain.stats.bypassed.load(Ordering::Acquire));
        assert!((chain.level_of_last(50) - LEVEL).abs() < 1e-3);
        chain.stats.rearm.store(true, Ordering::Release);
        chain.run(200, true);
        assert!(!chain.stats.bypassed.load(Ordering::Acquire));
        assert_eq!(chain.stats.misses.get(), misses);
        assert!((chain.level_of_last(100) - LEVEL * GAIN).abs() < 1e-3);
        assert_eq!(chain.clicks(), Vec::new());
    }

    #[test]
    fn single_misses_bridge_without_a_click_and_do_not_bypass() {
        let mut chain = chain(256);
        chain.run(50, true);
        for _ in 0..20 {
            chain.run(3, false);
            chain.run(17, true);
        }
        assert!(!chain.stats.bypassed.load(Ordering::Acquire));
        assert_eq!(chain.stats.bypasses.get(), 0);
        assert!(chain.stats.misses.get() >= 40);
        assert_eq!(chain.clicks(), Vec::new());
    }

    #[test]
    fn without_the_bridge_a_miss_is_a_click() {
        // What the offset is there for: the same change between wet and dry, played as it comes.
        let mut chain = chain(64);
        chain.run(301, true);
        let wet_end = chain.out.len();
        chain.run(3, false);
        let mut hard = chain.out[..wet_end].to_vec();
        let mut tone = Sine::new(TONE, RATE, LEVEL);
        let mut dry = vec![0.0; (301 + 3) * chain.geometry.samples()];
        tone.fill(&mut dry, 2);
        // Dry of the block before, from where the wet ends.
        hard.extend_from_slice(&dry[wet_end - chain.geometry.samples()..][..128]);
        assert!(
            !Clicks::for_tone(TONE, RATE).find(&hard, 2).is_empty(),
            "the probe finds the jump the bridge takes away"
        );
    }

    #[test]
    fn what_a_host_gone_wrong_sends_stays_within_bounds() {
        let mut chain = chain(64);
        chain.run(10, true);
        // The host answers the next block with nonsense.
        let mut block = vec![0.0; chain.geometry.samples()];
        chain.tone.fill(&mut block, 2);
        chain.insert.process(&mut block, 0);
        let back = Ring::new(
            Arc::clone(&chain.insert.region),
            chain.geometry.return_base(),
            chain.geometry,
        )
        .expect("fits");
        let mut nonsense = vec![f32::NAN; chain.geometry.samples()];
        nonsense[1] = f32::INFINITY;
        nonsense[2] = 1e30;
        assert!(back.push(chain.insert.sequence - 1, 0, &nonsense));
        chain.insert.process(&mut block, 0);
        assert!(
            block
                .iter()
                .all(|s| s.is_finite() && s.abs() <= 2.0 * WET_LIMIT)
        );
        // A wrong block length changes nothing and counts nothing.
        let blocks = chain.stats.blocks.get();
        let mut short = vec![0.25; 10];
        chain.insert.process(&mut short, 0);
        assert!(short.iter().all(|&s| s == 0.25));
        assert_eq!(chain.stats.blocks.get(), blocks);
    }
}
