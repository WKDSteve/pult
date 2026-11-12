//! Commands that change a running route: the words typed during `pult run`, and later the
//! vocabulary of the control interface.
//!
//! ```text
//! gain <target> <dB>     gain of an input or bus, for example `gain A1 -6`
//! mute <target>          unmute <target>
//! solo <input>           unsolo <input>
//! pan <input> <-1..1>    pan (mono) or balance (stereo) of an input
//! gate <input> <0..10>   keep an input quiet below a level; 0 or off switches it off
//! comp <input> <0..10>   even out an input's level and make it louder; 0 or off switches it off
//! eq <bus> on|off        switch the equalizer of a bus
//! eq <bus> <band> <setting> <value>
//!                        one setting of one of its bands: gain <dB>, freq <Hz>, q <width>,
//!                        kind low|bell|high (eq A1 2 gain 6)
//! limit <bus> on|off     switch the limiter of a bus
//! limit <bus> <dB>       its ceiling, -20 to 0 (limit A1 -3)
//! limit <bus> look <ms>  its lookahead, 0 to 5: how gently it turns down, and how much later the
//!                        sound leaves the bus
//! route <input> <bus>    send an input to a bus too; unroute <input> <bus> takes it off
//! scene <name>           change to a scene of the profile; scenes lists them
//! learn <target>         the next movement on the MIDI controller binds it; unlearn takes it off
//! show                   current gains, mutes and pans
//! save <file>            the current state as a profile
//! help                   the commands
//! quit                   stop (an empty line too)
//! ```
//!
//! A target is an input by its number (`1`) or a unique part of its name or label (`cable`), or
//! a bus by its name (`A1`) or a unique part of its label (`boxen`). What fits more than one is
//! refused. Keywords ignore case. Ranges are checked where the command is
//! applied, since they belong to the route. A gain or pan can take its time:
//! `gain A1 -6 over 500` gets there in 500 ms.

use std::fmt;

use crate::eq::{BandChange, BandKind, EQ_BANDS};
use crate::limiter::LimitChange;
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
    /// Sets the gain in dB, reached in so many milliseconds (0: at once, free of clicks).
    Gain(Target, f32, u32),
    /// Mutes (true) or unmutes (false).
    Mute(Target, bool),
    /// Solos (true) or unsolos (false) an input.
    Solo(usize, bool),
    /// Sets the pan of an input, -1 (left) to 1 (right), reached in so many milliseconds.
    Pan(usize, f32, u32),
    /// Sets the gate of an input by its one knob, 0 (off) to 10.
    Gate(usize, f32),
    /// Sets the compressor of an input by its one knob, 0 (off) to 10.
    Comp(usize, f32),
    /// Switches the equalizer of a bus on or off.
    Eq(usize, bool),
    /// Changes one setting of one band (second index, from 0) of the equalizer of a bus.
    EqBand(usize, usize, BandChange),
    /// Changes one setting of the limiter of a bus.
    Limit(usize, LimitChange),
    /// Makes a bus a monitor bus (the one solo acts on) or takes that away.
    Monitor(usize, bool),
    /// Routes an input (first index) to a bus (second index), or takes it off.
    Route(usize, usize, bool),
    /// Sets the level of a crossing (input, bus) in dB, reached in so many milliseconds: what of
    /// the input this bus gets.
    Send(usize, usize, f32, u32),
    /// Takes the send of a crossing (input, bus) before the input's fader (true) or after it.
    SendPre(usize, usize, bool),
    /// Changes to the scene of this name.
    Scene(String),
    /// Lists the scenes.
    Scenes,
    /// Lets the next movement on the MIDI controller bind this address or `scene/<name>`.
    Learn(String),
    /// Takes the MIDI binding of this address or `scene/<name>` away.
    Unlearn(String),
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
solo <input>         hear only the soloed inputs; unsolo <input> undoes it
pan <input> <-1..1>  pan a mono input, balance a stereo one
gate <input> <0..10> keep an input quiet below a level (gate 2 3); gate 2 off undoes it
comp <input> <0..10> even out an input's level and make it louder; comp 2 off undoes it
eq <bus> on|off      switch the equalizer of a bus
eq <bus> <band> <setting> <value>
                     one setting of one of its 4 bands: gain <dB>, freq <Hz>, q <width>,
                     kind low|bell|high (eq A1 2 gain 6, eq A1 1 kind bell)
