//! The Windows calls of the spike: a named file mapping as the shared memory, a named event to
//! wake the host, the clock both processes read, the realtime class of a thread, a timer that
//! sleeps shorter than a millisecond, and the CPU time of a child.
//!
//! This is the only module of the crate with `unsafe` code. Every block names the condition it
//! relies on; everything outside works with the safe types below.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::process::Child;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU32;
use std::time::Duration;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, FILETIME, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    WAIT_OBJECT_0,
};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateEventW, CreateWaitableTimerExW,
    EVENT_MODIFY_STATE, GetProcessTimes, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE, SetEvent,
    SetWaitableTimer, TIMER_ALL_ACCESS, WaitForSingleObject,
};
use windows::core::{HSTRING, PCWSTR, w};

/// A named file mapping, mapped into this process as words.
pub struct Mapping {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    words: usize,
}

// SAFETY: the mapping is only ever reached as a slice of atomics, which any thread may read and
// write; the handle and the view are plain kernel resources, freed once in `drop`.
#[allow(unsafe_code)]
unsafe impl Send for Mapping {}
// SAFETY: as above; nothing in it is tied to a thread.
#[allow(unsafe_code)]
unsafe impl Sync for Mapping {}

impl std::fmt::Debug for Mapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mapping")
            .field("words", &self.words)
            .finish_non_exhaustive()
    }
}

impl Mapping {
    /// A new mapping `name` of `words` words, zeroed by Windows.
    pub(crate) fn create(name: &str, words: usize) -> io::Result<Self> {
        let bytes = words * size_of::<AtomicU32>();
        let low = u32::try_from(bytes).map_err(|_| io::Error::other("the region is too large"))?;
        // SAFETY: no file behind it (INVALID_HANDLE_VALUE: the page file), no security
        // attributes, and a name that lives across the call.
        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                low,
                &HSTRING::from(name),
            )
        }?;
        // SAFETY: reads the thread's last error, set by the call just now.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // SAFETY: the handle came from the call above and is closed once.
            let _ = unsafe { CloseHandle(handle) };
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("a mapping named {name} exists already"),
            ));
        }
        Self::map(handle, words)
    }

    /// The mapping `name` another process made, which must hold `words` words.
    pub(crate) fn open(name: &str, words: usize) -> io::Result<Self> {
        // SAFETY: a name that lives across the call; the handle is not inherited.
        let handle =
            unsafe { OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, &HSTRING::from(name)) }?;
        Self::map(handle, words)
    }

    /// Maps `words` words of the mapping behind `handle`, which this takes over.
    fn map(handle: HANDLE, words: usize) -> io::Result<Self> {
        // SAFETY: `handle` is an open mapping. Windows refuses a view larger than the mapping,
        // so a view that comes back holds all the bytes asked for.
        let view = unsafe {
            MapViewOfFile(
                handle,
                FILE_MAP_ALL_ACCESS,
                0,
                0,
                words * size_of::<AtomicU32>(),
            )
        };
        if view.Value.is_null() {
            let error = io::Error::last_os_error();
            // SAFETY: the handle is open and closed once.
            let _ = unsafe { CloseHandle(handle) };
            return Err(error);
        }
        Ok(Self {
            handle,
            view,
            words,
        })
    }

    /// The mapping as words.
    pub(crate) fn words(&self) -> &[AtomicU32] {
        // SAFETY: the view is `words` words long (see `map`), page aligned, and stays mapped
        // until `drop`. An `AtomicU32` may be written through a shared reference, by this
        // process and, since every access to the mapping in both processes is an atomic one,
        // by the other.
        unsafe { std::slice::from_raw_parts(self.view.Value.cast::<AtomicU32>(), self.words) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: view and handle came from `map` and are given back once; no slice of the view
        // outlives `self`.
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}

/// A named event that resets itself: one `set` wakes one `wait`.
#[derive(Debug)]
pub struct Event(HANDLE);

// SAFETY: an event handle may be set and waited on from any thread.
#[allow(unsafe_code)]
unsafe impl Send for Event {}
// SAFETY: as above.
#[allow(unsafe_code)]
unsafe impl Sync for Event {}

impl Event {
    /// A new event `name`, not set.
    ///
    /// # Errors
    ///
    /// When Windows refuses it.
    pub fn create(name: &str) -> io::Result<Self> {
        // SAFETY: no security attributes and a name that lives across the call.
        let handle = unsafe { CreateEventW(None, false, false, &HSTRING::from(name)) }?;
        Ok(Self(handle))
    }

    /// The event `name` another process made.
    ///
    /// # Errors
    ///
    /// When there is none of that name.
    pub fn open(name: &str) -> io::Result<Self> {
        // SAFETY: a name that lives across the call; the handle is not inherited.
        let handle = unsafe {
            OpenEventW(
                EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE,
                false,
                &HSTRING::from(name),
            )
        }?;
        Ok(Self(handle))
    }

    /// Sets the event. It does not wait for anyone, so an audio callback may call it.
    pub fn set(&self) {
        // SAFETY: the handle is open for the lifetime of `self`.
        let _ = unsafe { SetEvent(self.0) };
    }

    /// Waits up to `timeout` for the event; true when it was set.
    #[must_use]
    pub fn wait(&self, timeout: Duration) -> bool {
        let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: the handle is open for the lifetime of `self`.
        unsafe { WaitForSingleObject(self.0, millis) == WAIT_OBJECT_0 }
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateEventW or OpenEventW and is closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Nanoseconds on the performance counter, which every process of the machine reads alike.
#[must_use]
pub fn now_ns() -> u64 {
    static FREQUENCY: OnceLock<u128> = OnceLock::new();
    let frequency = *FREQUENCY.get_or_init(|| {
        let mut ticks = 0_i64;
        // SAFETY: `ticks` is a valid place for the frequency.
        let _ = unsafe { QueryPerformanceFrequency(&raw mut ticks) };
        u128::try_from(ticks).unwrap_or(1).max(1)
    });
    let mut count = 0_i64;
    // SAFETY: `count` is a valid place for the counter.
    let _ = unsafe { QueryPerformanceCounter(&raw mut count) };
    let nanos = u128::try_from(count).unwrap_or(0) * 1_000_000_000 / frequency;
    u64::try_from(nanos).unwrap_or(u64::MAX)
}

/// Membership of the calling thread in the "Pro Audio" class of the multimedia scheduler,
/// given back on drop. The thread that joined must drop it.
#[derive(Debug)]
pub struct ProAudio(HANDLE);

impl ProAudio {
    /// Joins the class; `None` when Windows refuses.
    #[must_use]
    pub fn join() -> Option<Self> {
        let mut task_index = 0_u32;
        // SAFETY: the task name is a static NUL-terminated wide string and `task_index` a
        // valid place for the index Windows writes.
        unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &raw mut task_index) }
            .ok()
            .map(Self)
    }
}

impl Drop for ProAudio {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful AvSetMmThreadCharacteristicsW and is
        // given back once.
        let _ = unsafe { AvRevertMmThreadCharacteristics(self.0) };
    }
}

