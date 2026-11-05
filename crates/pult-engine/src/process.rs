//! The sound of one program as an input: process loopback, captured on a thread of its own.
//!
//! A program is picked by its executable's name like a device ("spotify" finds
//! `Spotify.exe`); the capture includes its child processes, so for a browser the main
//! process covers the audio of all its tabs. Windows delivers nothing while the program is
//! silent, so the thread fills such gaps with silence: the rings stay at their level and the
//! slot's stall watch keeps seeing callbacks. When the program exits, the stream reports a
//! fatal error and the slot looks for the program again by name.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use pult_core::{SelectError, select};

#[cfg(windows)]
use crate::ffi;
use crate::{Error, Role};

/// Channels of a process capture: Windows converts the program's sound to stereo.
pub(crate) const PROCESS_CHANNELS: u16 = 2;

/// How long the capture thread sleeps at most between looks for packets.
#[cfg_attr(
    not(windows),
    expect(
        dead_code,
        reason = "for the capture of the other systems, pult/linux-macos-gleichwertig"
    )
)]
const POLL: Duration = Duration::from_millis(10);

/// No packet for this long counts as silence, which is then filled in.
#[cfg_attr(
    not(windows),
    allow(
        dead_code,
        reason = "for the capture of the other systems, pult/linux-macos-gleichwertig"
    )
)]
const GAP: Duration = Duration::from_millis(30);

/// How long opening the capture may take.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// One process as [`pick`] sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Process {
    pub(crate) pid: u32,
    pub(crate) parent: u32,
    pub(crate) name: String,
}

/// The process `wanted` names, by executable name like a device name. Of several
/// processes with that name it takes the first whose parent is not one of them: the root
/// of the program's tree, which the capture includes with all its children.
pub(crate) fn pick(processes: &[Process], wanted: &str) -> Result<Process, SelectError> {
    let mut seen = HashSet::new();
    let names: Vec<&str> = processes
        .iter()
        .map(|p| p.name.as_str())
        .filter(|name| seen.insert(name.to_lowercase()))
        .collect();
    let name = names[select(wanted, &names)?].to_lowercase();
    let same: Vec<&Process> = processes
        .iter()
        .filter(|p| p.name.to_lowercase() == name)
        .collect();
    let root = same
        .iter()
        .find(|p| !same.iter().any(|q| q.pid == p.parent))
        .unwrap_or(&same[0]);
    Ok((*root).clone())
}

/// The names of all running programs, each once, sorted.
///
/// # Errors
///
/// [`Error::Process`] when the processes cannot be listed.
pub fn program_names() -> Result<Vec<String>, Error> {
    let mut seen = HashSet::new();
    let mut names: Vec<String> = processes()?
        .into_iter()
        .map(|p| p.name)
        .filter(|name| seen.insert(name.to_lowercase()))
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    Ok(names)
}

/// The running process `wanted` names.
pub(crate) fn find(wanted: &str) -> Result<Process, Error> {
    pick(&processes()?, wanted).map_err(|source| Error::Select {
        role: Role::Process,
        source,
    })
}

#[cfg(windows)]
fn processes() -> Result<Vec<Process>, Error> {
    let list = ffi::processes().map_err(|e| Error::Process {
        context: "listing programs".into(),
        message: e.message(),
    })?;
    Ok(list
        .into_iter()
        .map(|e| Process {
            pid: e.pid,
            parent: e.parent,
            name: e.name,
        })
        .collect())
}

#[cfg(not(windows))]
fn processes() -> Result<Vec<Process>, Error> {
    Err(Error::Process {
        context: "listing programs".into(),
        message: "capturing a program needs Windows".into(),
    })
}

/// Tracks when a capture last delivered, and how much silence a gap needs.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(
    not(windows),
    allow(
        dead_code,
        reason = "for the capture of the other systems, pult/linux-macos-gleichwertig"
    )
)]
struct Gaps {
    since: Instant,
    sample_rate: u32,
}

#[cfg_attr(
    not(windows),
    allow(
        dead_code,
        reason = "for the capture of the other systems, pult/linux-macos-gleichwertig"
    )
)]
impl Gaps {
    fn new(now: Instant, sample_rate: u32) -> Self {
        Self {
            since: now,
            sample_rate,
        }
    }

    /// Sound arrived at `now`.
    fn delivered(&mut self, now: Instant) {
        self.since = now;
    }

    /// Frames of silence owed at `now` when nothing arrived: none within [`GAP`], else the
    /// whole gap, at most half a second (after the system slept, say).
    fn owed(&mut self, now: Instant) -> usize {
        let gap = now.saturating_duration_since(self.since);
        if gap < GAP {
            return 0;
        }
        self.since = now;
        let frames = (gap.as_secs_f64() * f64::from(self.sample_rate)).round();
        // A non-negative value below the cap, so the cast neither wraps nor truncates.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let frames = frames.min(f64::from(self.sample_rate / 2)) as usize;
        frames
    }
}

