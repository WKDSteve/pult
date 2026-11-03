//! The Windows calls of the window that have no safe wrapper: the console of the terminal,
//! the one instance per session, and the start with Windows.
//!
//! This is the only module of `pult-app` with `unsafe` code; it belongs to the binary, the
//! library forbids `unsafe` outright. Every block names the condition it relies on.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GetLastError, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent,
    WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
use windows::core::HSTRING;

/// The names of the instance in the session of the user (`Local\`): who runs, and how to ask
/// it to show its window.
pub const INSTANCE: &str = pult_engine::APP_INSTANCE;
pub const SHOW: &str = r"Local\pult-app.show";

/// Where Windows looks for what to start when the user signs in, and pult's value there.
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const RUN_VALUE: &str = "pult";

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

/// A handle closed when dropped.
#[derive(Debug)]
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful Create/Open call and is closed only here.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// The claim of this process on being the one instance, held while it lives.
#[derive(Debug)]
pub struct Instance {
    _mutex: Option<Owned>,
}

/// Claims the instance `name`; `None` when another process holds it already. When the claim
/// cannot be made at all, this process runs anyway, as before there was a claim.
pub fn claim(name: &str) -> Option<Instance> {
    // SAFETY: no security attributes (None) and a name that lives across the call; the mutex
    // is only named, never waited on, so nothing can deadlock on it.
    let created = unsafe { CreateMutexW(None, false, &HSTRING::from(name)) };
    let Ok(handle) = created else {
        return Some(Instance { _mutex: None });
    };
    let mutex = Owned(handle);
    // SAFETY: GetLastError only reads the thread's last error, set by CreateMutexW just now.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return None;
    }
    Some(Instance {
        _mutex: Some(mutex),
    })
}

/// Asks the instance listening on `name` to show its window and lets it come to the front;
/// false when none answered within `patience` (it may still be starting).
pub fn ask_to_show(name: &str, patience: Duration) -> bool {
    // SAFETY: AllowSetForegroundWindow takes no pointers; it may fail when this process may
    // not pass the foreground on, then the window shows behind others.
    let _ = unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    let name = HSTRING::from(name);
    let start = Instant::now();
    loop {
        // SAFETY: a name that lives across the call; the access right only allows SetEvent.
        if let Ok(handle) = unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &name) } {
            let event = Owned(handle);
            // SAFETY: the handle is open with EVENT_MODIFY_STATE until `event` drops.
            return unsafe { SetEvent(event.0) }.is_ok();
        }
        if start.elapsed() >= patience {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Calls `show` on a thread of its own each time another start asks this instance to show its
/// window (through `name`). False when the event cannot be made; then nobody can ask.
pub fn listen_for_show(name: &str, mut show: impl FnMut() + Send + 'static) -> bool {
    let name = HSTRING::from(name);
    let (ready, made) = mpsc::channel();
    // The event is made on the thread that waits on it, so its handle never leaves the thread.
    let spawned = thread::Builder::new()
        .name("pult-show".to_owned())
        .spawn(move || {
            // SAFETY: no security attributes, an auto-reset event that starts unset, a name
            // that lives across the call.
            let created = unsafe { CreateEventW(None, false, false, &name) };
            let _ = ready.send(created.is_ok());
            let Ok(handle) = created else {
                return;
            };
            let event = Owned(handle);
            loop {
                // SAFETY: `event` stays open for the whole loop; the thread ends with the
                // process.
                if unsafe { WaitForSingleObject(event.0, INFINITE) } != WAIT_OBJECT_0 {
                    break;
                }
                show();
            }
        });
    spawned.is_ok() && made.recv().unwrap_or(false)
}

/// Lets Windows start `command` when the user signs in (`HKEY_CURRENT_USER\<key>`, value
/// `name`), or with `None` no longer; removing a value that is not there is no error.
///
/// # Errors
///
/// What Windows said when the value could not be written or removed.
pub fn set_run(key: &str, name: &str, command: Option<&str>) -> Result<(), String> {
    let (key, name) = (HSTRING::from(key), HSTRING::from(name));
    let result = match command {
        Some(command) => {
            // REG_SZ data is the text in UTF-16 with its closing NUL, counted in bytes.
            let data: Vec<u16> = command.encode_utf16().chain([0]).collect();
            let bytes = u32::try_from(data.len() * 2).map_err(|e| e.to_string())?;
            // SAFETY: `key`, `name` and `data` live across the call, and `bytes` is the size
            // of `data`; a missing key is created.
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    &key,
                    &name,
                    REG_SZ.0,
                    Some(data.as_ptr().cast()),
                    bytes,
                )
            }
        }
        None => {
            // SAFETY: `key` and `name` live across the call.
            match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &key, &name) } {
                ERROR_FILE_NOT_FOUND => return Ok(()),
                other => other,
            }
        }
    };
    result.ok().map_err(|e| e.message())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Registry::{RRF_RT_REG_SZ, RegDeleteTreeW, RegGetValueW};

    /// A name of this test run only, so a running pult-app is not disturbed.
    fn name(what: &str) -> String {
        format!(r"Local\pult-app.test.{what}.{}", std::process::id())
    }

    #[test]
    fn only_the_first_claim_holds() {
        let name = name("claim");
        let first = claim(&name);
        assert!(first.is_some());
        assert!(claim(&name).is_none());
        drop(first);
        assert!(claim(&name).is_some(), "the claim ends with its holder");
    }

    #[test]
    fn a_second_start_reaches_the_first() {
        let name = name("show");
        let (send, shown) = mpsc::channel();
        assert!(listen_for_show(&name, move || {
            let _ = send.send(());
        }));
        assert!(ask_to_show(&name, Duration::from_secs(2)));
        assert!(shown.recv_timeout(Duration::from_secs(5)).is_ok());
        assert!(ask_to_show(&name, Duration::from_secs(2)));
        assert!(shown.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    /// The value `name` under `key`, as text.
    fn read_run(key: &str, name: &str) -> Option<String> {
        let (key, name) = (HSTRING::from(key), HSTRING::from(name));
        let mut data = [0u16; 512];
        let mut bytes = u32::try_from(data.len() * 2).unwrap();
        // SAFETY: `data` holds `bytes` bytes; the strings live across the call.
        let read = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &key,
                &name,
                RRF_RT_REG_SZ,
                None,
                Some(data.as_mut_ptr().cast()),
                Some(&raw mut bytes),
            )
        };
        read.ok().ok()?;
        let len = data.iter().position(|&c| c == 0).unwrap_or(data.len());
        Some(String::from_utf16_lossy(&data[..len]))
    }

    #[test]
    fn the_start_with_windows_is_written_and_removed() {
        // A key of this test run, never the real Run key.
        let key = format!(r"Software\pult-app-test-{}", std::process::id());
        let command = r#""C:\Program Files\pult\pult-app.exe" --tray"#;
        set_run(&key, "pult", Some(command)).unwrap();
        assert_eq!(read_run(&key, "pult").as_deref(), Some(command));
        set_run(&key, "pult", None).unwrap();
        assert_eq!(read_run(&key, "pult"), None);
        set_run(&key, "pult", None).unwrap();
        // SAFETY: the key belongs to this test.
        let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key.as_str())) };
    }

    #[test]
    fn nobody_listening_is_no_answer() {
        assert!(!ask_to_show(&name("nobody"), Duration::from_millis(100)));
    }
}
