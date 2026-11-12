//! Commands to a pult that is already running: the words of its input line, from any terminal,
//! script or editor.

use std::fmt::Write as _;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use pult_core::control::{self, Command};
use pult_core::protocol::{Op, State};
use pult_hub::{Client, ClientError, start_window};
use pult_midi::device::{listen, ports};

use crate::args::Remote;

/// Exit status for words that are no command.
const USAGE_STATUS: u8 = 2;

/// Carries out `request`: what it did goes to stdout, what went wrong to stderr, with a status
/// that is not zero.
pub fn run(request: &Remote) -> ExitCode {
    match answer(request) {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(Problem::Words(why)) => {
            eprintln!("pult: {why}");
            ExitCode::from(USAGE_STATUS)
        }
        Err(Problem::Client(why)) => {
            eprintln!("pult: {why}");
            ExitCode::FAILURE
        }
    }
}

enum Problem {
    /// The words are no command for this route.
    Words(String),
    /// The running pult is not there or said no.
    Client(ClientError),
}

impl From<ClientError> for Problem {
    fn from(error: ClientError) -> Self {
        Self::Client(error)
    }
}

/// How long `pult learn` waits: a little longer than the route does.
const LEARN_WAIT: Duration = Duration::from_secs(32);
const LEARN_LOOK: Duration = Duration::from_millis(250);

/// How long a window that was just started may take to bring up its route.
const ROUTE_TRIES: usize = 60;
const ROUTE_WAIT: Duration = Duration::from_millis(250);

/// Starts the window unless a route runs, and shows the page if asked to and there is a route.
fn start(open: bool) -> Result<Vec<String>, Problem> {
    let mut lines = Vec::new();
    let client = match Client::find() {
        Ok(client) => {
            lines.push("pult is running already".to_owned());
            client
        }
        Err(ClientError::NotRunning) => {
            let program = std::env::current_exe()
                .map_err(|e| Problem::Client(ClientError::Failed(e.to_string())))?;
            start_window(&program).map_err(|why| Problem::Client(ClientError::Failed(why)))?;
            lines.push("started the window".to_owned());
            let Some(client) = wait_for_route() else {
                lines.push("no route yet: pick a profile in the window".to_owned());
                return Ok(lines);
            };
            client
        }
        Err(other) => return Err(other.into()),
    };
    if open {
        client.open_page()?;
    }
    Ok(lines)
}

fn wait_for_route() -> Option<Client> {
    for _ in 0..ROUTE_TRIES {
        if let Ok(client) = Client::find() {
            return Some(client);
        }
        std::thread::sleep(ROUTE_WAIT);
    }
    None
}

/// The ports of this machine, one per line.
fn midi_devices() -> Result<Vec<String>, Problem> {
    let (inputs, outputs) = ports().map_err(|why| Problem::Client(ClientError::Failed(why)))?;
    let mut lines = vec!["inputs:".to_owned()];
    lines.extend(inputs.iter().map(|name| format!("  {name}")));
    lines.push("outputs:".to_owned());
    lines.extend(outputs.iter().map(|name| format!("  {name}")));
    Ok(lines)
}

/// Fails with what keeps MIDI off, if anything does: asking would look as if it had a chance.
fn refuse_if_midi_is_off(client: &mut Client, what: &str) -> Result<(), Problem> {
    match client.state()?.midi.and_then(|midi| midi.problem) {
        Some(why) => Err(Problem::Client(ClientError::Failed(format!(
            "cannot {what}: {why}"
        )))),
        None => Ok(()),
    }
}

/// Asks for a learning and waits until the movement came or it gave up.
fn learn(client: &mut Client, target: &str) -> Result<Vec<String>, Problem> {
    refuse_if_midi_is_off(client, "learn")?;
    for line in client.op(&Op::Learn {
        target: target.to_owned(),
    })? {
        println!("{line}");
    }
    let until = Instant::now() + LEARN_WAIT;
    // A learning asked for just now shows only with the next state.
    std::thread::sleep(LEARN_LOOK);
    while Instant::now() < until {
        let Some(midi) = client.state()?.midi else {
            return Err(Problem::Client(ClientError::Failed(
                "this pult has no MIDI".to_owned(),
            )));
        };
        if midi.learning.as_deref() != Some(target) {
            return match midi.bindings.iter().find(|b| b.target == target) {
                Some(bound) => Ok(vec![format!("{} -> {target}", bound.control)]),
                None => Err(Problem::Client(ClientError::Failed(format!(
                    "nothing moved for {target}"
                )))),
            };
        }
        std::thread::sleep(LEARN_LOOK);
    }
    Err(Problem::Client(ClientError::Failed(format!(
        "nothing moved for {target}"
    ))))
}

