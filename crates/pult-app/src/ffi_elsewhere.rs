//! What `ffi` does on Windows, on Linux and macOS for now: the window runs, without the one
//! instance per session and without the start on signing in. A second start opens a second
//! window; turning the start on says that it is missing. Their real forms are the task
//! pult/linux-macos-gleichwertig.

use std::time::Duration;

/// The names `ffi` has on Windows, which `main` passes back here.
pub const INSTANCE: &str = pult_engine::APP_INSTANCE;
pub const SHOW: &str = "pult-app.show";
pub const RUN_KEY: &str = "";
pub const RUN_VALUE: &str = "pult";

/// A program started from a terminal has that terminal already.
pub fn attach_parent_console() {}

/// The claim of this process on being the one instance; nobody else can be seen yet.
#[derive(Debug)]
pub struct Instance;

/// Always succeeds: nothing keeps a second instance out yet.
#[expect(clippy::unnecessary_wraps, reason = "what `ffi` has on Windows")]
pub fn claim(_name: &str) -> Option<Instance> {
    Some(Instance)
}

/// Nobody can be asked yet.
pub fn ask_to_show(_name: &str, _patience: Duration) -> bool {
    false
}

/// Nobody can ask yet.
pub fn listen_for_show(_name: &str, _show: impl FnMut() + Send + 'static) -> bool {
    false
}

/// Turning the start on is refused, turning it off has nothing to do.
///
/// # Errors
///
/// That the start on signing in does not exist here yet.
pub fn set_run(_key: &str, _name: &str, command: Option<&str>) -> Result<(), String> {
    match command {
        Some(_) => Err("it is not built for this system yet".to_owned()),
        None => Ok(()),
    }
}

/// Nothing is set up to start.
#[must_use]
pub fn run_exists(_key: &str, _name: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_missing_says_so_instead_of_pretending() {
        assert!(claim(INSTANCE).is_some());
        assert!(!ask_to_show(SHOW, Duration::ZERO));
        assert!(!listen_for_show(SHOW, || {}));
        assert!(set_run(RUN_KEY, RUN_VALUE, Some("pult-app --tray")).is_err());
        assert_eq!(set_run(RUN_KEY, RUN_VALUE, None), Ok(()));
        assert!(!run_exists(RUN_KEY, RUN_VALUE));
    }
}
