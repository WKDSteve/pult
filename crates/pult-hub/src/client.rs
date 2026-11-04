//! A local program's way to a running pult: it finds the hub through `hub.json`, believes it only
//! when it says the same instance (`SRV-16`), and talks through the kit's narrow client
//! (`SRV-22`: `127.0.0.1` only, no redirect, no proxy, 64 KiB, 5 s).
//!
//! A session is kept between calls, in a file beside the control secret and with the same rights:
//! the hub takes only a few pairings a minute and drops its oldest session beyond a few, so a
//! session per command would soon push out a client that stays connected. The token is worth no
//! more than the secret next to it, and goes only to the hub the instance check believed.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use appkit_core::registry;
use appkit_hub::browser;
use appkit_hub::client::{self, Reply};
use appkit_hub::env::Env;
use appkit_hub::launch::{LaunchOpts, Window, spawn};
use pult_core::protocol::{Op, State};
use serde_json::{Value, json};
use tokio::runtime::Runtime;

use crate::IDENTITY;

/// The folder of the kept session, inside the config folder.
pub(crate) const SESSION_FOLDER: &str = "cli";

/// How often to ask a hub that has no state yet, and how long to wait in between: a route that
/// only just started.
const STATE_TRIES: usize = 20;
const STATE_WAIT: Duration = Duration::from_millis(100);

/// How long a route that was asked to stop may take: its owner looks in a few times a second,
/// and its devices take a moment to close.
const STOP_TRIES: usize = 100;
const STOP_WAIT: Duration = Duration::from_millis(100);

