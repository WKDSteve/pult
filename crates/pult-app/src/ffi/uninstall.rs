//! The Windows calls of `pult-app --uninstall` (`crate::uninstall`): the question and the answer
//! in a window, the folders of the shell, the registry of the user, where a shortcut leads and
//! which programs run.
//!
//! Nothing here is specific to pult: the names and places come from the caller.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, HANDLE};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize, IPersistFile, STGM_READ,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Programs, IShellLinkW, IsUserAnAdmin, KF_FLAG_DEFAULT,
    SHGetKnownFolderPath, ShellLink,
};
use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_DEFBUTTON2, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK,
    MB_SETFOREGROUND, MB_YESNO, MessageBoxW,
};
use windows::core::{HSTRING, Interface, PWSTR};

/// The longest path asked for in one piece (the long form of Windows paths).
const PATH_ROOM: usize = 32_768;

/// Asks `question` in a window titled `title`; true when the answer is yes. "No" is the button
/// Enter takes.
pub fn ask(title: &str, question: &str) -> bool {
    let style = MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2 | MB_SETFOREGROUND;
    // SAFETY: both texts live across the call; the box has no owner window.
    unsafe { MessageBoxW(None, &HSTRING::from(question), &HSTRING::from(title), style) == IDYES }
}

/// Says `text` in a window titled `title`, as a problem when `problem`.
pub fn tell(title: &str, text: &str, problem: bool) {
    let icon = if problem {
        MB_ICONERROR
    } else {
        MB_ICONINFORMATION
    };
    // SAFETY: both texts live across the call; the box has no owner window.
    let _ = unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(text),
            &HSTRING::from(title),
            MB_OK | icon | MB_SETFOREGROUND,
        )
    };
}

/// Whether this process runs with the rights of an administrator.
#[must_use]
pub fn elevated() -> bool {
    // SAFETY: IsUserAnAdmin takes nothing and only reads the token of the process.
    unsafe { IsUserAnAdmin() }.as_bool()
}

/// A folder of the shell of the current user.
#[derive(Clone, Copy, Debug)]
pub enum Folder {
    /// Where the Start menu keeps its programs.
    Programs,
    Desktop,
}

/// Where `folder` is for the current user (it may have been moved, as a synced profile moves the desktop).
#[must_use]
pub fn folder(folder: Folder) -> Option<PathBuf> {
    let id = match folder {
        Folder::Programs => FOLDERID_Programs,
        Folder::Desktop => FOLDERID_Desktop,
    };
    // SAFETY: `id` lives across the call; the text Windows hands out is read before it is freed,
    // and freed exactly once.
    unsafe {
        let text = SHGetKnownFolderPath(&raw const id, KF_FLAG_DEFAULT, None).ok()?;
        let path = text.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(text.as_ptr().cast()));
        path
    }
}

/// The text of the value `name` under `HKEY_CURRENT_USER\<key>` (`name` empty: its default
/// value); `None` when there is none or it is no text.
#[must_use]
pub fn registry_text(key: &str, name: &str) -> Option<String> {
    let (key, name) = (HSTRING::from(key), HSTRING::from(name));
    let mut bytes = 0u32;
    // SAFETY: the strings live across the call; no data is asked for, so only `bytes` is written.
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            &name,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&raw mut bytes),
        )
    }
    .ok()
    .ok()?;
    let mut data = vec![0u16; (bytes as usize).div_ceil(2) + 1];
    bytes = u32::try_from(data.len() * 2).ok()?;
    // SAFETY: `data` holds `bytes` bytes; the strings live across the call.
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            &name,
            RRF_RT_REG_SZ,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&raw mut bytes),
        )
    }
    .ok()
    .ok()?;
    let len = data.iter().position(|&c| c == 0).unwrap_or(data.len());
    Some(String::from_utf16_lossy(&data[..len]))
}

/// Removes the value `name` under `HKEY_CURRENT_USER\<key>`; one that is not there is no error.
///
/// # Errors
///
/// What Windows said when the value is still there.
pub fn remove_registry_value(key: &str, name: &str) -> Result<(), String> {
    // SAFETY: both strings live across the call.
    let result =
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(name)) };
    if result == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    result.ok().map_err(|e| e.message())
}

/// Removes the key `HKEY_CURRENT_USER\<key>` with all below it; one that is not there is no
/// error.
///
/// # Errors
///
/// What Windows said when the key is still there, or that `key` names no key of its own.
pub fn remove_registry_key(key: &str) -> Result<(), String> {
    if key.trim_matches('\\').is_empty() {
        // An empty name would mean everything below HKEY_CURRENT_USER.
        return Err("no key named".to_owned());
    }
    // SAFETY: `key` lives across the call.
    let result = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key)) };
    if result == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    result.ok().map_err(|e| e.message())
}

/// The program the shortcut `link` (a `.lnk` file) starts; `None` when it cannot be read or
/// names none.
#[must_use]
pub fn shortcut_target(link: &Path) -> Option<PathBuf> {
    let link = HSTRING::from(link.as_os_str());
    // SAFETY: COM is set up on this thread for the length of the block and taken down only when
    // this call set it up; the interfaces are released (dropped) before that. `link` and `path`
    // live across the calls, and no find data is asked for (null).
    unsafe {
        let set_up = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let target = (|| {
            let shortcut: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            shortcut
                .cast::<IPersistFile>()
                .ok()?
                .Load(&link, STGM_READ)
                .ok()?;
            let mut path = vec![0u16; PATH_ROOM];
            shortcut.GetPath(&mut path, std::ptr::null_mut(), 0).ok()?;
            let len = path.iter().position(|&c| c == 0).unwrap_or(path.len());
            (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&path[..len])))
        })();
        if set_up.is_ok() {
            CoUninitialize();
        }
        target
    }
}

