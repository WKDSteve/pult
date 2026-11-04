//! The real ports, on a thread of their own: it looks every few seconds which are there, keeps the
//! wanted one open and hands on what comes in. Nothing here touches the mixer or the audio callbacks.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::thread;
use std::time::Duration;

use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};

/// How often the thread looks for a device that came or went.
const LOOK_EVERY: Duration = Duration::from_secs(2);

/// What comes from the device thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    /// A message of a port.
    Bytes {
        /// Its name.
        port: Arc<str>,
        /// The message.
        bytes: Vec<u8>,
    },
    /// The wanted device is open.
    Connected(String),
    /// It went away.
    Lost(String),
    /// Something did not work, once per kind of trouble.
    Problem(String),
}

/// What goes to the device thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outgoing {
    /// Light a button.
    Light([u8; 3]),
    /// Which input to keep open.
    Want(Wanted),
}

/// Which input to keep open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wanted {
    /// None: no MIDI is in use.
    Nothing,
    /// The port with this name.
    Device(String),
    /// All of them: a learning that has to find out which controller it is.
    Any,
}

/// The name of a port without what Windows wraps around it: `MIDIIN2 (APC mini mk2)` is
/// `APC mini mk2`, and so is `MIDIOUT2 (APC mini mk2)`.
#[must_use]
pub fn clean_name(name: &str) -> &str {
    let name = name.trim();
    let wrapped = name.split_once('(').filter(|(head, _)| {
        let head = head.trim_end();
        let word = head.trim_end_matches(|c: char| c.is_ascii_digit());
        word.eq_ignore_ascii_case("MIDIIN") || word.eq_ignore_ascii_case("MIDIOUT")
    });
    match wrapped.and_then(|(_, rest)| rest.strip_suffix(')')) {
        Some(inner) => inner.trim(),
        None => name,
    }
}

/// The port among `ports` that is the wanted `device`: with exactly that name, for a device with
/// several ports that clean to the same; else the first with `device` in its name, ignoring case.
#[must_use]
pub fn find_port<'a>(ports: &'a [String], device: &str) -> Option<&'a String> {
    ports
        .iter()
        .find(|name| *name == device)
        .or_else(|| ports.iter().find(|name| is_port(name, device)))
}

/// Whether the port `name` is the wanted `device`: its whole name, else a part of it, ignoring case.
#[must_use]
pub fn is_port(name: &str, device: &str) -> bool {
    clean_name(name)
        .to_ascii_lowercase()
        .contains(&device.to_ascii_lowercase())
}

/// The output among `outputs` that belongs to the input `input`: the same name when there is one,
/// `MIDIOUT2 (x)` for `MIDIIN2 (x)`, else what cleans to the same.
#[must_use]
pub fn output_for<'a>(input: &str, outputs: &'a [String]) -> Option<&'a str> {
    let twin = input.replacen("MIDIIN", "MIDIOUT", 1);
    let input = clean_name(input);
    outputs
        .iter()
        .find(|name| **name == twin)
        .or_else(|| {
            outputs
                .iter()
                .find(|name| clean_name(name).eq_ignore_ascii_case(input))
        })
        .or_else(|| outputs.iter().find(|name| is_port(name, input)))
        .map(String::as_str)
}

/// The names of the ports there are: inputs, outputs.
///
/// # Errors
///
/// When the system's MIDI cannot be asked.
pub fn ports() -> Result<(Vec<String>, Vec<String>), String> {
    let input = MidiInput::new("pult").map_err(|e| e.to_string())?;
    let output = MidiOutput::new("pult").map_err(|e| e.to_string())?;
    let name_in = |port| input.port_name(port).ok();
    let name_out = |port| output.port_name(port).ok();
    Ok((
        input.ports().iter().filter_map(name_in).collect(),
        output.ports().iter().filter_map(name_out).collect(),
    ))
}

/// What is open.
#[derive(Default)]
struct Open {
    inputs: Vec<(String, MidiInputConnection<()>)>,
    output: Option<MidiOutputConnection>,
}

