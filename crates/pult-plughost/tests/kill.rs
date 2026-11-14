//! The watchdog case with a real second process: the host is killed while a tone runs through
//! the insert. The insert must fall to bypass after eight misses, play dry without a click, and
//! play wet again, at the gain the old host had, once a new host is there.
//!
//! Nothing here depends on how fast the machine is: after each block the test waits until the
//! host has worked on it (its heartbeat moved) before it sends the next one.

#![cfg(windows)]

use std::path::Path;
use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use pult_core::{Clicks, Sine};
use pult_plughost::bench::{self, CHANNELS, LEVEL, RATE, TONE};
use pult_plughost::layout::word;
use pult_plughost::{BYPASS_AFTER, Event, Geometry, Insert, Region, now_ns};

const GAIN: f32 = 0.5;

struct Chain {
    region: Arc<Region>,
    insert: Insert,
    tone: Sine,
    block: Vec<f32>,
    out: Vec<f32>,
}

impl Chain {
    /// One block through the insert, then up to `patience` for the host to work on it.
    fn step(&mut self, patience: Duration) {
        self.tone.fill(&mut self.block, CHANNELS);
        let beat = self.region.get(word::HOST_BEAT);
        self.insert.process(&mut self.block, now_ns());
        self.out.extend_from_slice(&self.block);
        let until = Instant::now() + patience;
        while self.region.get(word::HOST_BEAT) == beat && Instant::now() < until {
            std::thread::yield_now();
        }
    }

    /// The largest magnitude in the last `blocks` blocks that went out.
    fn level_of_last(&self, blocks: usize) -> f32 {
        let from = self.out.len() - blocks * self.block.len();
        self.out[from..].iter().fold(0.0, |max, s| max.max(s.abs()))
    }
}

/// Kills `child` when the test ends, however it ends.
struct Reaped(Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_killed_host_is_bypassed_and_a_new_one_comes_back_with_its_state() {
    let exe = Path::new(env!("CARGO_BIN_EXE_pult-plughost"));
    let geometry = Geometry::new(CHANNELS, 128, bench::BLOCKS).expect("sizes");
    let name = bench::unique_name();
    let region = Arc::new(Region::create(&name, geometry).expect("a mapping"));
    region.stamp(geometry);
    let event = Arc::new(Event::create(&bench::wake_name(&name)).expect("an event"));
    let wake = Arc::clone(&event);
    let (insert, stats) = Insert::new(
        Arc::clone(&region),
        geometry,
        RATE,
        Box::new(move || wake.set()),
    )
    .expect("fits");
    let mut chain = Chain {
        region: Arc::clone(&region),
        insert,
        tone: Sine::new(TONE, RATE, LEVEL),
        block: vec![0.0; geometry.samples()],
        out: Vec::new(),
    };
    let long = Duration::from_secs(5);

    let mut host = Reaped(bench::spawn_host(exe, &name, geometry, GAIN.to_bits()).expect("a host"));
    bench::wait_ready(&region, &mut host.0).expect("ready");
    for _ in 0..300 {
        chain.step(long);
    }
    assert_eq!(stats.misses.get(), 0);
    assert_eq!(stats.wet_blocks.get(), 299);
    assert!((chain.level_of_last(100) - LEVEL * GAIN).abs() < 1e-3);
    assert_eq!(region.get(word::HOST_STATE), GAIN.to_bits());

    // The host dies. What it had finished still plays, then eight misses, then bypass.
    drop(host);
    for _ in 0..300 {
        chain.step(Duration::ZERO);
    }
    assert!(stats.bypassed.load(Ordering::Acquire));
    assert_eq!(stats.misses.get(), u64::from(BYPASS_AFTER));
    assert_eq!(stats.bypasses.get(), 1);
    assert!((chain.level_of_last(100) - LEVEL).abs() < 1e-3);

    // A new host with what the old one left behind, and the insert is told.
    region.set(word::HOST_READY, 0);
    let left_behind = region.get(word::HOST_STATE);
    let mut host = Reaped(bench::spawn_host(exe, &name, geometry, left_behind).expect("a host"));
    bench::wait_ready(&region, &mut host.0).expect("ready again");
    stats.rearm.store(true, Ordering::Release);
    for _ in 0..300 {
        chain.step(long);
    }
    assert!(!stats.bypassed.load(Ordering::Acquire));
    assert_eq!(stats.misses.get(), u64::from(BYPASS_AFTER));
    assert!((chain.level_of_last(100) - LEVEL * GAIN).abs() < 1e-3);

    let clicks = Clicks::for_tone(TONE, RATE).find(&chain.out, CHANNELS);
    assert_eq!(clicks, Vec::new());

    // Asked to end, the host ends by itself.
    region.set(word::STOP, 1);
    event.set();
    let until = Instant::now() + long;
    while matches!(host.0.try_wait(), Ok(None)) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    let ended = host.0.try_wait().expect("a status");
    assert!(
        ended.is_some_and(|code| code.success()),
        "the host ended with {ended:?}"
    );
}

#[test]
fn a_host_without_its_mapping_says_so_and_ends() {
    let exe = Path::new(env!("CARGO_BIN_EXE_pult-plughost"));
    let geometry = Geometry::new(CHANNELS, 64, bench::BLOCKS).expect("sizes");
    let mut child = bench::spawn_host(exe, "Local\\pult-plughost-no-such-mapping", geometry, 0)
        .expect("starts");
    let status = child.wait().expect("ends");
    assert_eq!(status.code(), Some(2));
}
