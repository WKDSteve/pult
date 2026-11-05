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
    /// It is no longer in use, and still there.
    Closed(String),
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

/// Whether the port `name` is only a keyboard, on a device that has its controls on a port of their
/// own. Measured on the APC Key 25 mk2: `APC Key 25 mk2` is its keys, `MIDIIN2 (APC Key 25 mk2)` its
/// knobs, pads and buttons, with the same note numbers on both. A key is no control to bind, and
/// choosing this port would leave the controls out.
#[must_use]
pub fn is_only_keys(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("apc key") && lower.contains("mk2") && clean_name(name) == name.trim()
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
    /// The port that was told to be the device in use.
    device: Option<String>,
}

/// What to do about the ports.
#[derive(Debug, Default, PartialEq, Eq)]
struct Plan {
    /// Inputs to close.
    close: Vec<String>,
    /// Inputs to open.
    open: Vec<String>,
    /// The device in use went away: unplugged.
    lost: Option<String>,
    /// The device in use is no longer wanted, and still there.
    closed: Option<String>,
    /// The port that is the device in use from now on, also when it is open already: a learning
    /// listened to it before it was chosen.
    announce: Option<String>,
}

/// What `wanted` comes to, with the ports `inputs` there and `open` open, and `device` in use.
fn plan(wanted: &Wanted, inputs: &[String], open: &[String], device: Option<&str>) -> Plan {
    let chosen: Vec<&String> = match wanted {
        Wanted::Nothing => Vec::new(),
        Wanted::Any => inputs.iter().collect(),
        Wanted::Device(device) => find_port(inputs, device).into_iter().collect(),
    };
    let mut plan = Plan {
        close: open
            .iter()
            .filter(|name| !chosen.contains(name))
            .cloned()
            .collect(),
        open: chosen
            .iter()
            .filter(|name| !open.contains(name))
            .map(|name| (*name).clone())
            .collect(),
        ..Plan::default()
    };
    if let Some(device) = device
        && !chosen.iter().any(|name| *name == device)
    {
        if inputs.iter().any(|name| name == device) {
            plan.closed = Some(device.to_owned());
        } else {
            plan.lost = Some(device.to_owned());
        }
    }
    if let (Wanted::Device(_), Some(name)) = (wanted, chosen.first())
        && device != Some(name.as_str())
    {
        plan.announce = Some((*name).clone());
    }
    plan
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
    let open_names: Vec<String> = open.inputs.iter().map(|(name, _)| name.clone()).collect();
    let plan = plan(wanted, &inputs, &open_names, open.device.as_deref());
    open.inputs.retain(|(name, _)| !plan.close.contains(name));
    if let Some(name) = plan.lost {
        open.output = None;
        open.device = None;
        tell(incoming, Incoming::Lost(name));
    }
    if let Some(name) = plan.closed {
        open.output = None;
        open.device = None;
        tell(incoming, Incoming::Closed(name));
    }
    for name in &plan.open {
        match connect_input(name, incoming) {
            Ok(connection) => {
                open.inputs.push((name.clone(), connection));
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
    if let Some(name) = plan.announce
        && open.inputs.iter().any(|(known, _)| *known == name)
    {
        open.output = output_for(&name, &outputs).and_then(connect_output);
        tell(incoming, Incoming::Connected(name.clone()));
        open.device = Some(name);
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

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    const KEYS: &str = "APC Key 25 mk2";
    const CONTROLS: &str = "MIDIIN2 (APC Key 25 mk2)";

    #[test]
    fn a_learning_listens_to_all_and_tells_of_no_device() {
        let inputs = names(&[KEYS, CONTROLS]);
        assert_eq!(
            plan(&Wanted::Any, &inputs, &[], None),
            Plan {
                open: inputs.clone(),
                ..Plan::default()
            }
        );
    }

    /// Measured on the real device: the port was open from the learning, so it never came to be
    /// connected, and no output and no light came with it.
    #[test]
    fn a_port_chosen_while_it_is_open_is_announced_all_the_same() {
        let inputs = names(&[KEYS, CONTROLS]);
        assert_eq!(
            plan(&Wanted::Device(CONTROLS.to_owned()), &inputs, &inputs, None),
            Plan {
                close: names(&[KEYS]),
                announce: Some(CONTROLS.to_owned()),
                ..Plan::default()
            },
            "the other port closes, and that is no device lost"
        );
    }

    #[test]
    fn a_device_is_opened_and_announced_once() {
        let inputs = names(&[KEYS, CONTROLS]);
        let wanted = Wanted::Device(CONTROLS.to_owned());
        assert_eq!(
            plan(&wanted, &inputs, &[], None),
            Plan {
                open: names(&[CONTROLS]),
                announce: Some(CONTROLS.to_owned()),
                ..Plan::default()
            }
        );
        assert_eq!(
            plan(&wanted, &inputs, &names(&[CONTROLS]), Some(CONTROLS)),
            Plan::default()
        );
    }

    #[test]
    fn an_unplugged_device_is_lost_and_found_again() {
        let wanted = Wanted::Device(CONTROLS.to_owned());
        assert_eq!(
            plan(&wanted, &[], &names(&[CONTROLS]), Some(CONTROLS)),
            Plan {
                close: names(&[CONTROLS]),
                lost: Some(CONTROLS.to_owned()),
                ..Plan::default()
            }
        );
        assert_eq!(
            plan(&wanted, &names(&[KEYS, CONTROLS]), &[], None),
            Plan {
                open: names(&[CONTROLS]),
                announce: Some(CONTROLS.to_owned()),
                ..Plan::default()
            }
        );
    }

    #[test]
    fn a_device_no_longer_wanted_is_closed_not_lost() {
        let inputs = names(&[KEYS, CONTROLS]);
        assert_eq!(
            plan(
                &Wanted::Nothing,
                &inputs,
                &names(&[CONTROLS]),
                Some(CONTROLS)
            ),
            Plan {
                close: names(&[CONTROLS]),
                closed: Some(CONTROLS.to_owned()),
                ..Plan::default()
            }
        );
    }

    #[test]
    fn the_keys_of_an_apc_key_are_told_from_its_controls() {
        assert!(is_only_keys("APC Key 25 mk2"));
        assert!(
            !is_only_keys("APC Key 25"),
            "not measured; filtering it could leave nothing"
        );
        assert!(!is_only_keys("MIDIIN2 (APC Key 25 mk2)"));
        assert!(!is_only_keys("APC MINI"));
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
        // Whatever this machine has, asking does not fail; a Linux kernel without the ALSA
        // sequencer (WSL) has nothing to ask.
        if cfg!(target_os = "linux") && !std::path::Path::new("/dev/snd/seq").exists() {
            return;
        }
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
