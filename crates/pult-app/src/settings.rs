//! What the window remembers between runs, apart from any profile: the profile opened last,
//! whether pult starts with Windows, and whether the user saw the hint on the first close.
//!
//! The file is `%APPDATA%\pult\settings.toml`. A missing file is the defaults; an unreadable
//! one too, but then it is not written over, so nothing the user wrote by hand gets lost.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The settings of the window.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The profile opened last; a start without an argument opens it again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_profile: Option<PathBuf>,
    /// pult starts with Windows, into the tray (off unless the user turns it on).
    pub autostart: bool,
    /// The user saw the hint on the first close; later closes hide at once.
    pub told_about_tray: bool,
}

/// The settings file of this user: `%APPDATA%\pult\settings.toml`; `None` without `APPDATA`.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("pult").join("settings.toml"))
}

/// The settings kept in a file, and whether they may be written back there.
#[derive(Debug)]
pub struct Stored {
    pub settings: Settings,
    /// `None` when nothing may be written: no file name, or the file did not read.
    path: Option<PathBuf>,
}

impl Stored {
    /// Reads `path`; a missing file gives the defaults. The message says why the file did not
    /// read; then the defaults hold and the file is left as it is.
    #[must_use]
    pub fn load(path: Option<PathBuf>) -> (Self, Option<String>) {
        let Some(path) = path else {
            let note = "no APPDATA folder; settings are not kept".to_owned();
            return (Self::in_memory(), Some(note));
        };
        let read = match fs::read_to_string(&path) {
            Ok(text) => toml::from_str::<Settings>(&text).map_err(|e| e.to_string()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(err) => Err(err.to_string()),
        };
        match read {
            Ok(settings) => (
                Self {
                    settings,
                    path: Some(path),
                },
                None,
            ),
            Err(err) => {
                let note = format!(
                    "{} did not read, settings are not kept this run: {err}",
                    path.display()
                );
                (Self::in_memory(), Some(note))
            }
        }
    }

    /// Settings that are never written.
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            settings: Settings::default(),
            path: None,
        }
    }

    /// Changes the settings and writes them, when they changed and may be written.
    ///
    /// # Errors
    ///
    /// Why the file could not be written; the change is kept in memory anyway.
    pub fn change(&mut self, edit: impl FnOnce(&mut Settings)) -> Result<(), String> {
        let before = self.settings.clone();
        edit(&mut self.settings);
        match &self.path {
            Some(path) if self.settings != before => write(path, &self.settings)
                .map_err(|e| format!("could not write {}: {e}", path.display())),
            _ => Ok(()),
        }
    }
}

/// Writes next to `path` first and then replaces it, so a crash never leaves half a file.
fn write(path: &Path, settings: &Settings) -> io::Result<()> {
    let text = toml::to_string(settings).map_err(io::Error::other)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let next = path.with_extension("toml.new");
    fs::write(&next, text)?;
    fs::rename(&next, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of this test only, removed again at the end.
    struct Folder(PathBuf);

    impl Folder {
        fn new(what: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("pult-app-settings-{what}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }

        fn file(&self) -> PathBuf {
            self.0.join("pult").join("settings.toml")
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_missing_file_is_the_defaults_and_is_made_on_the_first_change() {
        let folder = Folder::new("missing");
        let (mut stored, note) = Stored::load(Some(folder.file()));
        assert_eq!(note, None);
        assert_eq!(stored.settings, Settings::default());
        assert!(!folder.file().exists(), "loading writes nothing");
        stored
            .change(|s| s.last_profile = Some(PathBuf::from(r"C:\studio\pult.toml")))
            .unwrap();
        let (again, _) = Stored::load(Some(folder.file()));
        assert_eq!(
            again.settings.last_profile.as_deref(),
            Some(Path::new(r"C:\studio\pult.toml"))
        );
        assert!(!folder.file().with_extension("toml.new").exists());
    }

    #[test]
    fn every_setting_comes_back() {
        let folder = Folder::new("round");
        let (mut stored, _) = Stored::load(Some(folder.file()));
        let wanted = Settings {
            last_profile: Some(PathBuf::from(r"D:\a b\pult.toml")),
            autostart: true,
            told_about_tray: true,
        };
        stored.change(|s| *s = wanted.clone()).unwrap();
        assert_eq!(Stored::load(Some(folder.file())).0.settings, wanted);
    }

    #[test]
    fn missing_and_unknown_keys_are_fine() {
        let folder = Folder::new("keys");
        fs::create_dir_all(folder.file().parent().unwrap()).unwrap();
        fs::write(folder.file(), "autostart = true\nfuture = 1\n").unwrap();
        let (stored, note) = Stored::load(Some(folder.file()));
        assert_eq!(note, None);
        assert!(stored.settings.autostart);
        assert_eq!(stored.settings.last_profile, None);
        assert!(!stored.settings.told_about_tray);
    }

    #[test]
    fn an_unreadable_file_is_left_alone() {
        let folder = Folder::new("broken");
        fs::create_dir_all(folder.file().parent().unwrap()).unwrap();
        fs::write(folder.file(), "autostart = maybe\n").unwrap();
        let (mut stored, note) = Stored::load(Some(folder.file()));
        assert!(note.unwrap().contains("not kept"));
        assert_eq!(stored.settings, Settings::default());
        stored.change(|s| s.autostart = true).unwrap();
        assert!(stored.settings.autostart, "the change holds for this run");
        assert_eq!(
            fs::read_to_string(folder.file()).unwrap(),
            "autostart = maybe\n"
        );
    }

    #[test]
    fn no_change_writes_nothing() {
        let folder = Folder::new("same");
        let (mut stored, _) = Stored::load(Some(folder.file()));
        stored.change(|s| s.autostart = false).unwrap();
        assert!(!folder.file().exists());
    }
}
