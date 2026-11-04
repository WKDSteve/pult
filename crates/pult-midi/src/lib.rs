//! A MIDI controller on pult, for whoever owns a route (`pult run`, the window). A movement becomes
//! the same request every client sends, so nothing here reaches around the desk, and all of it runs
//! on the owner's thread and the device's own: never in an audio callback.
//!
//! No MIDI is touched until there is something to do: no file with bindings, no learning, no thread
//! and no open port: a port can have one user only, depending on the driver, and it might be
//! another program's.

pub mod device;
pub mod file;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread::JoinHandle;

use pult_core::midi::{Map, Message, MidiStatus, Palette};
use pult_core::protocol::Op;
use pult_core::view::{Desk, MixerView};
use pult_hub::{MidiAsk, Pump};

use device::{Incoming, Outgoing, Wanted, is_only_keys};
use file::Saved;

/// What may wait from the device. A fader in a hurry sends about a thousand messages a second at
/// the very most; beyond that the oldest news is worthless anyway.
const INBOX: usize = 1024;

/// How many buttons are lit at one go: a slow device must not choke on a scene.
const LIGHTS_AT_ONCE: usize = 16;

/// A controller on a route. Its flags have nothing to do with each other: no state machine in them.
#[allow(clippy::struct_excessive_bools)]
pub struct Midi {
    path: PathBuf,
    map: Map,
    device: Option<String>,
    connected: bool,
    /// The file does not read: nothing is learned, so it is not overwritten.
    broken: bool,
    inbox: Option<Receiver<Incoming>>,
    outbox: Option<Sender<Outgoing>>,
    thread: Option<JoinHandle<()>>,
    /// Whether the thread is to be started when needed; not with channels from a test.
    spawns: bool,
    wanted: Wanted,
    notes: Vec<String>,
    /// The learning under way said already that keys are no controls.
    told_keys: bool,
}

impl Midi {
    /// The controller the file in the config folder of the running hub names, if any.
    #[must_use]
    pub fn start() -> Self {
        match pult_hub::config_folder() {
            Ok(dir) => Self::with_file(dir.join("midi.toml"), true),
            Err(why) => {
                let mut midi = Self::with_file(PathBuf::new(), false);
                midi.broken = true;
                midi.notes.push(format!("no MIDI: {why}"));
                midi
            }
        }
    }

    fn with_file(path: PathBuf, spawns: bool) -> Self {
        let (saved, problem) = match file::load(&path) {
            Ok(saved) => (saved, None),
            Err(why) => (Saved::default(), Some(why)),
        };
        let mut midi = Self {
            path,
            map: Map::new(saved.bindings),
            device: saved.device,
            connected: false,
            broken: problem.is_some(),
            inbox: None,
            outbox: None,
            thread: None,
            spawns,
            wanted: Wanted::Nothing,
            told_keys: false,
            notes: problem
                .into_iter()
                .map(|why| format!("no MIDI: {why}"))
                .collect(),
        };
        midi.follow_wanted();
        midi
    }