/// A handle closed when dropped.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful Create/Open call and is closed only here.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// The other processes of this session whose program is one of `names` (file names, letter case
/// does not count) and for which `ours` holds: the path of each and its process id. This process
/// is left out. A process that cannot be asked for its path is not ours to judge and left out.
#[must_use]
pub fn running(names: &[&str], ours: impl Fn(&Path) -> bool) -> Vec<(PathBuf, u32)> {
    let mut found = Vec::new();
    // SAFETY: a snapshot of the process list, read entry by entry into `entry`, which says its
    // own size; every handle is closed by `Owned`. Only the path of a program is asked for
    // (PROCESS_QUERY_LIMITED_INFORMATION), into `path` with its length in `len`.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return found;
        };
        let snapshot = Owned(snapshot);
        let mut entry = PROCESSENTRY32W {
            dwSize: u32::try_from(size_of::<PROCESSENTRY32W>()).unwrap_or(0),
            ..Default::default()
        };
        let mut more = Process32FirstW(snapshot.0, &raw mut entry).is_ok();
        while more {
            let pid = entry.th32ProcessID;
            let end = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
            let wanted =
                pid != std::process::id() && names.iter().any(|n| n.eq_ignore_ascii_case(&name));
            if wanted
                && let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
            {
                let process = Owned(process);
                let mut path = vec![0u16; PATH_ROOM];
                let mut len = u32::try_from(path.len()).unwrap_or(0);
                let asked = QueryFullProcessImageNameW(
                    process.0,
                    PROCESS_NAME_WIN32,
                    PWSTR(path.as_mut_ptr()),
                    &raw mut len,
                );
                if asked.is_ok() {
                    let path = PathBuf::from(String::from_utf16_lossy(&path[..len as usize]));
                    if ours(&path) {
                        found.push((path, pid));
                    }
                }
            }
            more = Process32NextW(snapshot.0, &raw mut entry).is_ok();
        }
    }
    found
}

/// Writes the text `text` as the value `name` under `HKEY_CURRENT_USER\<key>`, for a test.
#[cfg(test)]
pub fn write_registry_text(key: &str, name: &str, text: &str) {
    use windows::Win32::System::Registry::{REG_SZ, RegSetKeyValueW};
    let data: Vec<u16> = text.encode_utf16().chain([0]).collect();
    // SAFETY: the strings and `data` live across the call; the size is that of `data`.
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            &HSTRING::from(name),
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            u32::try_from(data.len() * 2).unwrap(),
        )
    }
    .ok()
    .unwrap();
}

/// The registry key of one test (see `ffi`), under the name `crate::uninstall` knows it by.
#[cfg(test)]
pub use super::TestPlace;

#[cfg(test)]
mod tests {
    use super::*;

    use write_registry_text as write;

    #[test]
    fn registry_text_is_read_and_removed() {
        let place = TestPlace::new("uninstall-registry");
        let key = format!(r"{}\below\entry", place.key());
        assert_eq!(registry_text(&key, "Name"), None);
        write(&key, "Name", r"C:\Übung macht\pult");
        write(&key, "", "the default");
        assert_eq!(
            registry_text(&key, "Name").as_deref(),
            Some(r"C:\Übung macht\pult")
        );
        assert_eq!(registry_text(&key, "").as_deref(), Some("the default"));
        remove_registry_value(&key, "Name").unwrap();
        remove_registry_value(&key, "Name").unwrap();
        assert_eq!(registry_text(&key, "Name"), None);
        assert_eq!(registry_text(&key, "").as_deref(), Some("the default"));
        remove_registry_key(&format!(r"{}\below", place.key())).unwrap();
        assert_eq!(registry_text(&key, ""), None);
        remove_registry_key(&format!(r"{}\below", place.key())).unwrap();
    }

    #[test]
    fn a_key_without_a_name_is_never_removed() {
        assert!(remove_registry_key("").is_err());
        assert!(remove_registry_key(r"\").is_err());
    }

    #[test]
    fn the_shell_says_where_its_folders_are() {
        assert!(folder(Folder::Programs).is_some_and(|p| p.is_absolute()));
        assert!(folder(Folder::Desktop).is_some_and(|p| p.is_absolute()));
    }

    #[test]
    fn a_file_that_is_no_shortcut_leads_nowhere() {
        let file =
            std::env::temp_dir().join(format!("pult-app-no-link-{}.lnk", std::process::id()));
        std::fs::write(&file, b"not a shortcut").unwrap();
        assert_eq!(shortcut_target(&file), None);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(shortcut_target(&file), None);
    }

    #[test]
    fn this_process_is_not_among_the_running() {
        let me = std::env::current_exe().unwrap();
        let name = me.file_name().unwrap().to_str().unwrap().to_owned();
        let found = running(&[&name], |_| true);
        assert!(found.iter().all(|(_, pid)| *pid != std::process::id()));
    }
}