/// A timer that sleeps with the high resolution Windows 10 1803 and later offer (about half a
/// millisecond instead of up to 15.6 ms).
#[derive(Debug)]
pub struct Timer(HANDLE);

impl Timer {
    /// A timer; `None` where Windows has no high resolution timers.
    #[must_use]
    pub fn new() -> Option<Self> {
        // SAFETY: no security attributes and no name.
        unsafe {
            CreateWaitableTimerExW(
                None,
                PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
        }
        .ok()
        .map(Self)
    }

    /// Sleeps for about `time`.
    pub fn sleep(&self, time: Duration) {
        // In units of 100 ns; negative means from now.
        let due = -i64::try_from(time.as_nanos() / 100).unwrap_or(i64::MAX);
        // SAFETY: the handle is open for the lifetime of `self`, `due` lives across the call,
        // and there is no completion routine.
        unsafe {
            if SetWaitableTimer(self.0, &raw const due, 0, None, None, false).is_ok() {
                WaitForSingleObject(self.0, 1000);
            }
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateWaitableTimerExW and is closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// The CPU time `child` used so far, in kernel and user mode; also after it ended, while its
/// handle is held. `None` when Windows does not say.
#[must_use]
pub fn cpu_time(child: &Child) -> Option<Duration> {
    let handle = HANDLE(child.as_raw_handle());
    let mut times = [FILETIME::default(); 4];
    let [created, exited, kernel, user] = &mut times;
    // SAFETY: the handle is the child's, open while `child` lives; the four places are valid.
    unsafe { GetProcessTimes(handle, created, exited, kernel, user) }.ok()?;
    let ticks =
        |time: FILETIME| u64::from(time.dwLowDateTime) | (u64::from(time.dwHighDateTime) << 32);
    // In units of 100 ns.
    Some(Duration::from_nanos(
        (ticks(times[2]) + ticks(times[3])) * 100,
    ))
}