    /// Whether a controller may speak: the owner then looks more often than its status line.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.wanted != Wanted::Nothing
    }

    /// What a client reads.
    #[must_use]
    pub fn status(&self) -> MidiStatus {
        self.map.status(self.device.clone(), self.connected)
    }

    /// Lets the next movement bind `target`, which the caller checked.
    pub fn learn(&mut self, target: String) {
        if self.broken {
            self.notes
                .push("cannot learn: repair or remove midi.toml first".to_owned());
            return;
        }
        self.map.learn(target);
        self.told_keys = false;
        self.follow_wanted();
    }

    /// Takes the binding of `target` away.
    pub fn forget(&mut self, target: &str) {
        if self.map.forget(target) {
            self.notes.push(format!("{target} is free"));
            self.save();
        } else {
            self.notes.push(format!("nothing is bound to {target}"));
        }
        self.follow_wanted();
    }

    /// What the wanted port is by now, for the thread, which starts when there is something to do.
    fn follow_wanted(&mut self) {
        let wanted = if self.broken {
            Wanted::Nothing
        } else if let Some(device) = &self.device {
            if self.map.bindings().is_empty() && self.map.learning().is_none() {
                Wanted::Nothing
            } else {
                Wanted::Device(device.clone())
            }
        } else if self.map.learning().is_some() {
            Wanted::Any
        } else {
            Wanted::Nothing
        };
        if wanted == self.wanted {
            return;
        }
        self.wanted = wanted;
        if self.outbox.is_none() && self.spawns {
            self.start_thread();
        }
        self.send(Outgoing::Want(self.wanted.clone()));
    }

    fn start_thread(&mut self) {
        let (to_thread, outgoing) = mpsc::channel();
        let (incoming, from_thread): (SyncSender<Incoming>, _) = mpsc::sync_channel(INBOX);
        match device::spawn(outgoing, incoming) {
            Ok(thread) => {
                self.thread = Some(thread);
                self.outbox = Some(to_thread);
                self.inbox = Some(from_thread);
            }
            Err(why) => self.notes.push(format!("no MIDI: {why}")),
        }
    }

    fn send(&self, what: Outgoing) {
        if let Some(outbox) = &self.outbox {
            // A thread that ended takes nothing any more; the owner plays on.
            let _ = outbox.send(what);
        }
    }

    fn save(&mut self) {
        let saved = Saved {
            device: self.device.clone(),
            bindings: self.map.bindings().to_vec(),
        };
        if let Err(why) = file::save(&self.path, &saved) {
            self.notes.push(why);
        }
    }

    /// Follows what the device said, against `view`, and asks `desk` for what that comes to. Returns
    /// what to tell the person. A fader's every step is not worth a line, nor a light per message.
    pub fn poll(&mut self, desk: &mut dyn Desk, view: &MixerView) -> Vec<String> {
        for message in self.drain() {
            if is_only_keys(&message.1) {
                if self.map.learning().is_some() && !self.told_keys {
                    self.told_keys = true;
                    self.notes
                        .push("that is a key; use a knob, a pad or a button".to_owned());
                }
                continue;
            }
            let step = self.map.step(message.0, view);
            if step.learned {
                if self.device.is_none() {
                    self.device = Some(message.1);
                }
                self.save();
                self.follow_wanted();
            }
            self.notes.extend(step.note);
            if let Some(op) = step.op {
                let scene = matches!(op, Op::Scene { .. });
                match op.action(view).and_then(|action| desk.act(action)) {
                    Ok(lines) if scene => self.notes.extend(lines),
                    Ok(_) => {}
                    Err(why) => self.notes.push(why),
                }
            }
        }
        if let Some(note) = self.map.expire(view) {
            self.notes.push(note);
            self.follow_wanted();
        }
        std::mem::take(&mut self.notes)
    }

    /// The messages that came, in order, with only the newest value of each fader of this batch:
    /// the mixer cannot do anything with the ones it has already passed.
    fn drain(&mut self) -> Vec<(Message, String)> {
        let Some(inbox) = &self.inbox else {
            return Vec::new();
        };
        let mut messages: Vec<(Message, String)> = Vec::new();
        let mut news = Vec::new();
        while let Ok(what) = inbox.try_recv() {
            match what {
                Incoming::Bytes { port, bytes } => {
                    let Some(message) = Message::parse(&bytes) else {
                        continue;
                    };
                    if matches!(message, Message::Cc { .. }) {
                        let control = message.control();
                        messages.retain(|(known, _)| known.control() != control);
                    }
                    messages.push((message, port.to_string()));
                }
                other => news.push(other),
            }
        }
        for what in news {
            match what {
                Incoming::Connected(name) => {
                    self.connected = true;
                    self.map.forget_lights();
                    self.notes.push(format!("MIDI device {name} is there"));
                }
                Incoming::Lost(name) => {
                    self.connected = false;
                    self.notes.push(format!("MIDI device {name} is gone"));
                }
                Incoming::Closed(_) => self.connected = false,
                Incoming::Problem(why) => self.notes.push(format!("MIDI: {why}")),
                Incoming::Bytes { .. } => {}
            }
        }
        messages
    }

    /// Brings the lights up to `view`.
    pub fn lights(&mut self, view: &MixerView) {
        let Some(device) = self.device.as_deref().filter(|_| self.connected) else {
            return;
        };
        let palette = Palette::for_device(device);
        for message in self.map.lights(view, palette, LIGHTS_AT_ONCE) {
            self.send(Outgoing::Light(message));
        }
    }
}

