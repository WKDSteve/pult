//! What `ffi` does on Windows, on Linux and macOS. The start on signing in is a file: an autostart
//! entry on Linux (`~/.config/autostart`), a launch agent on macOS (`~/Library/LaunchAgents`),
//! which the system reads when the user signs in next. The one instance per session is a locked
//! file (`pult_engine::session`), and a later start asks it to show its window through a socket
//! beside that file.

use std::fs;
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use pult_engine::session::{self, Mark};

use pult_app::settings::StartMode;

/// The names `ffi` has on Windows, which `main` passes back here.
pub const INSTANCE: &str = pult_engine::APP_INSTANCE;
pub const SHOW: &str = "pult-app.show";
/// The folder of the start on signing in; empty is the one the system reads.
pub const RUN_KEY: &str = "";
/// A launch agent is known by its label, a reversed domain by custom.
pub const RUN_VALUE: &str = if MACOS {
    "io.github.wkdsteve.pult"
} else {
    "pult"
};

const MACOS: bool = cfg!(target_os = "macos");

/// A program started from a terminal has that terminal already.
pub fn attach_parent_console() {}

/// The claim of this process on being the one instance, held while it lives.
#[derive(Debug)]
pub struct Instance {
    _mark: Option<Mark>,
}

/// Claims the instance `name`; `None` when another process holds it already. When the claim
/// cannot be made at all, this process runs anyway, as before there was a claim.
pub fn claim(name: &str) -> Option<Instance> {
    match Mark::claim(name) {
        Ok(Some(mark)) => Some(Instance { _mark: Some(mark) }),
        Ok(None) => None,
        Err(_) => Some(Instance { _mark: None }),
    }
}