/// The device and what is bound.
fn midi_shown(state: &State) -> Vec<String> {
    let Some(midi) = &state.midi else {
        return vec!["this pult has no MIDI".to_owned()];
    };
    if let Some(why) = &midi.problem {
        return vec![format!("MIDI is off: {why}")];
    }
    let mut lines = vec![match (&midi.device, midi.connected) {
        (Some(device), true) => format!("device: {device}"),
        (Some(device), false) => format!("device: {device} (not there)"),
        (None, _) => "no device yet; `pult learn <address>` finds it".to_owned(),
    }];
    lines.extend(
        midi.bindings
            .iter()
            .map(|bound| format!("{} -> {}", bound.control, bound.target)),
    );
    if let Some(target) = &midi.learning {
        lines.push(format!("waiting for a movement for {target}"));
    }
    lines
}

fn answer(request: &Remote) -> Result<Vec<String>, Problem> {
    if let Remote::Start { open } = request {
        return start(*open);
    }
    if *request == Remote::MidiDevices {
        return midi_devices();
    }
    if let Remote::MidiListen(seconds) = request {
        listen(*seconds).map_err(|why| Problem::Client(ClientError::Failed(why)))?;
        return Ok(Vec::new());
    }
    let mut client = match Client::find() {
        // Nothing to end is what was asked for.
        Err(ClientError::NotRunning) if *request == Remote::Stop => {
            return Ok(vec!["no pult is running".to_owned()]);
        }
        found => found?,
    };
    match request {
        Remote::Start { .. } | Remote::MidiDevices | Remote::MidiListen(_) => {
            unreachable!("handled above")
        }
        Remote::Learn(target) => learn(&mut client, target),
        Remote::Unlearn(target) => {
            refuse_if_midi_is_off(&mut client, "unlearn")?;
            client.op(&Op::Unlearn {
                target: target.clone(),
            })?;
            Ok(vec![format!("asked to free {target}")])
        }
        Remote::Midi => Ok(midi_shown(&client.state()?)),
        Remote::Stop => {
            client.stop()?;
            Ok(vec!["stopped".to_owned()])
        }
        Remote::Open { print: true } => Ok(vec![client.page_link()?]),
        Remote::Open { print: false } => {
            client.open_page()?;
            Ok(Vec::new())
        }
        Remote::Show { json: true } => {
            let state = client.state()?;
            let text = serde_json::to_string(&state)
                .map_err(|e| Problem::Client(ClientError::Failed(e.to_string())))?;
            Ok(vec![text])
        }
        Remote::Show { json: false } => Ok(shown(&client.state()?)),
        Remote::Scenes => Ok(client.state()?.scenes),
        Remote::Save => Ok(client.op(&Op::Save {})?),
        Remote::Line(line) => {
            let state = client.state()?;
            let names: Vec<String> = state
                .inputs
                .iter()
                .map(|input| control::search_name(&input.name, input.label.as_deref()))
                .collect();
            let buses: Vec<String> = state.buses.iter().map(|bus| bus.name.clone()).collect();
            let labels: Vec<String> = state
                .buses
                .iter()
                .map(|bus| bus.label.clone().unwrap_or_default())
                .collect();
            let command = control::parse(line, &names, &buses, &labels)
                .map_err(|why| Problem::Words(why.to_string()))?;
            debug_assert!(
                !matches!(command, Command::Stop),
                "args lets no stop through"
            );
            let op = Op::of_command(&command, &state)
                .ok_or_else(|| Problem::Words(format!("\"{line}\" is nothing to send")))?;
            Ok(client.op(&op)?)
        }
    }
}

/// One line per input and bus, as `show` in the input line prints them, and the scenes.
fn shown(state: &State) -> Vec<String> {
    let mut lines = Vec::with_capacity(state.inputs.len() + state.buses.len() + 1);
    for input in &state.inputs {
        let mut line = format!(
            "[{}] {}  gain {:.1} dB  pan {:.2}  -> {}",
            input.number,
            input.label.as_deref().unwrap_or(&input.name),
            input.gain_db,
            input.pan,
            input.buses.join(", ")
        );
        for (what, amount) in [("gate", input.gate), ("comp", input.comp)] {
            if amount > 0.0 {
                let _ = write!(line, "  {what} {amount:.1}");
            }
        }
        if input.muted {
            line.push_str("  muted");
        }
        if input.solo {
            line.push_str("  solo");
        }
        lines.push(line);
    }
    for bus in &state.buses {
        let mut line = format!("{} {}  gain {:.1} dB", bus.name, bus.device, bus.gain_db);
        // A run older than equalizers names no bands: nothing to say of them.
        if !bus.eq.is_empty() {
            line.push_str(&pult_core::view::eq_summary(bus.eq_on, &bus.eq));
        }
        // A run older than limiters has no ceiling: nothing to say of it either.
        if bus.limit != pult_core::view::LimitSettings::OLDER_RUN {
            line.push_str(&bus.limit.summary());
        }
        if bus.muted {
            line.push_str("  muted");
        }
        lines.push(line);
    }
    if !state.scenes.is_empty() {
        lines.push(format!("scenes: {}", state.scenes.join(", ")));
    }
    lines
}
