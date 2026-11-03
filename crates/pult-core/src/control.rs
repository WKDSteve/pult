//! Commands that change a running route: the words typed during `pult run`, and later the
//! vocabulary of the control interface.
//!
//! ```text
//! gain <target> <dB>     gain of an input or bus, for example `gain A1 -6`
//! mute <target>          unmute <target>
//! pan <input> <-1..1>    pan (mono) or balance (stereo) of an input
//! show                   current gains, mutes and pans
//! save <file>            the current state as a profile
//! help                   the commands
//! quit                   stop (an empty line too)
//! ```
//!
//! A target is an input by its number (`1`) or a unique part of its name (`cable`), or a bus
//! by its name (`A1`). Keywords ignore case. Ranges are checked where the command is
//! applied, since they belong to the route.

use std::fmt;

use crate::select::{SelectError, select};

/// An input or a bus of the running route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// An input, by index.
    Input(usize),
    /// A bus, by index.
    Bus(usize),
}

/// One command.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Sets the gain in dB.
    Gain(Target, f32),
    /// Mutes (true) or unmutes (false).
    Mute(Target, bool),
    /// Sets the pan of an input, -1 (left) to 1 (right).
    Pan(usize, f32),
    /// Lists the current state.
    Show,
    /// Writes the current state as a profile to this path.
    Save(String),
    /// Lists the commands.
    Help,
    /// Ends the run.
    Stop,
}

/// The command summary `help` prints.
pub const HELP: &str = "\
gain <target> <dB>   set the gain of an input or bus (gain A1 -6, gain 2 0)
mute <target>        mute an input or bus; unmute <target> undoes it
pan <input> <-1..1>  pan a mono input, balance a stereo one
show                 list gains, mutes and pans
save <file>          write the current state as a profile
quit                 stop (an empty line too)
A target is an input number (1), a unique part of its name (cable) or a bus name (A1).";

/// Why a line is not a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

fn error<T>(message: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError(message.into()))
}

/// Parses one line against the route's input names and bus names.
///
/// # Errors
///
/// [`ParseError`] naming the problem: an unknown keyword, a missing or unknown target, or a
/// value that is not a number.
pub fn parse<S: AsRef<str>>(line: &str, inputs: &[S], buses: &[S]) -> Result<Command, ParseError> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let Some((&keyword, rest)) = words.split_first() else {
        return Ok(Command::Stop);
    };
    match keyword.to_lowercase().as_str() {
        "quit" | "q" | "exit" | "stop" => no_arguments(rest, Command::Stop),
        "show" | "list" | "ls" => no_arguments(rest, Command::Show),
        "help" | "?" | "h" => no_arguments(rest, Command::Help),
        "save" => {
            // The rest of the line as typed, so a path keeps its spaces.
            let path = line.trim_start()[keyword.len()..].trim();
            if path.is_empty() {
                return error("save needs a file name (save abend.toml)");
            }
            Ok(Command::Save(path.to_owned()))
        }
        "mute" | "unmute" => {
            let target = target(&rest.join(" "), inputs, buses)?;
            Ok(Command::Mute(target, keyword.eq_ignore_ascii_case("mute")))
        }
        "gain" => {
            let (name, value) = split_value(rest, "gain <target> <dB>")?;
            Ok(Command::Gain(target(&name, inputs, buses)?, value))
        }
        "pan" => {
            let (name, value) = split_value(rest, "pan <input> <-1..1>")?;
            match target(&name, inputs, buses)? {
                Target::Input(index) => Ok(Command::Pan(index, value)),
                Target::Bus(_) => error("pan works on inputs only"),
            }
        }
        other => error(format!("unknown command \"{other}\"; type help")),
    }
}

fn no_arguments(rest: &[&str], command: Command) -> Result<Command, ParseError> {
    if rest.is_empty() {
        Ok(command)
    } else {
        error(format!("\"{}\" is too much here", rest.join(" ")))
    }
}

