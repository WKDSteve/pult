//! What `ffi` does on Windows, on Linux and macOS. The start on signing in is a file: an autostart
//! entry on Linux (`~/.config/autostart`), a launch agent on macOS (`~/Library/LaunchAgents`),
//! which the system reads when the user signs in next. The one instance per session is still
//! missing, so a second start opens a second window (pult/linux-macos-gleichwertig).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

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

/// The autostart entry that starts `exe` into the tray.
fn desktop_entry(exe: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=pult\nComment=The audio mixer, into the tray\n\
         Exec={} --tray\nTerminal=false\n",
        exec_word(exe)
    )
}

/// `text` as XML character data.
fn xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The launch agent `label` that starts `exe` into the tray.
fn launch_agent(label: &str, exe: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \t<key>Label</key>\n\t<string>{}</string>\n\
         \t<key>ProgramArguments</key>\n\t<array>\n\
         \t\t<string>{}</string>\n\t\t<string>--tray</string>\n\t</array>\n\
         \t<key>RunAtLoad</key>\n\t<true/>\n\
         \t<key>ProcessType</key>\n\t<string>Interactive</string>\n\
         </dict>\n</plist>\n",
        xml_text(label),
        xml_text(exe)
    )
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

/// Lets the system start `command`, the path of this program, into the tray when the user signs
/// in (a file of `name` in the folder `key`), or with `None` no longer; removing what is not
/// there is no error.
///
/// # Errors
///
/// What the system said when the file could not be written or removed, or that a line break in
/// the path cannot be written into it.
pub fn set_run(key: &str, name: &str, command: Option<&str>) -> Result<(), String> {
    let file = run_file(key, name)?;
    let Some(exe) = command else {
        return match fs::remove_file(&file) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    };
    if exe.contains(['\n', '\r']) {
        return Err("the path of pult-app has a line break".to_owned());
    }
    let text = if MACOS {
        launch_agent(name, exe)
    } else {
        desktop_entry(exe)
    };
    write_whole(&file, &text).map_err(|e| e.to_string())
}

/// Whether the system has something to start under that name.
#[must_use]
pub fn run_exists(key: &str, name: &str) -> bool {
    run_file(key, name).is_ok_and(|file| file.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_missing_says_so_instead_of_pretending() {
        assert!(claim(INSTANCE).is_some());
        assert!(!ask_to_show(SHOW, Duration::ZERO));
        assert!(!listen_for_show(SHOW, || {}));
    }

    /// A folder of this test run, never the one the system reads.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            Self(std::env::temp_dir().join(format!("pult-app-run-{name}-{}", std::process::id())))
        }

        fn key(&self) -> &str {
            self.0.to_str().expect("a temp folder in Unicode")
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_start_is_written_and_removed() {
        let folder = Folder::new("start");
        assert!(!run_exists(folder.key(), "pult"), "no folder yet");
        set_run(folder.key(), "pult", None).expect("nothing to remove is no error");
        set_run(folder.key(), "pult", Some("/opt/pult/pult-app")).expect("written");
        assert!(run_exists(folder.key(), "pult"));
        assert!(!run_exists(folder.key(), "other"));
        let file = run_file(folder.key(), "pult").expect("a file");
        let text = fs::read_to_string(&file).expect("readable");
        assert!(text.contains("/opt/pult/pult-app"), "{text}");
        assert!(text.contains("--tray"), "{text}");
        let files = fs::read_dir(&folder.0).expect("the folder").count();
        assert_eq!(files, 1, "nothing is left beside it");
        set_run(folder.key(), "pult", Some("/usr/bin/pult-app")).expect("written again");
        let text = fs::read_to_string(&file).expect("readable");
        assert!(
            text.contains("/usr/bin/pult-app") && !text.contains("/opt"),
            "{text}"
        );
        set_run(folder.key(), "pult", None).expect("removed");
        assert!(!run_exists(folder.key(), "pult"));
    }

    #[test]
    fn a_line_break_in_the_path_cannot_add_lines_to_the_file() {
        let folder = Folder::new("break");
        let refused = set_run(folder.key(), "pult", Some("/a\nExec=/bin/other"));
        assert!(refused.is_err());
        assert!(!run_exists(folder.key(), "pult"));
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
        let entry = desktop_entry("/opt/my apps/pult");
        assert!(
            entry.starts_with("[Desktop Entry]\nType=Application\n"),
            "{entry}"
        );
        assert!(
            entry.contains("\nExec=\"/opt/my apps/pult\" --tray\n"),
            "{entry}"
        );
    }

    #[test]
    fn a_launch_agent_keeps_the_path_as_one_escaped_argument() {
        let agent = launch_agent("io.github.wkdsteve.pult", "/Applications/R&B <1>/pult-app");
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
    }
}