limit <bus> on|off   switch the limiter of a bus
limit <bus> <dB>     its ceiling, -20 to 0 (limit A1 -3)
limit <bus> look <ms>
                     its lookahead, 0 to 5: gentler, and the sound leaves that much later
route <input> <bus>  send an input to a bus too; unroute <input> <bus> takes it off
monitor <bus> on|off make a bus the monitor: solo acts only there, a talk bus keeps hearing all
send <input> <bus> <dB>
                     the level of that crossing, -60 to 12 (send 2 B1 -6): a mix of its own
                     per bus; send <input> <bus> pre|post takes it before or after the fader
scene <name>         change to a scene of the profile; scenes lists them
learn <address>      the next movement on the MIDI controller binds it (learn input/1/gain,
                     learn scene/abend); unlearn <address> takes it off
show                 list gains, mutes and pans
save <file>          write the current state as a profile
quit                 stop (an empty line too)
A target is an input number (1), a unique part of its name or label (cable), a bus name (A1)
or a unique part of a bus label (boxen).
A gain or pan can take its time: gain A1 -6 over 500 gets there in 500 ms.";

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

/// What a command may call an input: its full name, and the label before it, so that a part of
/// either finds it.
#[must_use]
pub fn search_name(name: &str, label: Option<&str>) -> String {
    match label {
        Some(label) => format!("{label} {name}"),
        None => name.to_owned(),
    }
}

/// Parses one line against the route's input names, bus names and bus labels. `bus_labels` goes
/// with `buses`, one entry each, empty for a bus without a label.
///
/// # Errors
///
/// [`ParseError`] naming the problem: an unknown keyword, a missing, unknown or ambiguous target,
/// or a value that is not a number.
pub fn parse<S: AsRef<str>>(
    line: &str,
    inputs: &[S],
    buses: &[S],
    bus_labels: &[S],
) -> Result<Command, ParseError> {
    let names = Names {
        inputs,
        buses,
        bus_labels,
    };
    let words: Vec<&str> = line.split_whitespace().collect();
    let Some((&keyword, rest)) = words.split_first() else {
        return Ok(Command::Stop);
    };
    match keyword.to_lowercase().as_str() {
        "quit" | "q" | "exit" | "stop" => no_arguments(rest, Command::Stop),
        "show" | "list" | "ls" => no_arguments(rest, Command::Show),
        "help" | "?" | "h" => no_arguments(rest, Command::Help),
        "scenes" => no_arguments(rest, Command::Scenes),
        "learn" | "unlearn" => {
            // The rest of the line, so the name of a scene keeps its spaces.
            let target = line.trim_start()[keyword.len()..].trim();
            if target.is_empty() {
                return error("which address? (learn input/1/gain, learn scene/abend)");
            }
            Ok(if keyword.eq_ignore_ascii_case("learn") {
                Command::Learn(target.to_owned())
            } else {
                Command::Unlearn(target.to_owned())
            })
        }
        "scene" => {
            // The rest of the line, so a name keeps its spaces.
            let name = line.trim_start()[keyword.len()..].trim();
            if name.is_empty() {
                return error("scene needs a name (scene abend); scenes lists them");
            }
            Ok(Command::Scene(name.to_owned()))
        }
        "save" => {
            // The rest of the line as typed, so a path keeps its spaces.
            let path = line.trim_start()[keyword.len()..].trim();
            if path.is_empty() {
                return error("save needs a file name (save abend.toml)");
            }
            Ok(Command::Save(path.to_owned()))
        }
        "mute" | "unmute" => {
            let target = target(&rest.join(" "), names)?;
            Ok(Command::Mute(target, keyword.eq_ignore_ascii_case("mute")))
        }
        "solo" | "unsolo" => match target(&rest.join(" "), names)? {
            Target::Input(index) => Ok(Command::Solo(index, keyword.eq_ignore_ascii_case("solo"))),
            Target::Bus(_) => error("solo works on inputs only"),
        },
        "gain" => {
            let (rest, ramp_ms) = split_ramp(rest)?;
            let (name, value) = split_value(rest, "gain <target> <dB> [over <ms>]")?;
            Ok(Command::Gain(target(&name, names)?, value, ramp_ms))
        }
        "pan" => {
            let (rest, ramp_ms) = split_ramp(rest)?;
            let (name, value) = split_value(rest, "pan <input> <-1..1> [over <ms>]")?;
            match target(&name, names)? {
                Target::Input(index) => Ok(Command::Pan(index, value, ramp_ms)),
                Target::Bus(_) => error("pan works on inputs only"),
            }
        }
        "gate" | "comp" | "compressor" => {
            let gate = keyword.eq_ignore_ascii_case("gate");
            let usage = if gate {
                "gate <input> <0..10|off>"
            } else {
                "comp <input> <0..10|off>"
            };
            // `off` is the knob at 0.
            let off = rest
                .last()
                .is_some_and(|word| word.eq_ignore_ascii_case("off"));
            let (name, amount) = if off && rest.len() > 1 {
                (rest[..rest.len() - 1].join(" "), 0.0)
            } else {
                split_value(rest, usage)?
            };
            match target(&name, names)? {
                Target::Input(index) if gate => Ok(Command::Gate(index, amount)),
                Target::Input(index) => Ok(Command::Comp(index, amount)),
                Target::Bus(_) => error(format!("{keyword} works on inputs only")),
            }
        }
        "eq" => equalizer(rest, names),
        "limit" => limiter(rest, names),
        "route" | "unroute" => {
            let on = keyword.eq_ignore_ascii_case("route");
            let (input, bus) = route(rest, names)?;
            Ok(Command::Route(input, bus, on))
        }
        "send" => send(rest, names),
        "monitor" => monitor(rest, names),
        other => error(format!("unknown command \"{other}\"; type help")),
    }
}

