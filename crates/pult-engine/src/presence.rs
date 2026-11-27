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

    /// Waits until nobody holds the mark `name` any more. On Linux and macOS a process another
    /// test starts just now holds a copy of the mark's file until it runs its program, and the
    /// lock goes with the last copy; a named mutex of Windows is no inherited handle.
    #[cfg(windows)]
    fn let_go(_name: &str) {}

    #[cfg(not(windows))]
    fn let_go(name: &str) {
        assert!(crate::session::free(name), "the mark {name} is let go");
    }

    /// Holds the mark `name` along with its runs for a moment from another process, as a process
    /// another test starts just then does with its copy of the file. Nothing when flock(1) is
    /// missing (macOS).
    #[cfg(not(windows))]
    fn linger(name: &str) -> Option<std::process::Child> {
        let path = crate::session::file(name, "lock").expect("a path");
        let mut holder = std::process::Command::new("sh")
            .arg("-c")
            .arg("exec 9<\"$0\"; flock -s 9 2>/dev/null || exit 3; echo held; exec sleep 0.3")
            .arg(&path)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .ok()?;
        let mut line = String::new();
        let told = std::io::BufRead::read_line(
            &mut std::io::BufReader::new(holder.stdout.take().expect("its output")),
            &mut line,
        );
        if told.is_err() || line.trim() != "held" {
            let _ = holder.kill();
            let _ = holder.wait();
            return None;
        }
        Some(holder)
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

        // The race of a process another test starts while the runs hold the mark, made sure of.
        #[cfg(not(windows))]
        let lingering = linger(&run);

        // The mark goes with the runs that hold it, and with every copy of them.
        drop(first);
        drop(second);
        let_go(&run);
        let open_window = open_a_window(&window);
        let alone = Presence::announce_as(&run, &window);
        assert!(alone.note().expect("a note").contains("window is open"));
        drop(open_window);
        drop(alone);
        let_go(&run);
        let_go(&window);
        assert_eq!(Presence::announce_as(&run, &window).note(), None);
        #[cfg(not(windows))]
        if let Some(mut holder) = lingering {
            let _ = holder.wait();
        }
        #[cfg(not(windows))]
        for name in [&run, &window] {
            let _ = std::fs::remove_file(crate::session::file(name, "lock").expect("a path"));
        }
    }
}
