//! Changing a running route: the commands of `pult_core::control` applied to its controls,
//! and its current state written back as a profile.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::atomic::Ordering;

use pult_core::control::{Command, HELP, Target};
use pult_core::db_to_linear;

use crate::profile::{self, GAIN_DB};
use crate::{RouteSpec, Running};

impl Running {
    /// The input names a command may name, in route order.
    #[must_use]
    pub fn input_names(&self) -> Vec<String> {
        self.inputs.iter().map(|input| input.name.clone()).collect()
    }

    /// The bus names, in route order.
    #[must_use]
    pub fn bus_names(&self) -> Vec<String> {
        self.buses.iter().map(|bus| bus.name.clone()).collect()
    }

    /// The route as it is now: the spec it started with, with the gains, mutes and pans
    /// changed since.
    #[must_use]
    pub fn current(&self) -> RouteSpec {
        self.spec.clone()
    }

    /// Applies `command` and returns what to show for it, one line each. `Stop` changes
    /// nothing; ending the run is the caller's business.
    ///
    /// # Errors
    ///
    /// A message when a value is out of range or the profile cannot be written.
    pub fn apply(&mut self, command: &Command) -> Result<Vec<String>, String> {
        match command {
            Command::Gain(target, gain_db) => {
                if !GAIN_DB.contains(gain_db) {
                    return Err(format!(
                        "gain must be {} to {} dB, not {gain_db}",
                        GAIN_DB.start(),
                        GAIN_DB.end()
                    ));
                }
                let linear = db_to_linear(*gain_db);
                match *target {
                    Target::Input(index) => {
                        self.inputs[index].controls.gain.store(linear);
                        self.spec.inputs[index].gain_db = *gain_db;
                    }
                    Target::Bus(index) => {
                        self.buses[index].handle.controls.gain.store(linear);
                        self.spec.buses[index].gain_db = *gain_db;
                    }
                }
                Ok(vec![format!(
                    "{}: gain {gain_db:.1} dB",
                    self.target_label(*target)
                )])
            }
            Command::Mute(target, muted) => {
                match *target {
                    Target::Input(index) => {
                        self.inputs[index]
                            .controls
                            .muted
                            .store(*muted, Ordering::Relaxed);
                        self.spec.inputs[index].muted = *muted;
                    }
                    Target::Bus(index) => {
                        self.buses[index]
                            .handle
                            .controls
                            .muted
                            .store(*muted, Ordering::Relaxed);
                        self.spec.buses[index].muted = *muted;
                    }
                }
                let state = if *muted { "muted" } else { "unmuted" };
                Ok(vec![format!("{}: {state}", self.target_label(*target))])
            }
            Command::Pan(index, pan) => {
                if !(-1.0..=1.0).contains(pan) {
                    return Err(format!("pan must be -1 to 1, not {pan}"));
                }
                self.inputs[*index].controls.pan.store(*pan);
                self.spec.inputs[*index].pan = *pan;
                Ok(vec![format!(
                    "{}: pan {pan:.2}",
                    self.target_label(Target::Input(*index))
                )])
            }
            Command::Show => Ok(self.show()),
            Command::Save(path) => self.save_profile(Path::new(path)),
            Command::Help => Ok(HELP.lines().map(str::to_owned).collect()),
            Command::Stop => Ok(Vec::new()),
        }
    }

    /// Routes input `input` to bus `bus` or takes it off (a matrix button); the mixer of the
    /// bus follows in its next callback, a newly routed ring after priming.
    ///
    /// # Errors
    ///
    /// A message when an index is out of range or the input would feed no bus.
    pub fn route(&mut self, input: usize, bus: usize, on: bool) -> Result<Vec<String>, String> {
        let Some(running) = self.inputs.get(input) else {
            return Err(format!("there is no input {}", input + 1));
        };
        let Some(route) = running.routes.get(bus) else {
            return Err(format!("there is no bus {}", bus + 1));
        };
        let label = self.target_label(Target::Input(input));
        let bus_name = self.buses[bus].name.clone();
        if !on && route.is_routed() && running.routes.iter().filter(|r| r.is_routed()).count() == 1
        {
            return Err(format!("{label} would feed no bus; mute it instead"));
        }
        route.routed.store(on, Ordering::Release);
        let buses = &mut self.spec.inputs[input].buses;
        buses.retain(|&b| b != bus);
        if on {
            buses.push(bus);
            buses.sort_unstable();
        }
        let state = if on { "->" } else { "off" };
        Ok(vec![format!("{label} {state} {bus_name}")])
    }