/// `monitor <bus> on|off`; the bus may have several words.
fn monitor<S: AsRef<str>>(rest: &[&str], names: Names<'_, S>) -> Result<Command, ParseError> {
    let usage = "usage: monitor <bus> on|off";
    let Some((switch, bus)) = rest.split_last() else {
        return error(usage);
    };
    let on = match switch.to_ascii_lowercase().as_str() {
        "on" => true,
        "off" => false,
        _ => return error(usage),
    };
    match target(&bus.join(" "), names)? {
        Target::Bus(index) => Ok(Command::Monitor(index, on)),
        Target::Input(_) => error("monitor works on buses only"),
    }
}

/// `send <input> <bus> <dB> [over <ms>]` or `send <input> <bus> pre|post`.
fn send<S: AsRef<str>>(rest: &[&str], names: Names<'_, S>) -> Result<Command, ParseError> {
    let usage = "usage: send <input> <bus> <dB> [over <ms>], or send <input> <bus> pre|post";
    if let [words @ .., last] = rest
        && (last.eq_ignore_ascii_case("pre") || last.eq_ignore_ascii_case("post"))
    {
        if words.len() < 2 {
            return error(usage);
        }
        let (input, bus) = route(words, names)?;
        return Ok(Command::SendPre(
            input,
            bus,
            last.eq_ignore_ascii_case("pre"),
        ));
    }
    let (rest, ramp_ms) = split_ramp(rest)?;
    let (name, db) = split_value(rest, usage.trim_start_matches("usage: "))?;
    let words: Vec<&str> = name.split_whitespace().collect();
    if words.len() < 2 {
        return error(usage);
    }
    let (input, bus) = route(&words, names)?;
    Ok(Command::Send(input, bus, db, ramp_ms))
}