/// The thread's work, until the other side of `outgoing` is gone.
pub fn run(outgoing: &Receiver<Outgoing>, incoming: &SyncSender<Incoming>) {
    let mut wanted = Wanted::Nothing;
    let mut open = Open::default();
    let mut told = None;
    loop {
        follow(&wanted, &mut open, incoming, &mut told);
        match outgoing.recv_timeout(LOOK_EVERY) {
            Ok(Outgoing::Want(next)) => wanted = next,
            Ok(Outgoing::Light(message)) => {
                if let Some(output) = &mut open.output {
                    // A device on its way out; the next look notices.
                    let _ = output.send(&message);
                }
                // Lights come in runs; no need to look at the ports for each.
                while let Ok(next) = outgoing.try_recv() {
                    match next {
                        Outgoing::Light(message) => {
                            if let Some(output) = &mut open.output {
                                let _ = output.send(&message);
                            }
                        }
                        Outgoing::Want(next) => wanted = next,
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn tell(incoming: &SyncSender<Incoming>, what: Incoming) {
    match incoming.try_send(what) {
        Ok(()) | Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {}
    }
}

/// Brings what is open in line with what is wanted and what is there.
fn follow(
    wanted: &Wanted,
    open: &mut Open,
    incoming: &SyncSender<Incoming>,
    told: &mut Option<String>,
) {
    let (inputs, outputs) = match ports() {
        Ok(found) => found,
        Err(why) => {
            if told.as_deref() != Some(why.as_str()) {
                tell(incoming, Incoming::Problem(why.clone()));
                *told = Some(why);
            }
            return;
        }
    };
    let chosen: Vec<&String> = match wanted {
        Wanted::Nothing => Vec::new(),
        Wanted::Any => inputs.iter().collect(),
        Wanted::Device(device) => find_port(&inputs, device).into_iter().collect(),
    };
    // What went, or is wanted no longer.
    let gone: Vec<String> = open
        .inputs
        .iter()
        .filter(|(name, _)| !chosen.contains(&name))
        .map(|(name, _)| name.clone())
        .collect();
    if !gone.is_empty() {
        open.inputs.retain(|(name, _)| !gone.contains(name));
        open.output = None;
        for name in &gone {
            // Only the device in use: a port a learning listened to does not matter.
            if matches!(wanted, Wanted::Device(_)) || !inputs.contains(name) {
                tell(incoming, Incoming::Lost(name.clone()));
            }
        }
    }
    for name in chosen {
        if open.inputs.iter().any(|(known, _)| known == name) {
            continue;
        }
        match connect_input(name, incoming) {
            Ok(connection) => {
                open.inputs.push((name.clone(), connection));
                if matches!(wanted, Wanted::Device(_)) {
                    open.output = output_for(name, &outputs).and_then(connect_output);
                    tell(incoming, Incoming::Connected(name.clone()));
                }
                *told = None;
            }
            Err(why) => {
                let why = format!("{name}: {why}");
                if told.as_deref() != Some(why.as_str()) {
                    tell(incoming, Incoming::Problem(why.clone()));
                    *told = Some(why);
                }
            }
        }
    }
}

fn connect_input(
    name: &str,
    incoming: &SyncSender<Incoming>,
) -> Result<MidiInputConnection<()>, String> {
    let mut input = MidiInput::new("pult").map_err(|e| e.to_string())?;
    // The clock of a controller is thirty messages a second of nothing.
    input.ignore(midir::Ignore::All);
    let port = input
        .ports()
        .into_iter()
        .find(|port| input.port_name(port).is_ok_and(|n| n == name))
        .ok_or("it went away")?;
    let label: Arc<str> = Arc::from(name);
    let send = incoming.clone();
    input
        .connect(
            &port,
            "pult",
            move |_, bytes, ()| {
                // Too much at once: what is lost here, a later value of the same fader replaces.
                let _ = send.try_send(Incoming::Bytes {
                    port: Arc::clone(&label),
                    bytes: bytes.to_vec(),
                });
            },
            (),
        )
        // Often: another program has it, which on Windows nobody can share.
        .map_err(|e| e.to_string())
}

fn connect_output(name: &str) -> Option<MidiOutputConnection> {
    let output = MidiOutput::new("pult").ok()?;
    let port = output
        .ports()
        .into_iter()
        .find(|port| output.port_name(port).is_ok_and(|n| n == name))?;
    output.connect(&port, "pult").ok()
}

/// Prints what every input says for `seconds`: to see what a controller sends. Nothing else of
/// pult may have the ports open meanwhile.
///
/// # Errors
///
/// When the system's MIDI cannot be asked.
pub fn listen(seconds: u64) -> Result<(), String> {
    let (inputs, _) = ports()?;
    let (incoming, from_ports) = std::sync::mpsc::sync_channel(4096);
    let mut open = Vec::new();
    for name in &inputs {
        match connect_input(name, &incoming) {
            Ok(connection) => {
                println!("listening to {name}");
                open.push(connection);
            }
            Err(why) => println!("cannot open {name}: {why}"),
        }
    }
    let until = std::time::Instant::now() + Duration::from_secs(seconds);
    while let Some(left) = until.checked_duration_since(std::time::Instant::now()) {
        if let Ok(Incoming::Bytes { port, bytes }) = from_ports.recv_timeout(left) {
            println!("{port}: {bytes:02X?}");
        }
    }
    Ok(())
}

/// Starts the thread.
///
/// # Errors
///
/// When the system gives no thread.
pub fn spawn(
    outgoing: Receiver<Outgoing>,
    incoming: SyncSender<Incoming>,
) -> Result<thread::JoinHandle<()>, String> {
    thread::Builder::new()
        .name("pult-midi".to_owned())
        .spawn(move || run(&outgoing, &incoming))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wrapping_of_windows_goes() {
        assert_eq!(clean_name("MIDIIN2 (APC mini mk2)"), "APC mini mk2");
        assert_eq!(clean_name("MIDIOUT2 (APC mini mk2)"), "APC mini mk2");
        assert_eq!(clean_name("MIDIIN (x)"), "x");
        assert_eq!(clean_name("APC MINI"), "APC MINI");
        assert_eq!(clean_name("nanoKONTROL (Port 1)"), "nanoKONTROL (Port 1)");
        assert_eq!(clean_name(" MIDIINSIDE (x) "), "MIDIINSIDE (x)");
    }

    #[test]
    fn of_two_ports_of_one_device_the_exact_name_decides() {
        let ports = vec![
            "APC Key 25 mk2".to_owned(),
            "MIDIIN2 (APC Key 25 mk2)".to_owned(),
        ];
        assert_eq!(
            find_port(&ports, "MIDIIN2 (APC Key 25 mk2)"),
            Some(&ports[1])
        );
        assert_eq!(find_port(&ports, "APC Key 25 mk2"), Some(&ports[0]));
        assert_eq!(find_port(&ports, "apc key"), Some(&ports[0]));
        assert_eq!(find_port(&ports, "nano"), None);
    }

    #[test]
    fn a_port_is_found_by_a_part_of_its_name() {
        assert!(is_port("MIDIIN2 (APC MINI)", "apc mini"));
        assert!(is_port("APC MINI", "APC"));
        assert!(!is_port("MIDIIN2 (nanoKONTROL)", "APC"));
        // The word around the name is not part of the name.
        assert!(!is_port("MIDIIN2 (APC MINI)", "MIDIIN"));
    }

    #[test]
    fn the_output_of_an_input_has_its_name() {
        let outputs = vec![
            "Microsoft GS Wavetable Synth".to_owned(),
            "APC mini mk2 Notes".to_owned(),
            "MIDIOUT2 (APC mini mk2 Control)".to_owned(),
        ];
        assert_eq!(
            output_for("MIDIIN2 (APC mini mk2 Control)", &outputs),
            Some("MIDIOUT2 (APC mini mk2 Control)")
        );
        let twins = vec![
            "APC Key 25 mk2".to_owned(),
            "MIDIOUT2 (APC Key 25 mk2)".to_owned(),
        ];
        assert_eq!(
            output_for("MIDIIN2 (APC Key 25 mk2)", &twins),
            Some("MIDIOUT2 (APC Key 25 mk2)")
        );
        assert_eq!(output_for("APC Key 25 mk2", &twins), Some("APC Key 25 mk2"));
        assert_eq!(
            output_for("APC mini mk2 Notes", &outputs),
            Some("APC mini mk2 Notes")
        );
        assert_eq!(output_for("Other", &outputs), None);
    }

    #[test]
    fn asking_for_the_ports_works_without_any_device() {
        // Whatever this machine has, asking does not fail.
        assert!(ports().is_ok());
    }

    #[test]
    fn the_thread_ends_when_its_owner_goes() {
        let (to_thread, outgoing) = std::sync::mpsc::channel();
        let (incoming, _from_thread) = std::sync::mpsc::sync_channel(8);
        let thread = spawn(outgoing, incoming).expect("a thread");
        to_thread
            .send(Outgoing::Want(Wanted::Nothing))
            .expect("sent");
        drop(to_thread);
        thread.join().expect("it ended");
    }
}
