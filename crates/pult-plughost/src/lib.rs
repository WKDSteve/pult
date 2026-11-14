//! A spike for plug-ins outside the process of pult: the engine's side and the host's side of
//! one insert point, with shared memory between them.
//!
//! Foreign plug-in code shall never run inside pult. A chain of plug-ins gets a process of its
//! own, `pult-plughost`; the audio callback of a bus writes each block into a ring in shared
//! memory, wakes the host and takes the block before as the host made it: one block of latency,
//! and never a wait. This crate is that way without any plug-in, a gain in its place, to measure
//! what the change of process costs and what happens when the host dies:
//!
//! - [`layout`]: what the shared memory looks like, word by word.
//! - [`region`]: the memory itself, a named mapping between two processes or plain memory inside
//!   one.
//! - [`ring`]: a ring of blocks for one writer and one reader that do not trust each other.
//! - [`insert`]: the engine's side, what an audio callback calls.
//! - [`host`]: the host's side.
//! - `bench` (Windows): starts a host, plays the engine at a fixed block size, kills the host on
//!   request, restarts it and reports misses, wake delays, CPU and clicks.
//!
//! Nothing of pult uses this crate yet. Mappings, events and the measurement exist for Windows
//! only so far; ring, insert and host are plain Rust and tested on every system.
//!
//! `unsafe` lives in the module `ffi` alone (the Windows calls); the rest of the crate denies it.

#![deny(unsafe_code)]

#[cfg(windows)]
pub mod bench;
#[cfg(windows)]
#[allow(unsafe_code)]
mod ffi;
pub mod host;
pub mod insert;
pub mod layout;
pub mod region;
pub mod ring;

#[cfg(windows)]
pub use ffi::{Event, ProAudio, now_ns};
pub use host::Host;
pub use insert::{BYPASS_AFTER, Insert, InsertStats};
pub use layout::Geometry;
pub use region::Region;
pub use ring::Ring;

/// Nanoseconds on a clock of this process. Enough where both sides live in one process; between
/// two, both must read one clock of the machine, which only the Windows build has so far.
#[cfg(not(windows))]
#[must_use]
pub fn now_ns() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    u64::try_from(START.get_or_init(Instant::now).elapsed().as_nanos()).unwrap_or(u64::MAX)
}