/// `eq <bus> on|off` or `eq <bus> <band> <setting> <value>`; the bus may have several words.
fn equalizer<S: AsRef<str>>(rest: &[&str], names: Names<'_, S>) -> Result<Command, ParseError> {
    let usage = "usage: eq <bus> on|off, or eq <bus> <band> gain|freq|q|kind <value>";
    let only_bus = |words: &[&str]| match target(&words.join(" "), names)? {
        Target::Bus(index) => Ok(index),
        Target::Input(_) => error("eq works on buses only"),
    };
    if let [bus @ .., switch] = rest
        && !bus.is_empty()
        && (switch.eq_ignore_ascii_case("on") || switch.eq_ignore_ascii_case("off"))
    {
        return Ok(Command::Eq(
            only_bus(bus)?,
            switch.eq_ignore_ascii_case("on"),
        ));
    }
    let [bus @ .., band, setting, value] = rest else {
        return error(usage);
    };
    if bus.is_empty() {
        return error(usage);
    }
    let number = |what: &str| match value.parse::<f32>() {
        Ok(number) if number.is_finite() => Ok(number),
        _ => error(format!("\"{value}\" is no {what}; {usage}")),
    };
    let change = match setting.to_lowercase().as_str() {
        "gain" => BandChange::Gain(number("gain in dB")?),
        "freq" => BandChange::Freq(number("frequency in Hz")?),
        "q" => BandChange::Q(number("width")?),
        "kind" => match BandKind::from_word(value) {
            Some(kind) => BandChange::Kind(kind),
            None => {
                return error(format!(
                    "\"{value}\" is no kind; kinds are low, bell and high"
                ));
            }
        },
        other => return error(format!("\"{other}\" is no setting of a band; {usage}")),
    };
    let band = match band.parse::<usize>().ok().and_then(|n| n.checked_sub(1)) {
        Some(index) if index < EQ_BANDS => index,
        _ => {
            return error(format!(
                "there is no band {band}; bands are 1 to {EQ_BANDS}"
            ));
        }
    };
    Ok(Command::EqBand(only_bus(bus)?, band, change))
}

/// `limit <bus> on|off`, `limit <bus> <dB>` or `limit <bus> look <ms>`; the bus may have several
/// words.
fn limiter<S: AsRef<str>>(rest: &[&str], names: Names<'_, S>) -> Result<Command, ParseError> {
    let usage = "usage: limit <bus> on|off, limit <bus> <dB>, or limit <bus> look <ms>";
    let only_bus = |words: &[&str]| match target(&words.join(" "), names)? {
        Target::Bus(index) => Ok(index),
        Target::Input(_) => error("limit works on buses only"),
    };
    let number = |value: &str, what: &str| match value.parse::<f32>() {
        Ok(number) if number.is_finite() => Ok(number),
        _ => error(format!("\"{value}\" is no {what}; {usage}")),
    };
    if let [bus @ .., look, value] = rest
        && !bus.is_empty()
        && (look.eq_ignore_ascii_case("look") || look.eq_ignore_ascii_case("lookahead"))
    {
        let ms = number(value, "lookahead in ms")?;
        return Ok(Command::Limit(only_bus(bus)?, LimitChange::Lookahead(ms)));
    }
    let [bus @ .., last] = rest else {
        return error(usage);
    };
    if bus.is_empty() {
        return error(usage);
    }
    let change = if last.eq_ignore_ascii_case("on") || last.eq_ignore_ascii_case("off") {
        LimitChange::Switch(last.eq_ignore_ascii_case("on"))
    } else {
        LimitChange::Ceiling(number(last, "ceiling in dB")?)
    };
    Ok(Command::Limit(only_bus(bus)?, change))
}

fn no_arguments(rest: &[&str], command: Command) -> Result<Command, ParseError> {
    if rest.is_empty() {
        Ok(command)
    } else {
        error(format!("\"{}\" is too much here", rest.join(" ")))
    }
}

/// Takes `over <ms>` off the end of a gain or pan. Only with a target and a value before it, so
/// an input whose name holds the word stays one: `gain over 3` sets the input called over.
fn split_ramp<'a, 'b>(rest: &'a [&'b str]) -> Result<(&'a [&'b str], u32), ParseError> {
    if let [before @ .., value, keyword, ms] = rest
        && keyword.eq_ignore_ascii_case("over")
        && !before.is_empty()
        && value.parse::<f32>().is_ok()
    {
        return match ms.parse::<u32>() {
            Ok(ms) => Ok((&rest[..rest.len() - 2], ms)),
            Err(_) => error(format!("\"{ms}\" is no time in milliseconds")),
        };
    }
    Ok((rest, 0))
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

/// What a line may call the parts of the route.
struct Names<'a, S> {
    inputs: &'a [S],
    buses: &'a [S],
    bus_labels: &'a [S],
}

// By hand: a derive would ask for `S: Copy`, and only references are copied.
impl<S> Clone for Names<'_, S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for Names<'_, S> {}

