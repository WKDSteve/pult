//! Named marks in the user's session on Linux and macOS, what a named mutex is on Windows: a
//! file in a folder of the user's own, locked while its holder lives. The system lets go of the
//! lock when the process ends, however it ends, so nothing stale keeps anyone out.

use std::fs::{DirBuilder, File, OpenOptions, TryLockError};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

/// The folder for what belongs to this session of the user, from what the environment says: the
/// runtime folder the session has, or else one in the user's home. The temp folder is no choice,
/// others can make things in it.
fn folder_from(runtime: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    // The specification: a relative XDG_RUNTIME_DIR counts as not set.
    let runtime = runtime.filter(|folder| folder.is_absolute());
    runtime.or_else(|| Some(home?.join(".cache").join("pult")))
}

/// The path of the file `name` in the session's folder, which is made if need be (for the user
/// alone); `None` when there is no such place. A name may be the one Windows has for the mark
/// (`Local\pult-app.instance`), only its last part counts.
///
/// # Errors
///
/// When the folder cannot be made.
pub fn file(name: &str, extension: &str) -> io::Result<PathBuf> {
    let folder = folder_from(
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
        std::env::home_dir(),
    )
    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no folder for this session"))?;
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&folder)?;
    Ok(in_folder(&folder, name, extension))
}

fn in_folder(folder: &Path, name: &str, extension: &str) -> PathBuf {
    let short = name.rsplit(['\\', '/']).next().unwrap_or(name);
    folder.join(format!("{short}.{extension}"))
}

fn lock_file(name: &str) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(file(name, "lock")?)
}

/// A mark held while it lives.
#[derive(Debug)]
pub struct Mark {
    _lock: File,
}

impl Mark {
    /// Holds the mark `name` as the only one; `None` when someone holds it already. An error
    /// when it cannot be asked at all.
    ///
    /// # Errors
    ///
    /// What the system said about the file or its lock.
    pub fn claim(name: &str) -> io::Result<Option<Self>> {
        let file = lock_file(name)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _lock: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error),
        }
    }

    /// Holds the mark `name` along with others, and tells whether someone held it already.
    /// `None` when it cannot be held; then the next one does not see this one.
    #[must_use]
    pub fn join(name: &str) -> (Option<Self>, bool) {
        let Ok(file) = lock_file(name) else {
            return (None, false);
        };
        // Alone if it can be had alone; then it is opened up for those who come later.
        let held = matches!(file.try_lock(), Err(TryLockError::WouldBlock));
        match file.try_lock_shared() {
            Ok(()) => (Some(Self { _lock: file }), held),
            Err(_) => (None, held),
        }
    }
}

/// Whether someone holds the mark `name` as the only one, without taking part.
#[must_use]
pub fn claimed(name: &str) -> bool {
    let Ok(path) = file(name, "lock") else {
        return false;
    };
    File::open(path)
        .is_ok_and(|file| matches!(file.try_lock_shared(), Err(TryLockError::WouldBlock)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(what: &str) -> String {
        format!(r"Local\pult-test-{what}-{}", std::process::id())
    }

    /// Waits until nobody holds the mark `name`. A process another test starts just now has a
    /// copy of every open file until it runs its program, and a lock goes with the last copy.
    fn free(name: &str) -> bool {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while Mark::claim(name).expect("asked").is_none() {
            if std::time::Instant::now() > until {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        true
    }

    /// Takes the mark's file away when the test ends.
    struct Clean(String);

    impl Drop for Clean {
        fn drop(&mut self) {
            if let Ok(path) = file(&self.0, "lock") {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    #[test]
    fn the_folder_is_the_sessions_and_otherwise_in_the_home() {
        let home = || Some(PathBuf::from("/home/someone"));
        assert_eq!(
            folder_from(Some("/run/user/1000".into()), home()),
            Some(PathBuf::from("/run/user/1000"))
        );
        assert_eq!(
            folder_from(None, home()),
            Some(PathBuf::from("/home/someone/.cache/pult"))
        );
        assert_eq!(
            folder_from(Some("relative".into()), home()),
            Some(PathBuf::from("/home/someone/.cache/pult")),
            "a relative one counts as not set"
        );
        assert_eq!(folder_from(None, None), None);
    }

    #[test]
    fn a_name_of_windows_is_a_file_name_here() {
        assert_eq!(
            in_folder(Path::new("/run"), r"Local\pult-app.instance", "lock"),
            PathBuf::from("/run/pult-app.instance.lock")
        );
        assert_eq!(
            in_folder(Path::new("/run"), "a/../../b", "sock"),
            PathBuf::from("/run/b.sock")
        );
    }

    #[test]
    fn only_the_first_claim_holds_and_ends_with_its_holder() {
        let name = name("claim");
        let _clean = Clean(name.clone());
        assert!(!claimed(&name), "nobody yet");
        let first = Mark::claim(&name).expect("asked").expect("the first");
        assert!(claimed(&name));
        assert!(Mark::claim(&name).expect("asked").is_none());
        drop(first);
        assert!(free(&name), "the claim ends with its holder");
        assert!(!claimed(&name));
    }

    #[test]
    fn who_joins_sees_those_before_and_they_can_stay_together() {
        let name = name("join");
        let _clean = Clean(name.clone());
        let (first, held) = Mark::join(&name);
        assert!(first.is_some() && !held);
        let (second, held) = Mark::join(&name);
        assert!(second.is_some() && held);
        let (third, held) = Mark::join(&name);
        assert!(third.is_some() && held);
        drop((first, second));
        let (fourth, held) = Mark::join(&name);
        assert!(held, "the third is still there");
        drop((third, fourth));
        assert!(free(&name), "the mark goes with those who hold it");
        let (alone, held) = Mark::join(&name);
        assert!(alone.is_some() && !held);
    }

    #[test]
    fn a_process_that_ends_lets_go_of_its_claim() {
        // A child of this test holds the mark, as another pult would.
        let name = name("child");
        let _clean = Clean(name.clone());
        let held = Mark::claim(&name).expect("asked").expect("the first");
        drop(held);
        let path = file(&name, "lock").expect("a path");
        let mut holder = std::process::Command::new("sh")
            .arg("-c")
            .arg("exec 9<\"$0\"; flock -x 9 2>/dev/null || exit 3; echo held; exec sleep 60")
            .arg(&path)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("sh runs");
        let mut line = String::new();
        let told = std::io::BufRead::read_line(
            &mut std::io::BufReader::new(holder.stdout.take().expect("its output")),
            &mut line,
        );
        if told.is_err() || line.trim() != "held" {
            // No flock(1) here (macOS has none): nothing to learn from this run.
            let _ = holder.kill();
            let _ = holder.wait();
            return;
        }
        assert!(claimed(&name), "seen from another process");
        assert!(Mark::claim(&name).expect("asked").is_none());
        holder.kill().expect("ends");
        holder.wait().expect("ended");
        assert!(free(&name), "the system let go of it");
    }
}