    /// Writes the current state as a profile to `path`.
    ///
    /// # Errors
    ///
    /// A message when the file cannot be written.
    pub fn save_profile(&self, path: &Path) -> Result<Vec<String>, String> {
        save(path, &self.spec)
            .map(|()| vec![format!("saved the current state to {}", path.display())])
    }

    /// `[1] CABLE Output` or `A1`.
    fn target_label(&self, target: Target) -> String {
        match target {
            Target::Input(index) => format!("[{}] {}", index + 1, self.inputs[index].name),
            Target::Bus(index) => self.buses[index].name.clone(),
        }
    }

    /// One line per input and bus with its settings.
    fn show(&self) -> Vec<String> {
        let mut lines = Vec::with_capacity(self.inputs.len() + self.buses.len());
        for (index, (input, spec)) in self.inputs.iter().zip(&self.spec.inputs).enumerate() {
            let buses: Vec<&str> = input
                .routes
                .iter()
                .filter(|route| route.is_routed())
                .map(|route| self.buses[route.bus].name.as_str())
                .collect();
            let mut line = format!(
                "[{}] {}  gain {:.1} dB  pan {:.2}  -> {}",
                index + 1,
                input.name,
                spec.gain_db,
                spec.pan,
                buses.join(", ")
            );
            if spec.muted {
                line.push_str("  muted");
            }
            lines.push(line);
        }
        for (bus, spec) in self.buses.iter().zip(&self.spec.buses) {
            let mut line = format!("{} {}  gain {:.1} dB", bus.name, bus.device, spec.gain_db);
            if spec.muted {
                line.push_str("  muted");
            }
            lines.push(line);
        }
        lines
    }
}

/// Writes `spec` as a profile to `path`, through a temporary file so that a failed write
/// leaves an existing profile intact.
fn save(path: &Path, spec: &RouteSpec) -> Result<(), String> {
    let text = profile::render(spec);
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = Path::new(&temporary);
    let failed = |e: std::io::Error| format!("cannot write {}: {e}", path.display());
    std::fs::write(temporary, text).map_err(failed)?;
    std::fs::rename(temporary, path).map_err(|e| {
        let _ = std::fs::remove_file(temporary);
        failed(e)
    })
}

/// A TOML basic string.
pub(crate) fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_escapes_what_toml_needs() {
        assert_eq!(quoted("CABLE Output"), "\"CABLE Output\"");
        assert_eq!(quoted("a\"b\\c\td\u{1}"), "\"a\\\"b\\\\c\\td\\u0001\"");
        assert_eq!(quoted("Kopfhörer"), "\"Kopfhörer\"");
    }

    #[cfg(windows)]
    #[test]
    fn save_writes_to_a_path_that_is_not_unicode() {
        use std::os::windows::ffi::OsStringExt;
        let dir = std::env::temp_dir().join(format!("pult-save-wide-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        // A lone surrogate: a valid Windows file name, but no valid Unicode.
        let name = std::ffi::OsString::from_wide(&[0xD800, u16::from(b'a'), u16::from(b'.')]);
        let path = dir.join(name);
        assert!(path.to_str().is_none());
        let spec = profile::parse(include_str!("../../../examples/pult.toml")).expect("example");
        save(&path, &spec).expect("save");
        assert!(path.exists());
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn save_replaces_a_file_and_leaves_no_temporary() {
        let dir = std::env::temp_dir().join(format!("pult-save-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("profil.toml");
        std::fs::write(&path, "old").expect("old file");
        let spec = profile::parse(include_str!("../../../examples/pult.toml")).expect("example");
        save(&path, &spec).expect("save");
        let text = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(profile::parse(&text).expect("valid"), spec);
        assert!(!dir.join("profil.toml.tmp").exists());
        std::fs::remove_dir_all(&dir).expect("clean up");
    }
}
