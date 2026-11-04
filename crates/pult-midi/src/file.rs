//! The bindings as a file, `midi.toml` in the config folder:
//!
//! ```toml
//! device = "APC MINI"             # a part of the name of the port
//!
//! [[binding]]
//! cc = 48                         # a fader or knob; or note = 0 for a button
//! channel = 0                     # optional, 0 to 15
//! target = "input/1/gain"         # a parameter address, or scene/<name>
//! relative = true                 # optional: an endless knob, which sends steps
//! ```
//!
//! pult writes it when something is learned; comments in it do not survive that. A file that does
//! not read is left alone, and MIDI stays off until it is repaired: what was learned must not go.

use std::fmt::Write as _;
use std::path::Path;

use pult_core::midi::{Bound, Control, ControlKind};
use serde::Deserialize;

/// What the file holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Saved {
    /// The device, once there is one.
    pub device: Option<String>,
    /// The bindings.
    pub bindings: Vec<Bound>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    device: Option<String>,
    #[serde(default)]
    binding: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    cc: Option<u8>,
    note: Option<u8>,
    #[serde(default)]
    channel: u8,
    target: String,
    #[serde(default)]
    relative: bool,
}

/// Reads the text of a file.
///
/// # Errors
///
/// What is wrong, for a syntax error, an unknown key or a number no controller sends.
pub fn parse(text: &str) -> Result<Saved, String> {
    let file: File = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut bindings: Vec<Bound> = Vec::with_capacity(file.binding.len());
    for entry in file.binding {
        let (kind, number) = match (entry.cc, entry.note) {
            (Some(cc), None) => (ControlKind::Cc, cc),
            (None, Some(note)) => (ControlKind::Note, note),
            _ => {
                return Err(format!(
                    "binding of {}: give either cc or note",
                    entry.target
                ));
            }
        };
        if number > 127 {
            return Err(format!(
                "binding of {}: {number} is above 127",
                entry.target
            ));
        }
        if entry.channel > 15 {
            return Err(format!(
                "binding of {}: channel {} is above 15",
                entry.target, entry.channel
            ));
        }
        let control = Control {
            kind,
            channel: entry.channel,
            number,
        };
        if bindings.iter().any(|known| known.control == control) {
            return Err(format!("{control} is bound twice"));
        }
        bindings.push(Bound {
            control,
            target: entry.target,
            relative: entry.relative,
        });
    }
    Ok(Saved {
        device: file.device,
        bindings,
    })
}

/// A TOML string.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The text of `saved`, which [`parse`] reads back.
#[must_use]
pub fn render(saved: &Saved) -> String {
    // Writing into a String cannot fail.
    let mut text = String::from("# The MIDI controller of pult; written by `pult learn`.\n");
    if let Some(device) = &saved.device {
        let _ = writeln!(text, "device = {}", quoted(device));
    }
    for bound in &saved.bindings {
        let key = match bound.control.kind {
            ControlKind::Cc => "cc",
            ControlKind::Note => "note",
        };
        let _ = write!(text, "\n[[binding]]\n{key} = {}\n", bound.control.number);
        if bound.control.channel > 0 {
            let _ = writeln!(text, "channel = {}", bound.control.channel);
        }
        let _ = writeln!(text, "target = {}", quoted(&bound.target));
        if bound.relative {
            text.push_str("relative = true\n");
        }
    }
    text
}

/// Reads the file at `path`; a missing one holds nothing.
///
/// # Errors
///
/// What is wrong with it.
pub fn load(path: &Path) -> Result<Saved, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text).map_err(|why| format!("{}: {why}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Saved::default()),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// Writes `saved` to `path`, through a temporary file so that a failed write leaves the old intact.
///
/// # Errors
///
/// What went wrong.
pub fn save(path: &Path, saved: &Saved) -> Result<(), String> {
    let failed = |e: std::io::Error| format!("cannot write {}: {e}", path.display());
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(failed)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = Path::new(&temporary);
    std::fs::write(temporary, render(saved)).map_err(failed)?;
    std::fs::rename(temporary, path).map_err(|e| {
        let _ = std::fs::remove_file(temporary);
        failed(e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bound(kind: ControlKind, channel: u8, number: u8, target: &str) -> Bound {
        Bound {
            control: Control {
                kind,
                channel,
                number,
            },
            target: target.to_owned(),
            relative: number == 48,
        }
    }

    #[test]
    fn it_reads_back_what_it_wrote() {
        let saved = Saved {
            device: Some("APC \"MINI\"\\".to_owned()),
            bindings: vec![
                bound(ControlKind::Cc, 0, 48, "input/1/gain"),
                bound(ControlKind::Note, 3, 12, "scene/spät am Abend"),
            ],
        };
        assert_eq!(parse(&render(&saved)), Ok(saved));
        assert_eq!(parse(&render(&Saved::default())), Ok(Saved::default()));
    }

    #[test]
    fn it_refuses_what_makes_no_sense() {
        let binding = |rest: &str| format!("[[binding]]\ntarget = \"x\"\n{rest}\n");
        for (text, why) in [
            (binding(""), "either cc or note"),
            (binding("cc = 1\nnote = 2"), "either cc or note"),
            (binding("cc = 128"), "above 127"),
            (binding("cc = 1\nchannel = 16"), "above 15"),
            (binding("cc = 300"), "cc"),
            (binding("cc = 1\ncolor = 2"), "unknown field"),
            ("devices = \"x\"".to_owned(), "unknown field"),
            (binding("cc = 1") + &binding("cc = 1"), "bound twice"),
            ("not toml".to_owned(), ""),
        ] {
            let message = parse(&text).expect_err(&text);
            assert!(message.contains(why), "{text}: {message}");
        }
    }

    #[test]
    fn a_missing_file_holds_nothing_and_saving_makes_its_folder() {
        let dir = std::env::temp_dir().join(format!("pult-midi-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("below").join("midi.toml");
        assert_eq!(load(&path), Ok(Saved::default()));
        let saved = Saved {
            device: Some("APC".to_owned()),
            bindings: vec![bound(ControlKind::Cc, 0, 48, "input/1/gain")],
        };
        save(&path, &saved).expect("saved");
        assert_eq!(load(&path), Ok(saved));
        assert!(!dir.join("below").join("midi.toml.tmp").exists());
        std::fs::write(&path, "broken =").expect("a broken file");
        assert!(load(&path).expect_err("broken").contains("midi.toml"));
        std::fs::remove_dir_all(&dir).expect("clean up");
    }
}