/// The target words and the number after them.
fn split_value(rest: &[&str], usage: &str) -> Result<(String, f32), ParseError> {
    let Some((&value, name)) = rest.split_last() else {
        return error(format!("usage: {usage}"));
    };
    if name.is_empty() {
        return error(format!("usage: {usage}"));
    }
    match value.parse::<f32>() {
        Ok(number) if number.is_finite() => Ok((name.join(" "), number)),
        _ => error(format!("\"{value}\" is not a number; usage: {usage}")),
    }
}

/// An input number, a bus name or a unique part of an input name.
fn target<S: AsRef<str>>(name: &str, inputs: &[S], buses: &[S]) -> Result<Target, ParseError> {
    let name = name.trim().trim_start_matches('[').trim_end_matches(']');
    if name.is_empty() {
        return error("which input or bus? (a number, a name part or a bus name)");
    }
    if let Ok(number) = name.parse::<usize>() {
        return match number.checked_sub(1) {
            Some(index) if index < inputs.len() => Ok(Target::Input(index)),
            _ => error(format!(
                "there is no input {number}; inputs are 1 to {}",
                inputs.len()
            )),
        };
    }
    if let Some(index) = buses
        .iter()
        .position(|bus| bus.as_ref().eq_ignore_ascii_case(name))
    {
        return Ok(Target::Bus(index));
    }
    match select(name, inputs) {
        Ok(index) => Ok(Target::Input(index)),
        Err(SelectError::NotFound { .. }) => error(format!("no input or bus matches \"{name}\"")),
        Err(SelectError::Ambiguous { candidates, .. }) => error(format!(
            "\"{name}\" matches several inputs: {}; use the number",
            candidates.join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INPUTS: [&str; 3] = ["CABLE Output", "Analogue 1 + 2", "Spotify.exe"];
    const BUSES: [&str; 2] = ["A1", "B2"];

    fn ok(line: &str) -> Command {
        parse(line, &INPUTS, &BUSES).expect(line)
    }

    fn bad(line: &str) -> String {
        parse(line, &INPUTS, &BUSES).expect_err(line).0
    }

    #[test]
    fn parses_every_command() {
        assert_eq!(ok("gain A1 -6"), Command::Gain(Target::Bus(0), -6.0));
        assert_eq!(ok("GAIN b2 +3.5"), Command::Gain(Target::Bus(1), 3.5));
        assert_eq!(ok("gain 2 0"), Command::Gain(Target::Input(1), 0.0));
        assert_eq!(ok("gain [3] -1"), Command::Gain(Target::Input(2), -1.0));
        assert_eq!(ok("mute cable"), Command::Mute(Target::Input(0), true));
        assert_eq!(
            ok("unmute Analogue 1"),
            Command::Mute(Target::Input(1), false)
        );
        assert_eq!(ok("pan spotify -0.5"), Command::Pan(2, -0.5));
        assert_eq!(ok("show"), Command::Show);
        assert_eq!(
            ok("save  C:\\Profile\\mein  abend.toml "),
            Command::Save("C:\\Profile\\mein  abend.toml".into())
        );
        assert_eq!(ok("?"), Command::Help);
        assert_eq!(ok("q"), Command::Stop);
        assert_eq!(ok("   "), Command::Stop);
    }

    #[test]
    fn names_what_is_wrong() {
        assert!(bad("louder A1").contains("unknown command \"louder\""));
        assert!(bad("gain A1").contains("usage: gain"));
        assert!(bad("gain A1 laut").contains("\"laut\" is not a number"));
        assert!(bad("gain A1 inf").contains("not a number"));
        assert!(bad("gain 4 0").contains("no input 4; inputs are 1 to 3"));
        assert!(bad("mute 0").contains("no input 0"));
        assert!(bad("mute vlc").contains("no input or bus matches \"vlc\""));
        assert!(bad("mute a").contains("several inputs"));
        assert!(bad("mute").contains("which input or bus"));
        assert!(bad("pan A1 0").contains("inputs only"));
        assert!(bad("show me").contains("too much"));
        assert!(bad("save").contains("file name"));
    }
}