/// The bus called `name`, or the only one whose label holds it; `Ok(None)` for none.
fn bus<S: AsRef<str>>(name: &str, names: Names<'_, S>) -> Result<Option<usize>, ParseError> {
    if let Some(index) = names
        .buses
        .iter()
        .position(|bus| bus.as_ref().eq_ignore_ascii_case(name))
    {
        return Ok(Some(index));
    }
    match select(name, names.bus_labels) {
        Ok(index) => Ok(Some(index)),
        Err(SelectError::NotFound { .. }) => Ok(None),
        Err(SelectError::Ambiguous { candidates, .. }) => error(format!(
            "\"{name}\" matches several buses: {}; use the bus name",
            candidates.join(", ")
        )),
    }
}

/// The input and the bus of `route`. Both can have several words, so every place to divide them is
/// tried: exactly one reading must make sense. Without one, the last word is taken for the bus, to
/// say what is wrong.
fn route<S: AsRef<str>>(rest: &[&str], names: Names<'_, S>) -> Result<(usize, usize), ParseError> {
    let usage = "usage: route <input> <bus>";
    if rest.len() < 2 {
        return error(usage);
    }
    let mut readings = Vec::new();
    for place in 1..rest.len() {
        let (input_words, bus_words) = rest.split_at(place);
        if let (Ok(Target::Input(input)), Ok(Some(bus))) = (
            target(&input_words.join(" "), names),
            bus(&bus_words.join(" "), names),
        ) && !readings.contains(&(input, bus))
        {
            readings.push((input, bus));
        }
    }
    match readings.as_slice() {
        [reading] => return Ok(*reading),
        [] => {}
        _ => {
            return error(format!(
                "\"{}\" can be read several ways; use the input number and the bus name",
                rest.join(" ")
            ));
        }
    }
    let (bus_name, input_words) = (rest[rest.len() - 1], &rest[..rest.len() - 1]);
    let Some(bus) = bus(bus_name, names)? else {
        return error(format!("there is no bus \"{bus_name}\""));
    };
    match target(&input_words.join(" "), names)? {
        Target::Input(input) => Ok((input, bus)),
        Target::Bus(_) => error("only an input can be routed to a bus"),
    }
}