/// Why a client got no answer it can use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// No pult answers: no `hub.json`, nothing on its port, or another program there.
    NotRunning,
    /// The mixer or the hub said no, and why.
    Refused(String),
    /// Anything else, as a text.
    Failed(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning => f.write_str(
                "no pult is running; start a route with `pult run` or in the window of pult",
            ),
            Self::Refused(why) | Self::Failed(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for ClientError {}

fn failed(what: &str, why: impl fmt::Display) -> ClientError {
    ClientError::Failed(format!("{what}: {why}"))
}

/// A connection to the pult running on this machine.
pub struct Client {
    runtime: Runtime,
    dir: PathBuf,
    port: u16,
    token: Option<String>,
}

impl Client {
    /// Finds the running pult.
    ///
    /// # Errors
    ///
    /// [`ClientError::NotRunning`] when there is none that says the instance of `hub.json`.
    pub fn find() -> Result<Self, ClientError> {
        Self::find_with(&Env::from_process())
    }

    /// [`Self::find`] with these variables instead of the process's.
    ///
    /// # Errors
    ///
    /// As [`Self::find`].
    pub fn find_with(env: &Env) -> Result<Self, ClientError> {
        let dir =
            appkit_hub::instance::config_folder(env, &IDENTITY).map_err(ClientError::Failed)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| failed("no runtime", e))?;
        let record = runtime
            .block_on(appkit_hub::instance::running_hub(&dir, &IDENTITY))
            .ok_or(ClientError::NotRunning)?;
        let token = registry::read_secret(&session_dir(&dir)).ok().flatten();
        Ok(Self {
            runtime,
            dir,
            port: record.port,
            token,
        })
    }

    /// The state of the mixer, waiting a moment for a route that only just started.
    ///
    /// # Errors
    ///
    /// A [`ClientError`] when the hub does not give one.
    pub fn state(&mut self) -> Result<State, ClientError> {
        let path = IDENTITY.route("state");
        for _ in 0..STATE_TRIES {
            let reply = self.call("GET", &path, "")?;
            if reply.status == 200 {
                return serde_json::from_str(&reply.body).map_err(|e| failed("an odd state", e));
            }
            if reply.status != 503 {
                return Err(refusal(&reply));
            }
            std::thread::sleep(STATE_WAIT);
        }
        Err(ClientError::Failed(
            "the route gave no state in time".to_owned(),
        ))
    }

    /// Asks the mixer to do `op`; returns what it did, one line each.
    ///
    /// # Errors
    ///
    /// [`ClientError::Refused`] with the mixer's reason, or another [`ClientError`].
    pub fn op(&mut self, op: &Op) -> Result<Vec<String>, ClientError> {
        let body = serde_json::to_string(op).map_err(|e| failed("an odd operation", e))?;
        let reply = self.call("POST", &IDENTITY.route("ops"), &body)?;
        if reply.status != 200 {
            return Err(refusal(&reply));
        }
        let doc: Value =
            serde_json::from_str(&reply.body).map_err(|e| failed("an odd answer", e))?;
        Ok(doc["lines"]
            .as_array()
            .map(|lines| {
                lines
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Asks the route to end, as the person at it could, and waits until it has.
    ///
    /// # Errors
    ///
    /// A [`ClientError`] when it refuses or does not end in time.
    pub fn stop(&mut self) -> Result<(), ClientError> {
        self.op(&Op::Stop {})?;
        for _ in 0..STOP_TRIES {
            let still = self
                .runtime
                .block_on(appkit_hub::instance::running_hub(&self.dir, &IDENTITY));
            if still.is_none() {
                return Ok(());
            }
            std::thread::sleep(STOP_WAIT);
        }
        Err(ClientError::Failed(
            "the route was asked to stop and still runs".to_owned(),
        ))
    }

    /// A link to the page with a new pairing code, good once and for a minute (`SRV-10`): the code
    /// is in the fragment, which goes to no server and no referrer. It needs no session.
    ///
    /// # Errors
    ///
    /// A [`ClientError`] when the hub gives no code.
    pub fn page_link(&self) -> Result<String, ClientError> {
        let code = self.pairing_code()?;
        Ok(format!("http://127.0.0.1:{}/#pair={code}", self.port))
    }

    /// Opens the page in the default browser, with a new pairing code.
    ///
    /// # Errors
    ///
    /// A [`ClientError`] when the hub gives no code or no browser starts.
    pub fn open_page(&self) -> Result<(), ClientError> {
        browser::open(&self.page_link()?, &[self.port])
            .map_err(|why| failed("cannot open the browser", why))
    }

    /// One request under the session, with a new one when the old is no longer known.
    fn call(&mut self, method: &str, path: &str, body: &str) -> Result<Reply, ClientError> {
        if self.token.is_none() {
            self.pair()?;
        }
        let reply = self.send(method, path, body)?;
        if reply.status != 401 {
            return Ok(reply);
        }
        self.pair()?;
        self.send(method, path, body)
    }

    fn send(&self, method: &str, path: &str, body: &str) -> Result<Reply, ClientError> {
        let bearer = format!("Bearer {}", self.token.as_deref().unwrap_or_default());
        let headers = [
            ("Authorization", bearer.as_str()),
            ("Content-Type", "application/json"),
        ];
        self.runtime
            .block_on(client::request(self.port, method, path, &headers, body))
            .map_err(|e| failed("the hub did not answer", e))
    }

    /// Gets a session with the control secret, and keeps it for the next call.
    fn pair(&mut self) -> Result<(), ClientError> {
        let code = self.pairing_code()?;
        let origin = format!("http://127.0.0.1:{}", self.port);
        let headers = [
            ("Origin", origin.as_str()),
            ("Content-Type", "application/json"),
        ];
        let body = json!({ "code": code }).to_string();
        let reply = self
            .runtime
            .block_on(client::request(
                self.port,
                "POST",
                "/auth/exchange",
                &headers,
                &body,
            ))
            .map_err(|e| failed("the hub did not answer", e))?;
        if reply.status != 200 {
            return Err(refusal(&reply));
        }
        let token = field(&reply, "token")?;
        // Without the file the next call only pairs again.
        let _ = registry::write_secret(&session_dir(&self.dir), &token);
        self.token = Some(token);
        Ok(())
    }

    /// A pairing code, against the control secret.
    fn pairing_code(&self) -> Result<String, ClientError> {
        let secret = registry::read_secret(&self.dir)
            .map_err(|e| failed("cannot read the control secret", e))?
            .ok_or(ClientError::NotRunning)?;
        let bearer = format!("Bearer {secret}");
        let headers = [
            ("Authorization", bearer.as_str()),
            ("Content-Type", "application/json"),
        ];
        let reply = self
            .runtime
            .block_on(client::request(
                self.port,
                "POST",
                "/api/control/pair",
                &headers,
                "{}",
            ))
            .map_err(|e| failed("the hub did not answer", e))?;
        if reply.status != 200 {
            return Err(refusal(&reply));
        }
        field(&reply, "code")
    }
}

/// The config folder of pult on this machine.
///
/// # Errors
///
/// A text when there is none.
pub fn config_folder() -> Result<PathBuf, String> {
    appkit_hub::instance::config_folder(&Env::from_process(), &IDENTITY)
}

/// Starts the window of pult, the program `pult-app` beside `program`, on its own: nothing of this
/// process goes along. It opens the profile it had last, and its hub comes up with the route.
///
/// # Errors
///
/// A text when there is no such program beside it, or it does not start.
pub fn start_window(program: &Path) -> Result<(), String> {
    let folder = program.parent().ok_or("the program has no folder")?;
    let window = folder.join(format!("pult-app{}", std::env::consts::EXE_SUFFIX));
    if !window.is_file() {
        return Err(format!(
            "no window beside this program: {}",
            window.display()
        ));
    }
    spawn(
        &window,
        &[],
        &LaunchOpts {
            cwd: folder,
            window: Window::Default,
        },
    )
    .map(|_| ())
}

/// Where the kept session lies.
pub(crate) fn session_dir(dir: &Path) -> PathBuf {
    dir.join(SESSION_FOLDER)
}

/// A text field of a reply.
fn field(reply: &Reply, name: &str) -> Result<String, ClientError> {
    let doc: Value = serde_json::from_str(&reply.body).map_err(|e| failed("an odd answer", e))?;
    doc[name]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| ClientError::Failed(format!("the answer has no {name}")))
}

/// What a reply that is not a success says.
fn refusal(reply: &Reply) -> ClientError {
    let said = serde_json::from_str::<Value>(&reply.body)
        .ok()
        .and_then(|doc| doc["error"]["message"].as_str().map(str::to_owned));
    let why = said.unwrap_or_else(|| format!("the hub answered {}", reply.status));
    match reply.status {
        400 | 422 => ClientError::Refused(why),
        _ => ClientError::Failed(why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Control;
    use appkit_core::registry::HubRecord;
    use pult_core::view::{Desk, FakeDesk};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A folder for one test, removed when it ends.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("pult-client-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("a folder");
            Self(dir)
        }

        fn env(&self) -> Env {
            Env::from_pairs([
                (IDENTITY.var("CONFIG_DIR"), self.0.clone().into_os_string()),
                (IDENTITY.var("PORT"), "0".into()),
            ])
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A hub on the demo desk, pumped by a thread as the owner of a route would.
    struct Running {
        control: Option<Control>,
        stop: Arc<AtomicBool>,
        owner: Option<std::thread::JoinHandle<()>>,
    }

    impl Running {
        fn start(folder: &Folder) -> Self {
            let (control, mut pump) = Control::start_with(folder.env()).expect("a hub");
            let stop = Arc::new(AtomicBool::new(false));
            let stopping = Arc::clone(&stop);
            let owner = std::thread::spawn(move || {
                let mut desk = FakeDesk::new();
                while !stopping.load(Ordering::Relaxed) {
                    let view = desk.view();
                    pump.pump(&mut desk, &view);
                    std::thread::sleep(Duration::from_millis(5));
                }
            });
            Self {
                control: Some(control),
                stop,
                owner: Some(owner),
            }
        }
    }

    impl Drop for Running {
        fn drop(&mut self) {
            self.control.take();
            self.stop.store(true, Ordering::Relaxed);
            if let Some(owner) = self.owner.take() {
                let _ = owner.join();
            }
        }
    }

    #[test]
    fn nothing_there_is_no_pult_and_a_stale_record_neither() {
        let folder = Folder::new("nothing");
        assert_eq!(
            Client::find_with(&folder.env()).err(),
            Some(ClientError::NotRunning)
        );

        // A port nobody listens on any more.
        let free = TcpListener::bind(("127.0.0.1", 0)).expect("a port");
        let port = free.local_addr().expect("its address").port();
        drop(free);
        registry::write_record(
            &folder.0,
            &HubRecord {
                app: "pult".to_owned(),
                port,
                pid: 1,
                version: "0".to_owned(),
                instance: "gone".to_owned(),
            },
        )
        .expect("a record");
        assert_eq!(
            Client::find_with(&folder.env()).err(),
            Some(ClientError::NotRunning)
        );
    }

    #[test]
    fn the_secret_goes_to_no_program_with_another_instance() {
        let folder = Folder::new("foreign");
        registry::write_secret(&folder.0, "the-secret").expect("a secret");
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("a port");
        let port = listener.local_addr().expect("its address").port();
        registry::write_record(
            &folder.0,
            &HubRecord {
                app: "pult".to_owned(),
                port,
                pid: 1,
                version: "0".to_owned(),
                instance: "the-right-one".to_owned(),
            },
        )
        .expect("a record");
        // Something on that port calls itself pult, but with another instance.
        let heard = std::thread::spawn(move || {
            let mut said = String::new();
            listener.set_nonblocking(true).expect("not blocking");
            let until = std::time::Instant::now() + Duration::from_secs(2);
            while std::time::Instant::now() < until {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                };
                stream.set_nonblocking(false).expect("blocking");
                let mut buffer = [0; 4096];
                let n = stream.read(&mut buffer).expect("a request");
                said.push_str(&String::from_utf8_lossy(&buffer[..n]));
                let body = r#"{"app":"pult","instance":"another"}"#;
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .expect("an answer");
            }
            said
        });
        assert_eq!(
            Client::find_with(&folder.env()).err(),
            Some(ClientError::NotRunning)
        );
        let said = heard.join().expect("what it heard");
        assert!(said.contains("hello"), "{said}");
        assert!(!said.contains("the-secret"), "{said}");
        assert!(!said.contains("Authorization"), "{said}");
    }

    #[test]
    fn it_reads_acts_and_is_told_no() {
        let folder = Folder::new("acts");
        let _running = Running::start(&folder);
        let mut client = Client::find_with(&folder.env()).expect("a pult");
        assert_eq!(client.state().expect("a state").scenes, ["Abend", "Laut"]);
        assert_eq!(
            client
                .op(&Op::Scene {
                    name: "abend".to_owned()
                })
                .expect("done"),
            ["scene abend"]
        );
        match client.op(&Op::Scene {
            name: "nichts".to_owned(),
        }) {
            Err(ClientError::Refused(why)) => assert!(why.contains("no scene"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_page_link_carries_a_code_only_in_its_fragment() {
        let folder = Folder::new("link");
        let _running = Running::start(&folder);
        let client = Client::find_with(&folder.env()).expect("a pult");
        let link = client.page_link().expect("a link");
        let (place, code) = link.split_once("/#pair=").expect("the fragment");
        assert_eq!(place, format!("http://127.0.0.1:{}", client.port));
        assert!(
            code.len() >= 32 && code.bytes().all(|b| b.is_ascii_hexdigit()),
            "{code}"
        );
        assert!(browser::is_allowed(&link, &[client.port]), "{link}");
    }

    #[test]
    fn no_window_beside_the_program_is_said() {
        let folder = Folder::new("window");
        let why = start_window(&folder.0.join("pult.exe")).expect_err("nothing there");
        assert!(why.contains("no window beside"), "{why}");
    }

    #[test]
    fn a_session_is_kept_and_a_lost_one_replaced() {
        let folder = Folder::new("session");
        let running = Running::start(&folder);
        // More calls than the hub takes pairings in a minute, each as a program of its own.
        let mut token = None;
        for _ in 0..15 {
            let mut client = Client::find_with(&folder.env()).expect("a pult");
            client.state().expect("a state");
            let kept = client.token.clone().expect("a session");
            assert!(token.is_none() || token.as_ref() == Some(&kept));
            token = Some(kept);
        }

        // A session the hub no longer knows, as after another run: a new one, without a word.
        registry::write_secret(&session_dir(&folder.0), "stale").expect("a file");
        let mut client = Client::find_with(&folder.env()).expect("a pult");
        client.state().expect("a state");
        assert_ne!(client.token.as_deref(), Some("stale"));
        assert_ne!(client.token, token, "a new session");
        drop(running);
        assert!(
            !session_dir(&folder.0).exists(),
            "the end of the run takes the session along"
        );
    }
}
