//! Whether another pult plays already: two routes on one device are heard twice, and nothing
//! in the sound says why. It only leads to a note, since runs on other devices are fine.

/// The name the window claims as its one instance in the user's session.
pub const APP_INSTANCE: &str = r"Local\pult-app.instance";

/// The name every `pult run` of the user's session holds while it runs.
const RUN_MARK: &str = r"Local\pult-run.active";

/// This process counted as a run while it lives, and who else was there when it started.
#[derive(Debug)]
pub struct Presence {
    #[cfg(windows)]
    _mark: Option<crate::ffi::SessionMark>,
    #[cfg(not(windows))]
    _mark: Option<crate::session::Mark>,
    other_run: bool,
    window: bool,
}

impl Presence {
    /// Marks this process as a run and looks for others. If the mark cannot be made, this run
    /// plays anyway, just without being seen by the next one.
    #[must_use]
    pub fn announce() -> Self {
        Self::announce_as(RUN_MARK, APP_INSTANCE)
    }

    #[cfg(windows)]
    fn announce_as(run: &str, window: &str) -> Self {
        let (mark, other_run) = crate::ffi::SessionMark::join(run);
        Self {
            _mark: mark,
            other_run,
            window: crate::ffi::SessionMark::exists(window),
        }
    }

    #[cfg(not(windows))]
    fn announce_as(run: &str, window: &str) -> Self {
        let (mark, other_run) = crate::session::Mark::join(run);
        Self {
            _mark: mark,
            other_run,
            window: crate::session::claimed(window),
        }
    }

    /// The line to show when another pult is there.
    #[must_use]
    pub fn note(&self) -> Option<&'static str> {
        match (self.other_run, self.window) {
            (true, _) => {
                Some("note: another pult run is active; on the same devices you hear both")
            }
            (false, true) => {
                Some("note: the pult window is open; if it plays on the same devices you hear both")
            }
            (false, false) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Holds the name of the window, as a window does.
    #[cfg(windows)]
    fn open_a_window(name: &str) -> impl Drop {
        crate::ffi::SessionMark::join(name).0.expect("a mark")
    }

    #[cfg(not(windows))]
    fn open_a_window(name: &str) -> std::fs::File {
        let path = crate::session::file(name, "lock").expect("a path");
        let file = std::fs::File::create(path).expect("a file");
        file.try_lock().expect("the lock");
        file
    }

    #[test]
    fn a_run_sees_the_runs_before_it_and_the_window() {
        let id = std::process::id();
        let run = format!(r"Local\pult-test-run-{id}");
        let window = format!(r"Local\pult-test-window-{id}");

        let first = Presence::announce_as(&run, &window);
        assert_eq!(first.note(), None);

        let second = Presence::announce_as(&run, &window);
        assert!(second.note().expect("a note").contains("another pult run"));

        // The mark goes with the runs that hold it.
        drop(first);
        drop(second);
        let open_window = open_a_window(&window);
        let alone = Presence::announce_as(&run, &window);
        assert!(alone.note().expect("a note").contains("window is open"));
        drop(open_window);
        drop(alone);
        assert_eq!(Presence::announce_as(&run, &window).note(), None);
        #[cfg(not(windows))]
        for name in [&run, &window] {
            let _ = std::fs::remove_file(crate::session::file(name, "lock").expect("a path"));
        }
    }
}
