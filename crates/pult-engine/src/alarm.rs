//! The device alarm: a wake-up call for the supervisor when an audio device comes or goes.
//!
//! Polling stays what finds a device ([`crate::Running::supervise`] looks for a lost one every
//! [`crate::slot::RETRY_EVERY`]); the alarm only brings that look forward. Under Windows a
//! device notification client (`IMMNotificationClient`, in `ffi`) sets an atomic and nothing
//! else; the supervisor takes it on its next call and lets every lost device look at once. When
//! the client cannot be registered, or on the other systems, the alarm never rings and
//! everything works as before, only up to a second slower.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Whether a device came or went since the supervisor last asked.
#[derive(Default)]
pub(crate) struct Alarm {
    rang: Arc<AtomicBool>,
    /// Keeps the notification client registered while the route runs.
    #[cfg(windows)]
    _watch: Option<crate::ffi::DeviceWatch>,
}

impl std::fmt::Debug for Alarm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Alarm")
            .field("rang", &self.rang.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl Alarm {
    /// An alarm that never rings: for a route without devices.
    pub(crate) fn silent() -> Self {
        Self::default()
    }

    /// An alarm the audio system rings, where it can; otherwise a silent one, since polling
    /// alone still finds every device.
    pub(crate) fn watching() -> Self {
        let rang = Arc::new(AtomicBool::new(false));
        #[cfg(windows)]
        {
            let watch = crate::ffi::DeviceWatch::start(Arc::clone(&rang)).ok();
            Self {
                rang,
                _watch: watch,
            }
        }
        #[cfg(not(windows))]
        {
            Self { rang }
        }
    }

    /// Whether it rang since the last call; asking takes the ring.
    pub(crate) fn rang(&self) -> bool {
        self.rang.swap(false, Ordering::AcqRel)
    }

    /// Rings it, as the notification client does.
    #[cfg(test)]
    pub(crate) fn ring(&self) {
        self.rang.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ring_is_taken_once() {
        let alarm = Alarm::silent();
        assert!(!alarm.rang());
        alarm.ring();
        alarm.ring();
        assert!(alarm.rang());
        assert!(!alarm.rang());
    }

    #[cfg(windows)]
    #[test]
    fn the_watch_registers_and_unregisters() {
        // Registering needs no device, only the audio service; dropping unregisters and ends
        // the thread (a hang would stop the test here).
        let watch = crate::ffi::DeviceWatch::start(Arc::new(AtomicBool::new(false)));
        if let Err(why) = &watch {
            panic!("no device notifications: {why}");
        }
        drop(watch);
    }
}