/// A running capture of one program; dropping it stops the capture thread.
pub(crate) struct ProcessStream {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for ProcessStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl ProcessStream {
    /// Starts capturing process `pid` (and its children) at `sample_rate`. `data` gets
    /// interleaved stereo blocks, silence included; `fail` gets the error that ended the
    /// capture (the program exited, the audio system failed). Returns once the capture
    /// runs or failed to open.
    pub(crate) fn start(
        pid: u32,
        sample_rate: u32,
        data: impl FnMut(&[f32]) + Send + 'static,
        fail: impl FnMut(String) + Send + 'static,
    ) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready, opened) = mpsc::channel();
        let thread = {
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name(format!("pult process {pid}"))
                .spawn(move || capture(pid, sample_rate, &stop, &ready, data, fail))
                .map_err(|e| format!("cannot start the capture thread: {e}"))?
        };
        let mut stream = Self {
            stop,
            thread: Some(thread),
        };
        match opened.recv_timeout(OPEN_TIMEOUT + Duration::from_secs(1)) {
            Ok(Ok(())) => Ok(stream),
            Ok(Err(message)) => Err(message),
            Err(_) => {
                // Leave the thread to finish on its own rather than block on it.
                stream.stop.store(true, Ordering::Relaxed);
                stream.thread = None;
                Err("opening the capture timed out".into())
            }
        }
    }
}

/// The capture thread: opens the capture, reports that through `ready`, then hands packets
/// and filled-in silence to `data` until `stop` or an error.
#[cfg(windows)]
fn capture(
    pid: u32,
    sample_rate: u32,
    stop: &AtomicBool,
    ready: &mpsc::Sender<Result<(), String>>,
    mut data: impl FnMut(&[f32]),
    mut fail: impl FnMut(String),
) {
    let opened = ffi::Com::init().and_then(|com| {
        ffi::LoopbackCapture::open(pid, sample_rate, PROCESS_CHANNELS, OPEN_TIMEOUT)
            .map(|capture| (com, capture))
    });
    // `_com` must outlive the capture, so it is dropped last.
    let (_com, capture) = match opened {
        Ok(pair) => {
            let _ = ready.send(Ok(()));
            pair
        }
        Err(err) => {
            let _ = ready.send(Err(err.message()));
            return;
        }
    };
    let channels = usize::from(PROCESS_CHANNELS);
    // One poll of silence per call; longer gaps take several.
    let chunk = (sample_rate as usize / 100).max(1);
    let silence = vec![0.0_f32; chunk * channels];
    let silent = |mut frames: usize, data: &mut dyn FnMut(&[f32])| {
        while frames > 0 {
            let now = frames.min(chunk);
            data(&silence[..now * channels]);
            frames -= now;
        }
    };
    let mut gaps = Gaps::new(Instant::now(), sample_rate);
    while !stop.load(Ordering::Relaxed) {
        match capture.wait(POLL) {
            Ok(ffi::Wake::Exited) => {
                fail("the program exited".into());
                return;
            }
            Err(err) => {
                fail(err.message());
                return;
            }
            Ok(ffi::Wake::Ready | ffi::Wake::Timeout) => {}
        }
        let mut got = false;
        let drained = capture.drain(|packet| {
            got = true;
            match packet {
                ffi::Packet::Samples(samples) => data(samples),
                ffi::Packet::Silence(frames) => silent(frames, &mut data),
            }
        });
        if let Err(err) = drained {
            fail(err.message());
            return;
        }
        let now = Instant::now();
        if got {
            gaps.delivered(now);
        } else {
            silent(gaps.owed(now), &mut data);
        }
    }
}

#[cfg(not(windows))]
fn capture(
    _pid: u32,
    _sample_rate: u32,
    _stop: &AtomicBool,
    ready: &mpsc::Sender<Result<(), String>>,
    _data: impl FnMut(&[f32]),
    _fail: impl FnMut(String),
) {
    let _ = ready.send(Err("capturing a program needs Windows".into()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, parent: u32, name: &str) -> Process {
        Process {
            pid,
            parent,
            name: name.into(),
        }
    }

    #[test]
    fn pick_finds_the_root_of_a_program_tree() {
        let list = [
            process(4, 0, "System"),
            process(20, 10, "chrome.exe"),
            process(10, 1, "chrome.exe"),
            process(30, 10, "chrome.exe"),
            process(40, 1, "Spotify.exe"),
        ];
        assert_eq!(pick(&list, "chrome").map(|p| p.pid), Ok(10));
        assert_eq!(pick(&list, "SPOTIFY").map(|p| p.pid), Ok(40));
        assert!(matches!(
            pick(&list, "vlc"),
            Err(SelectError::NotFound { .. })
        ));
        // Two different programs match: ambiguous, each named once.
        let list = [process(1, 0, "game.exe"), process(2, 0, "gamebar.exe")];
        assert!(matches!(
            pick(&list, "game"),
            Err(SelectError::Ambiguous { candidates, .. }) if candidates.len() == 2
        ));
        assert_eq!(pick(&list, "game.exe").map(|p| p.pid), Ok(1));
    }

    #[test]
    fn gaps_owe_silence_only_after_a_pause() {
        let start = Instant::now();
        let mut gaps = Gaps::new(start, 48_000);
        assert_eq!(gaps.owed(start + Duration::from_millis(20)), 0);
        assert_eq!(gaps.owed(start + Duration::from_millis(40)), 1920);
        assert_eq!(gaps.owed(start + Duration::from_millis(50)), 0);
        gaps.delivered(start + Duration::from_millis(60));
        assert_eq!(gaps.owed(start + Duration::from_millis(80)), 0);
        assert_eq!(gaps.owed(start + Duration::from_secs(5)), 24_000);
    }
}