/// An input number, a bus name, or a unique part of an input name or a bus label. A part that fits
/// an input and a bus label too is refused, whichever of them it would have been.
fn target<S: AsRef<str>>(name: &str, names: Names<'_, S>) -> Result<Target, ParseError> {
    let name = name.trim().trim_start_matches('[').trim_end_matches(']');
    if name.is_empty() {
        return error("which input or bus? (a number, a name part or a bus name)");
    }
    if let Ok(number) = name.parse::<usize>() {
        return match number.checked_sub(1) {
            Some(index) if index < names.inputs.len() => Ok(Target::Input(index)),
            _ => error(format!(
                "there is no input {number}; inputs are 1 to {}",
                names.inputs.len()
            )),
        };
    }
    let bus = bus(name, names)?;
    // The name of a bus is exact, nothing else can mean it.
    if let Some(index) = bus
        && names.buses[index].as_ref().eq_ignore_ascii_case(name)
    {
        return Ok(Target::Bus(index));
    }
    match (select(name, names.inputs), bus) {
        (Ok(index), None) => Ok(Target::Input(index)),
        (Err(SelectError::NotFound { .. }), Some(index)) => Ok(Target::Bus(index)),
        (Err(SelectError::NotFound { .. }), None) => {
            error(format!("no input or bus matches \"{name}\""))
        }
        (Ok(index), Some(bus)) => error(format!(
            "\"{name}\" matches input {} and bus {}; use the number or the bus name",
            index + 1,
            names.buses[bus].as_ref()
        )),
        (Err(SelectError::Ambiguous { candidates, .. }), None) => error(format!(
            "\"{name}\" matches several inputs: {}; use the number",
            candidates.join(", ")
        )),
        (Err(SelectError::Ambiguous { candidates, .. }), Some(bus)) => error(format!(
            "\"{name}\" matches several inputs ({}) and bus {}; use the number or the bus name",
            candidates.join(", "),
            names.buses[bus].as_ref()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INPUTS: [&str; 3] = ["CABLE Output", "Analogue 1 + 2", "Spotify.exe"];
    const BUSES: [&str; 2] = ["A1", "B2"];
    const NO_LABELS: [&str; 2] = ["", ""];

    fn ok(line: &str) -> Command {
        parse(line, &INPUTS, &BUSES, &NO_LABELS).expect(line)
    }

    fn bad(line: &str) -> String {
        parse(line, &INPUTS, &BUSES, &NO_LABELS).expect_err(line).0
    }

    #[test]
    fn monitor_switches_a_bus() {
        assert_eq!(ok("monitor B2 on"), Command::Monitor(1, true));
        assert_eq!(ok("MONITOR a1 OFF"), Command::Monitor(0, false));
        assert!(bad("monitor 2 on").contains("buses only"));
        assert!(bad("monitor A1").contains("usage: monitor"));
    }

    #[test]
    fn a_send_has_a_level_or_a_point() {
        assert_eq!(ok("send 2 B2 -6"), Command::Send(1, 1, -6.0, 0));
        assert_eq!(
            ok("send cable a1 -3 over 200"),
            Command::Send(0, 0, -3.0, 200)
        );
        assert_eq!(ok("send Analogue 1 b2 pre"), Command::SendPre(1, 1, true));
        assert_eq!(ok("SEND 3 A1 POST"), Command::SendPre(2, 0, false));
        assert!(bad("send 2").contains("usage: send"));
        assert!(bad("send 2 pre").contains("usage: send"));
    }

    #[test]
    fn parses_every_command() {
        assert_eq!(ok("gain A1 -6"), Command::Gain(Target::Bus(0), -6.0, 0));
        assert_eq!(ok("GAIN b2 +3.5"), Command::Gain(Target::Bus(1), 3.5, 0));
        assert_eq!(ok("gain 2 0"), Command::Gain(Target::Input(1), 0.0, 0));
        assert_eq!(ok("gain [3] -1"), Command::Gain(Target::Input(2), -1.0, 0));
        assert_eq!(
            ok("gain A1 -6 over 500"),
            Command::Gain(Target::Bus(0), -6.0, 500)
        );
        assert_eq!(
            ok("gain Analogue 1 -6 OVER 20"),
            Command::Gain(Target::Input(1), -6.0, 20)
        );
        assert_eq!(ok("pan 2 0.5 over 100"), Command::Pan(1, 0.5, 100));
        assert_eq!(ok("scene abend"), Command::Scene("abend".into()));
        assert_eq!(
            ok("SCENE  spät am  Abend "),
            Command::Scene("spät am  Abend".into())
        );
        assert_eq!(ok("scenes"), Command::Scenes);
        assert_eq!(
            ok("learn input/1/gain"),
            Command::Learn("input/1/gain".into())
        );
        assert_eq!(
            ok("LEARN scene/spät am Abend"),
            Command::Learn("scene/spät am Abend".into())
        );
        assert_eq!(
            ok("unlearn bus/A1/mute"),
            Command::Unlearn("bus/A1/mute".into())
        );
        assert_eq!(ok("mute cable"), Command::Mute(Target::Input(0), true));
        assert_eq!(
            ok("unmute Analogue 1"),
            Command::Mute(Target::Input(1), false)
        );
        assert_eq!(ok("solo cable"), Command::Solo(0, true));
        assert_eq!(ok("UNSOLO Analogue 1"), Command::Solo(1, false));
        assert_eq!(ok("pan spotify -0.5"), Command::Pan(2, -0.5, 0));
        assert_eq!(ok("route 2 B2"), Command::Route(1, 1, true));
        assert_eq!(ok("gate 2 3"), Command::Gate(1, 3.0));
        assert_eq!(ok("GATE cable 7.5"), Command::Gate(0, 7.5));
        assert_eq!(ok("gate Analogue 1 off"), Command::Gate(1, 0.0));
        assert_eq!(ok("comp 3 4"), Command::Comp(2, 4.0));
        assert_eq!(ok("compressor spotify OFF"), Command::Comp(2, 0.0));
        assert_eq!(
            ok("limit A1 off"),
            Command::Limit(0, LimitChange::Switch(false))
        );
        assert_eq!(
            ok("LIMIT b2 ON"),
            Command::Limit(1, LimitChange::Switch(true))
        );
        assert_eq!(
            ok("limit A1 -3.5"),
            Command::Limit(0, LimitChange::Ceiling(-3.5))
        );
        assert_eq!(
            ok("limit A1 look 2"),
            Command::Limit(0, LimitChange::Lookahead(2.0))
        );
        assert_eq!(
            ok("limit B2 lookahead 0"),
            Command::Limit(1, LimitChange::Lookahead(0.0))
        );
        assert_eq!(ok("eq A1 off"), Command::Eq(0, false));
        assert_eq!(ok("EQ b2 ON"), Command::Eq(1, true));
        assert_eq!(
            ok("eq A1 2 gain 6"),
            Command::EqBand(0, 1, BandChange::Gain(6.0))
        );
        assert_eq!(
            ok("eq b2 4 FREQ 12000"),
            Command::EqBand(1, 3, BandChange::Freq(12_000.0))
        );
        assert_eq!(
            ok("eq A1 1 q 1.4"),
            Command::EqBand(0, 0, BandChange::Q(1.4))
        );
        assert_eq!(
            ok("eq A1 1 kind Bell"),
            Command::EqBand(0, 0, BandChange::Kind(BandKind::Peak))
        );
        assert_eq!(ok("ROUTE Analogue 1 a1"), Command::Route(1, 0, true));
        assert_eq!(ok("unroute cable b2"), Command::Route(0, 1, false));
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
    fn an_input_is_found_by_its_label_too() {
        let inputs = [
            search_name("CABLE Output", Some("Musik")),
            search_name("Analogue 1 + 2", Some("Mikro")),
            search_name("Spotify.exe", None),
        ];
        let (buses, labels) = (BUSES.map(str::to_owned), NO_LABELS.map(str::to_owned));
        let parsed = |line| parse(line, &inputs, &buses, &labels).expect(line);
        assert_eq!(parsed("mute mik"), Command::Mute(Target::Input(1), true));
        assert_eq!(parsed("mute cable"), Command::Mute(Target::Input(0), true));
        assert_eq!(
            parsed("mute spotify"),
            Command::Mute(Target::Input(2), true)
        );
    }

    const LABELS: [&str; 2] = ["Grosse Boxen", "Kopfhoerer"];

    #[test]
    fn a_bus_is_found_by_its_label_too() {
        let parsed = |line| parse(line, &INPUTS, &BUSES, &LABELS).expect(line);
        assert_eq!(parsed("mute boxen"), Command::Mute(Target::Bus(0), true));
        assert_eq!(
            parsed("gain Grosse Boxen -6 over 100"),
            Command::Gain(Target::Bus(0), -6.0, 100)
        );
        assert_eq!(parsed("mute KOPF"), Command::Mute(Target::Bus(1), true));
        // The name of a bus and what was found before stay what they were.
        assert_eq!(parsed("mute b2"), Command::Mute(Target::Bus(1), true));
        assert_eq!(parsed("mute cable"), Command::Mute(Target::Input(0), true));
        assert_eq!(parsed("route 1 boxen"), Command::Route(0, 0, true));
        assert_eq!(
            parsed("unroute 2 Grosse Boxen"),
            Command::Route(1, 0, false)
        );
        assert_eq!(parsed("route Analogue 1 kopf"), Command::Route(1, 1, true));
    }

    #[test]
    fn what_fits_more_than_one_is_refused() {
        let inputs = ["CABLE Output", "Boxen Links", "Boxen Rechts"];
        let labels = ["Grosse Boxen", "Kleine Boxen"];
        let wrong = |line, inputs: &[&str], labels: &[&str]| {
            parse(line, inputs, &BUSES, labels).expect_err(line).0
        };
        let both = wrong("mute links", &inputs, &["Links", ""]);
        assert!(both.contains("input 2 and bus A1"), "{both}");
        let buses = wrong("mute boxen", &INPUTS, &labels);
        assert!(
            buses.contains("several buses: Grosse Boxen, Kleine Boxen"),
            "{buses}"
        );
        let many = wrong("mute boxen", &inputs, &["Boxen", ""]);
        assert!(
            many.contains("several inputs (Boxen Links, Boxen Rechts) and bus A1"),
            "{many}"
        );
        let route = wrong("route 1 boxen", &INPUTS, &labels);
        assert!(route.contains("several buses"), "{route}");
        // "a b c": input "a" with the bus "b c", or input "a b" with the bus "c".
        let ways = wrong("route a b c", &["a", "a b"], &["b c", "c"]);
        assert!(ways.contains("several ways"), "{ways}");
        // The same input and bus in two readings is one meaning.
        assert_eq!(
            parse("route a b c", &["a b"], &BUSES, &["b c", ""]),
            Ok(Command::Route(0, 0, true))
        );
    }

    #[test]
    fn the_word_over_can_be_a_name() {
        let inputs = ["over", "game over 2", "Analogue"];
        let parsed = |line| parse(line, &inputs, &BUSES, &NO_LABELS).expect(line);
        assert_eq!(
            parsed("gain over 3"),
            Command::Gain(Target::Input(0), 3.0, 0)
        );
        assert_eq!(
            parsed("gain over 3 over 100"),
            Command::Gain(Target::Input(0), 3.0, 100)
        );
        // No value before `over`: not a ramp, but the end of a name and a number.
        assert_eq!(
            parsed("gain game over 2 -6"),
            Command::Gain(Target::Input(1), -6.0, 0)
        );
    }

    #[test]
    fn names_what_is_wrong() {
        assert!(bad("louder A1").contains("unknown command \"louder\""));
        assert!(bad("gain A1").contains("usage: gain"));
        assert!(bad("gain A1 laut").contains("\"laut\" is not a number"));
        assert!(bad("gain A1 inf").contains("not a number"));
        assert!(bad("gain A1 -6 over").contains("not a number"));
        assert!(bad("gain A1 -6 over bald").contains("\"bald\" is no time"));
        assert!(bad("gain A1 -6 over -5").contains("\"-5\" is no time"));
        assert!(bad("pan 1 0 over 1.5").contains("\"1.5\" is no time"));
        assert!(bad("scene").contains("scene needs a name"));
        assert!(bad("learn").contains("which address"));
        assert!(bad("unlearn").contains("which address"));
        assert!(bad("scenes abend").contains("too much"));
        assert!(bad("gain 4 0").contains("no input 4; inputs are 1 to 3"));
        assert!(bad("mute 0").contains("no input 0"));
        assert!(bad("mute vlc").contains("no input or bus matches \"vlc\""));
        assert!(bad("mute a").contains("several inputs"));
        assert!(bad("mute").contains("which input or bus"));
        assert!(bad("pan A1 0").contains("inputs only"));
        assert!(bad("gate A1 3").contains("gate works on inputs only"));
        assert!(bad("eq 2 off").contains("eq works on buses only"));
        assert!(bad("eq A1").contains("usage: eq <bus> on|off"));
        assert!(bad("eq off").contains("usage: eq <bus> on|off"));
        assert!(bad("eq A1 2 gain").contains("usage: eq"));
        assert!(bad("eq A1 5 gain 3").contains("no band 5; bands are 1 to 4"));
        assert!(bad("eq A1 0 gain 3").contains("no band 0"));
        assert!(bad("eq A1 2 gain laut").contains("\"laut\" is no gain in dB"));
        assert!(bad("eq A1 2 slope 3").contains("\"slope\" is no setting of a band"));
        assert!(bad("eq A1 2 kind notch").contains("kinds are low, bell and high"));
        assert!(bad("eq Z9 2 gain 3").contains("no input or bus matches"));
        assert!(bad("comp B2 off").contains("comp works on inputs only"));
        assert!(bad("gate 2").contains("usage: gate <input> <0..10|off>"));
        assert!(bad("comp off").contains("usage: comp <input> <0..10|off>"));
        assert!(bad("comp 2 viel").contains("\"viel\" is not a number"));
        assert!(bad("solo A1").contains("solo works on inputs only"));
        assert!(bad("unsolo").contains("which input or bus"));
        assert!(bad("route").contains("usage: route"));
        assert!(bad("route 1").contains("usage: route"));
        assert!(bad("route 1 C3").contains("no bus \"C3\""));
        assert!(bad("route A1 B2").contains("only an input"));
        assert!(bad("unroute vlc A1").contains("no input or bus matches \"vlc\""));
        assert!(bad("show me").contains("too much"));
        assert!(bad("save").contains("file name"));
    }
}