/// Asks the instance listening on `name` to show its window; false when none answered within
/// `patience` (it may still be starting).
pub fn ask_to_show(name: &str, patience: Duration) -> bool {
    let Ok(path) = session::file(name, "sock") else {
        return false;
    };
    let start = Instant::now();
    loop {
        // Being reached is the question; there is nothing else to say.
        if UnixStream::connect(&path).is_ok() {
            return true;
        }
        if start.elapsed() >= patience {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Calls `show` on a thread of its own each time another start asks this instance to show its
/// window (through `name`). False when the socket cannot be made; then nobody can ask.
pub fn listen_for_show(name: &str, mut show: impl FnMut() + Send + 'static) -> bool {
    let Ok(path) = session::file(name, "sock") else {
        return false;
    };
    // Only the holder of the instance listens, so a socket that is there is one a process before
    // this one left behind.
    let _ = fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else {
        return false;
    };
    thread::Builder::new()
        .name("pult-show".to_owned())
        .spawn(move || {
            // The thread ends with the process.
            for asked in listener.incoming() {
                if asked.is_ok() {
                    show();
                }
            }
        })
        .is_ok()
}

/// Where the system looks for what to start on signing in, from what the environment says.
fn system_folder(
    macos: bool,
    home: Option<PathBuf>,
    xdg_config: Option<PathBuf>,
) -> Option<PathBuf> {
    if macos {
        return Some(home?.join("Library").join("LaunchAgents"));
    }
    // The specification: a relative XDG_CONFIG_HOME counts as not set.
    let config = xdg_config
        .filter(|folder| folder.is_absolute())
        .or_else(|| Some(home?.join(".config")))?;
    Some(config.join("autostart"))
}

/// The file of the start `name` in `key`, the folder the system reads when `key` is empty.
fn run_file(key: &str, name: &str) -> Result<PathBuf, String> {
    let folder = if key.is_empty() {
        system_folder(
            MACOS,
            std::env::home_dir(),
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        )
        .ok_or("there is no home folder to put it in")?
    } else {
        PathBuf::from(key)
    };
    let extension = if MACOS { "plist" } else { "desktop" };
    Ok(folder.join(format!("{name}.{extension}")))
}

/// `exe` as one quoted word of an `Exec` line of a desktop entry: the quoting rule of its
/// specification first, the escaping of a string value over that, `%` being a field code.
fn exec_word(exe: &str) -> String {
    let mut word = String::from("\"");
    for c in exe.chars() {
        match c {
            '"' | '`' | '$' => {
                word.push_str(r"\\");
                word.push(c);
            }
            '\\' => word.push_str(r"\\\\"),
            '%' => word.push_str("%%"),
            _ => word.push(c),
        }
    }
    word.push('"');
    word
}

/// The autostart entry that starts `exe` the way `mode` says.
fn desktop_entry(exe: &str, mode: StartMode) -> String {
    let (comment, argument) = match mode {
        StartMode::Tray => (", into the tray", " --tray"),
        StartMode::Window => ("", ""),
    };
    format!(
        "[Desktop Entry]\nType=Application\nName=pult\nComment=The audio mixer{comment}\n\
         Exec={}{argument}\nTerminal=false\n",
        exec_word(exe)
    )
}

/// `text` as XML character data.
fn xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The launch agent `label` that starts `exe` the way `mode` says.
fn launch_agent(label: &str, exe: &str, mode: StartMode) -> String {
    let argument = match mode {
        StartMode::Tray => "\t\t<string>--tray</string>\n",
        StartMode::Window => "",
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \t<key>Label</key>\n\t<string>{}</string>\n\
         \t<key>ProgramArguments</key>\n\t<array>\n\
         \t\t<string>{}</string>\n{argument}\t</array>\n\
         \t<key>RunAtLoad</key>\n\t<true/>\n\
         \t<key>ProcessType</key>\n\t<string>Interactive</string>\n\
         </dict>\n</plist>\n",
        xml_text(label),
        xml_text(exe)
    )
}

/// The way an autostart entry or a launch agent starts: an `Exec` line ending in the argument (the
/// path before it is in quotes) or the argument as a string of its own.
fn mode_of(text: &str) -> StartMode {
    let tray = text.contains("<string>--tray</string>")
        || text
            .lines()
            .any(|line| line.starts_with("Exec=") && line.ends_with(" --tray"));
    if tray {
        StartMode::Tray
    } else {
        StartMode::Window
    }
}

/// Writes `file` whole or not at all, with the folder it needs.
fn write_whole(file: &Path, text: &str) -> io::Result<()> {
    if let Some(folder) = file.parent() {
        fs::create_dir_all(folder)?;
    }
    let beside = file.with_extension("new");
    fs::write(&beside, text)?;
    fs::rename(&beside, file).inspect_err(|_| {
        let _ = fs::remove_file(&beside);
    })
}

/// Lets the system start the program `exe` the way `mode` says when the user signs in (a file of
/// `name` in the folder `key`), or with `None` no longer; removing what is not there is no error.
///
/// # Errors
///
/// What the system said when the file could not be written or removed, or that a line break in
/// the path cannot be written into it.
pub fn set_run(key: &str, name: &str, start: Option<(&str, StartMode)>) -> Result<(), String> {
    let file = run_file(key, name)?;
    let Some((exe, mode)) = start else {
        return match fs::remove_file(&file) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    };
    if exe.contains(['\n', '\r']) {
        return Err("the path of pult-app has a line break".to_owned());
    }
    let text = if MACOS {
        launch_agent(name, exe, mode)
    } else {
        desktop_entry(exe, mode)
    };
    write_whole(&file, &text).map_err(|e| e.to_string())
}

/// How the system starts what it has under that name; `None` when it has nothing. A file that
/// does not read counts as what pult writes by default.
#[must_use]
pub fn run_start(key: &str, name: &str) -> Option<StartMode> {
    let file = run_file(key, name).ok()?;
    if !file.exists() {
        return None;
    }
    Some(fs::read_to_string(&file).map_or_else(|_| StartMode::default(), |text| mode_of(&text)))
}

/// A place for the start on signing in that belongs to one test, removed with it: never the
/// folder the system reads.
#[cfg(test)]
pub struct TestPlace(String);

#[cfg(test)]
impl TestPlace {
    pub fn new(what: &str) -> Self {
        let folder =
            std::env::temp_dir().join(format!("pult-app-run-{what}-{}", std::process::id()));
        Self(
            folder
                .to_str()
                .expect("a temp folder in Unicode")
                .to_owned(),
        )
    }

    pub fn key(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
impl Drop for TestPlace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name of this test run only, so a running pult-app is not disturbed; its files go at
    /// the end.
    struct Name(String);

    impl Name {
        fn new(what: &str) -> Self {
            Self(format!(
                r"Local\pult-app.test.{what}.{}",
                std::process::id()
            ))
        }
    }

    impl Drop for Name {
        fn drop(&mut self) {
            for extension in ["lock", "sock"] {
                if let Ok(path) = session::file(&self.0, extension) {
                    let _ = fs::remove_file(path);
                }
            }
        }
    }

    #[test]
    fn only_the_first_claim_holds() {
        let name = Name::new("claim");
        let first = claim(&name.0);
        assert!(first.is_some());
        assert!(claim(&name.0).is_none());
        drop(first);
        // A process started just now holds a copy of the lock until it runs its program.
        let until = Instant::now() + Duration::from_secs(5);
        while claim(&name.0).is_none() {
            assert!(Instant::now() < until, "the claim ends with its holder");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_second_start_reaches_the_first() {
        let name = Name::new("show");
        let (send, shown) = std::sync::mpsc::channel();
        assert!(listen_for_show(&name.0, move || {
            let _ = send.send(());
        }));
        assert!(ask_to_show(&name.0, Duration::from_secs(2)));
        assert!(shown.recv_timeout(Duration::from_secs(5)).is_ok());
        assert!(ask_to_show(&name.0, Duration::from_secs(2)));
        assert!(shown.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn nobody_listening_is_no_answer() {
        assert!(!ask_to_show(
            &Name::new("nobody").0,
            Duration::from_millis(100)
        ));
    }

    #[test]
    fn a_socket_left_behind_is_no_answer_and_does_not_keep_the_next_one_out() {
        let name = Name::new("stale");
        let path = session::file(&name.0, "sock").expect("a path");
        drop(UnixListener::bind(&path).expect("a socket"));
        assert!(path.exists(), "it stays when its process goes");
        assert!(!ask_to_show(&name.0, Duration::from_millis(100)));
        let (send, shown) = std::sync::mpsc::channel();
        assert!(listen_for_show(&name.0, move || {
            let _ = send.send(());
        }));
        assert!(ask_to_show(&name.0, Duration::from_secs(2)));
        assert!(shown.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn the_start_is_written_and_removed() {
        let folder = TestPlace::new("start");
        assert_eq!(run_start(folder.key(), "pult"), None, "no folder yet");
        set_run(folder.key(), "pult", None).expect("nothing to remove is no error");
        set_run(
            folder.key(),
            "pult",
            Some(("/opt/pult/pult-app", StartMode::Tray)),
        )
        .expect("written");
        assert_eq!(run_start(folder.key(), "pult"), Some(StartMode::Tray));
        assert_eq!(run_start(folder.key(), "other"), None);
        let file = run_file(folder.key(), "pult").expect("a file");
        let text = fs::read_to_string(&file).expect("readable");
        assert!(text.contains("/opt/pult/pult-app"), "{text}");
        assert!(text.contains("--tray"), "{text}");
        let files = fs::read_dir(folder.key()).expect("the folder").count();
        assert_eq!(files, 1, "nothing is left beside it");
        set_run(
            folder.key(),
            "pult",
            Some(("/usr/bin/pult-app", StartMode::Window)),
        )
        .expect("written again");
        let text = fs::read_to_string(&file).expect("readable");
        assert!(
            text.contains("/usr/bin/pult-app") && !text.contains("/opt"),
            "{text}"
        );
        assert!(!text.contains("--tray"), "{text}");
        assert_eq!(run_start(folder.key(), "pult"), Some(StartMode::Window));
        set_run(folder.key(), "pult", None).expect("removed");
        assert_eq!(run_start(folder.key(), "pult"), None);
    }

    #[test]
    fn a_line_break_in_the_path_cannot_add_lines_to_the_file() {
        let folder = TestPlace::new("break");
        let refused = set_run(
            folder.key(),
            "pult",
            Some(("/a\nExec=/bin/other", StartMode::Tray)),
        );
        assert!(refused.is_err());
        assert_eq!(run_start(folder.key(), "pult"), None);
    }

    #[test]
    fn linux_reads_its_config_folder_and_macos_its_launch_agents() {
        let home = || Some(PathBuf::from("/home/someone"));
        assert_eq!(
            system_folder(false, home(), None),
            Some(PathBuf::from("/home/someone/.config/autostart"))
        );
        assert_eq!(
            system_folder(false, home(), Some("/elsewhere".into())),
            Some(PathBuf::from("/elsewhere/autostart"))
        );
        assert_eq!(
            system_folder(false, home(), Some("relative".into())),
            Some(PathBuf::from("/home/someone/.config/autostart")),
            "a relative one counts as not set"
        );
        assert_eq!(
            system_folder(true, home(), Some("/elsewhere".into())),
            Some(PathBuf::from("/home/someone/Library/LaunchAgents"))
        );
        assert_eq!(system_folder(false, None, None), None);
        assert_eq!(system_folder(true, None, None), None);
    }

    #[test]
    fn a_desktop_entry_quotes_the_path_as_its_specification_says() {
        assert_eq!(exec_word("/opt/my apps/pult"), "\"/opt/my apps/pult\"");
        assert_eq!(exec_word("/a$b"), r#""/a\\$b""#);
        assert_eq!(exec_word("/a\"b"), r#""/a\\"b""#);
        assert_eq!(exec_word("/a`b"), r#""/a\\`b""#);
        assert_eq!(exec_word(r"/a\b"), r#""/a\\\\b""#);
        assert_eq!(exec_word("/100%/pult"), "\"/100%%/pult\"");
        let entry = desktop_entry("/opt/my apps/pult", StartMode::Tray);
        assert!(
            entry.starts_with("[Desktop Entry]\nType=Application\n"),
            "{entry}"
        );
        assert!(
            entry.contains("\nExec=\"/opt/my apps/pult\" --tray\n"),
            "{entry}"
        );
        assert_eq!(mode_of(&entry), StartMode::Tray);
        let entry = desktop_entry("/opt/--tray/pult", StartMode::Window);
        assert!(entry.contains("\nExec=\"/opt/--tray/pult\"\n"), "{entry}");
        assert_eq!(mode_of(&entry), StartMode::Window);
    }

    #[test]
    fn a_launch_agent_keeps_the_path_as_one_escaped_argument() {
        let agent = launch_agent(
            "io.github.wkdsteve.pult",
            "/Applications/R&B <1>/pult-app",
            StartMode::Tray,
        );
        assert!(
            agent.contains("<string>io.github.wkdsteve.pult</string>"),
            "{agent}"
        );
        assert!(
            agent.contains("<string>/Applications/R&amp;B &lt;1&gt;/pult-app</string>"),
            "{agent}"
        );
        assert!(agent.contains("<string>--tray</string>"), "{agent}");
        assert!(agent.contains("<key>RunAtLoad</key>\n\t<true/>"), "{agent}");
        assert_eq!(mode_of(&agent), StartMode::Tray);
        let agent = launch_agent("label", "/Applications/pult-app", StartMode::Window);
        assert!(!agent.contains("--tray"), "{agent}");
        assert!(
            agent.contains("<string>/Applications/pult-app</string>\n\t</array>"),
            "{agent}"
        );
        assert_eq!(mode_of(&agent), StartMode::Window);
    }
}