impl Drop for Midi {
    fn drop(&mut self) {
        self.outbox = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// One look of an owner of a route: hands `view` to the clients and the controller, carries out
/// what they asked for. Returns what to tell the person. The view gets the status of MIDI first.
pub fn serve(
    pump: &mut Pump,
    midi: &mut Midi,
    desk: &mut dyn Desk,
    view: &mut MixerView,
) -> Vec<String> {
    view.midi = Some(midi.status());
    let mut lines = pump.pump(desk, view);
    for ask in pump.take_midi_asks() {
        match ask {
            MidiAsk::Learn(target) => midi.learn(target),
            MidiAsk::Forget(target) => midi.forget(&target),
        }
    }
    lines.extend(midi.poll(desk, view));
    midi.lights(view);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use pult_core::view::FakeDesk;

    /// A folder for one test, removed when it ends.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("pult-midi-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("a folder");
            Self(dir)
        }

        fn file(&self) -> PathBuf {
            self.0.join("midi.toml")
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A controller with channels instead of a device.
    struct Bench {
        midi: Midi,
        desk: FakeDesk,
        to_midi: SyncSender<Incoming>,
        from_midi: Receiver<Outgoing>,
    }

    impl Bench {
        fn new(folder: &Folder) -> Self {
            let mut midi = Midi::with_file(folder.file(), false);
            let (to_midi, inbox) = mpsc::sync_channel(INBOX);
            let (outbox, from_midi) = mpsc::channel();
            midi.inbox = Some(inbox);
            midi.outbox = Some(outbox);
            Self {
                midi,
                desk: FakeDesk::new(),
                to_midi,
                from_midi,
            }
        }

        fn device_says(&self, bytes: &[u8]) {
            self.to_midi
                .send(Incoming::Bytes {
                    port: "APC MINI".into(),
                    bytes: bytes.to_vec(),
                })
                .expect("sent");
        }

        fn poll(&mut self) -> Vec<String> {
            let view = self.desk.view();
            self.midi.poll(&mut self.desk, &view)
        }

        fn sent(&self) -> Vec<Outgoing> {
            self.from_midi.try_iter().collect()
        }
    }

    #[test]
    fn nothing_is_opened_without_bindings_or_learning() {
        let folder = Folder::new("idle");
        let midi = Midi::with_file(folder.file(), true);
        assert!(!midi.is_active());
        assert!(midi.thread.is_none());
        assert_eq!(midi.notes, Vec::<String>::new());
        assert_eq!(midi.status(), MidiStatus::default());
    }

    #[test]
    fn a_learned_control_is_saved_and_works_after_a_restart() {
        let folder = Folder::new("learn");
        let mut bench = Bench::new(&folder);
        bench.midi.learn("input/1/mute".to_owned());
        assert!(bench.midi.is_active(), "it listens to find the device");
        assert_eq!(bench.sent(), [Outgoing::Want(Wanted::Any)]);
        bench.device_says(&[0x90, 7, 127]);
        let said = bench.poll();
        assert_eq!(said, ["note 7 -> input/1/mute"]);
        assert_eq!(
            bench.sent(),
            [Outgoing::Want(Wanted::Device("APC MINI".to_owned()))],
            "from now on only that device"
        );
        let text = std::fs::read_to_string(folder.file()).expect("the file");
        assert!(text.contains("device = \"APC MINI\""), "{text}");
        assert!(text.contains("note = 7"), "{text}");
        drop(bench);

        // Another run reads it.
        let mut bench = Bench::new(&folder);
        assert!(bench.midi.is_active());
        assert_eq!(bench.midi.status().bindings[0].target, "input/1/mute");
        bench.device_says(&[0x90, 7, 127]);
        bench.poll();
        assert!(bench.desk.view().inputs[0].muted, "through the desk");
    }

    #[test]
    fn a_key_of_an_apc_key_binds_nothing_and_its_port_is_not_chosen() {
        let folder = Folder::new("keys");
        let mut bench = Bench::new(&folder);
        bench.midi.learn("input/1/mute".to_owned());
        bench.sent();
        bench
            .to_midi
            .send(Incoming::Bytes {
                port: "APC Key 25 mk2".into(),
                bytes: vec![0x90, 0x45, 90],
            })
            .expect("sent");
        assert_eq!(
            bench.poll(),
            ["that is a key; use a knob, a pad or a button"]
        );
        assert_eq!(bench.midi.status().bindings, []);
        bench
            .to_midi
            .send(Incoming::Bytes {
                port: "APC Key 25 mk2".into(),
                bytes: vec![0x90, 0x46, 90],
            })
            .expect("sent");
        assert_eq!(bench.poll(), Vec::<String>::new(), "said once a learning");
        assert_eq!(bench.midi.status().device, None);
        assert!(bench.midi.map.learning().is_some(), "it goes on waiting");

        // The same number from the port of the controls is a button.
        bench
            .to_midi
            .send(Incoming::Bytes {
                port: "MIDIIN2 (APC Key 25 mk2)".into(),
                bytes: vec![0x90, 0x45, 127],
            })
            .expect("sent");
        assert_eq!(bench.poll(), ["note 69 -> input/1/mute"]);
        assert_eq!(
            bench.midi.status().device.as_deref(),
            Some("MIDIIN2 (APC Key 25 mk2)")
        );
    }

    #[test]
    fn a_fader_runs_through_the_desk_and_only_its_newest_value_counts() {
        let folder = Folder::new("fader");
        std::fs::write(
            folder.file(),
            "device = \"APC\"\n[[binding]]\ncc = 48\ntarget = \"bus/A1/gain\"\n",
        )
        .expect("a file");
        let mut bench = Bench::new(&folder);
        // 0 dB sits at about 97: near enough to pick up, then the last of a run decides.
        for value in [96, 97, 98, 60] {
            bench.device_says(&[0xB0, 48, value]);
        }
        let said = bench.poll();
        // The batch came to its last value, which is far: the fader has yet to find the gain.
        assert!(said[0].contains("waits"), "{said:?}");
        assert_eq!(bench.desk.view().buses[0].gain_db, 0.0);

        bench.device_says(&[0xB0, 48, 97]);
        assert_eq!(
            bench.poll(),
            Vec::<String>::new(),
            "a fader's step is not worth a line"
        );
        bench.device_says(&[0xB0, 48, 60]);
        bench.poll();
        assert!(bench.desk.view().buses[0].gain_db < -6.0);
    }

    #[test]
    fn a_refusal_is_told_and_a_scene_too() {
        let folder = Folder::new("refused");
        std::fs::write(
            folder.file(),
            "device = \"APC\"\n[[binding]]\nnote = 1\ntarget = \"matrix/1/A1\"\n\
             [[binding]]\nnote = 2\ntarget = \"scene/abend\"\n",
        )
        .expect("a file");
        let mut bench = Bench::new(&folder);
        // The first input feeds only A1 in the demo.
        bench.device_says(&[0x90, 1, 127]);
        assert!(bench.poll()[0].contains("would feed no bus"));
        bench.device_says(&[0x90, 2, 127]);
        assert_eq!(bench.poll(), ["scene Abend"]);
        assert_eq!(bench.desk.view().inputs[0].gain_db, -12.0);
    }

    #[test]
    fn lights_wait_for_the_device_and_come_again_when_it_returns() {
        let folder = Folder::new("lights");
        std::fs::write(
            folder.file(),
            "device = \"APC MINI\"\n[[binding]]\nnote = 2\ntarget = \"scene/abend\"\n",
        )
        .expect("a file");
        let mut bench = Bench::new(&folder);
        let view = bench.desk.view();
        bench.midi.lights(&view);
        assert_eq!(bench.sent(), [], "nothing is there yet");

        bench
            .to_midi
            .send(Incoming::Connected("APC MINI".to_owned()))
            .expect("sent");
        assert_eq!(bench.poll(), ["MIDI device APC MINI is there"]);
        assert!(bench.midi.status().connected);
        bench.midi.lights(&view);
        assert_eq!(bench.sent(), [Outgoing::Light([0x90, 2, 5])]);
        bench.midi.lights(&view);
        assert_eq!(bench.sent(), []);

        bench
            .to_midi
            .send(Incoming::Lost("APC MINI".to_owned()))
            .expect("sent");
        assert_eq!(bench.poll(), ["MIDI device APC MINI is gone"]);
        assert!(!bench.midi.status().connected);
        bench
            .to_midi
            .send(Incoming::Connected("APC MINI".to_owned()))
            .expect("sent");
        bench.poll();
        bench.midi.lights(&view);
        assert_eq!(
            bench.sent(),
            [Outgoing::Light([0x90, 2, 5])],
            "it knows nothing of before"
        );
    }

    #[test]
    fn a_broken_file_stays_and_midi_stays_off() {
        let folder = Folder::new("broken");
        std::fs::write(folder.file(), "broken =").expect("a file");
        let mut bench = Bench::new(&folder);
        assert!(!bench.midi.is_active());
        assert!(bench.poll()[0].contains("no MIDI"));
        bench.midi.learn("input/1/mute".to_owned());
        assert!(bench.poll()[0].contains("repair"));
        assert_eq!(
            std::fs::read_to_string(folder.file()).expect("a file"),
            "broken ="
        );
    }

    #[test]
    fn forgetting_frees_a_control_and_the_last_closes_the_device() {
        let folder = Folder::new("forget");
        std::fs::write(
            folder.file(),
            "device = \"APC\"\n[[binding]]\nnote = 2\ntarget = \"input/1/mute\"\n",
        )
        .expect("a file");
        let mut bench = Bench::new(&folder);
        assert!(bench.midi.is_active());
        bench.midi.forget("input/9/mute");
        assert!(bench.poll()[0].contains("nothing is bound"));
        bench.midi.forget("input/1/mute");
        assert_eq!(bench.poll(), ["input/1/mute is free"]);
        assert!(!bench.midi.is_active(), "the port is free for others");
        assert_eq!(bench.sent(), [Outgoing::Want(Wanted::Nothing)]);
        assert!(
            !std::fs::read_to_string(folder.file())
                .expect("a file")
                .contains("note")
        );
    }
}
