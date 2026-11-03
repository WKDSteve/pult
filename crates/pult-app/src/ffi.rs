//! The one Windows call of the window that has no safe wrapper.
//!
//! This is the only module of `pult-app` with `unsafe` code; it belongs to the binary, the
//! library forbids `unsafe` outright.

use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};

/// Lets `println!` and `eprintln!` reach the terminal `pult-app` was started from.
///
/// A release build is a window program and gets no console of its own, so `--help` and errors
/// of the command line would go nowhere. Without a parent console (a double click, the tray)
/// this does nothing; output redirected to a file or pipe keeps going there.
pub fn attach_parent_console() {
    // SAFETY: AttachConsole takes no pointers and changes only which console the process
    // writes to; it fails, harmlessly, when there is no parent console or the process has one.
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}
