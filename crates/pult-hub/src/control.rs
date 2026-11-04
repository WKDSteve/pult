//! The hub as a part of a running program: a thread of its own with its server, and the files
//! that let local programs find it (`hub.json`, `control.secret`). Nothing here runs in the
//! program's way: when the hub cannot start, the route plays anyway.

use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use appkit_core::authn::{CODE_BYTES, Secret, SystemClock, TOKEN_BYTES};
use appkit_core::registry::{self, HubRecord};
use appkit_hub::env::Env;
use appkit_hub::{HubStatus, Limits, RunningServer};
use tokio::sync::oneshot;

use crate::{IDENTITY, Pump, Start, link, start};

/// How long a pairing code holds (`SRV-10`).
const PAIRING_TTL: Duration = Duration::from_secs(60);

/// The running hub; dropping it stops the server and takes its files away.
pub struct Control {
    stop: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
    port: u16,
}

impl Control {
    /// Starts the hub on its port, or on the one in `PULT_PORT`.
    ///
    /// # Errors
    ///
    /// A text for the person at the terminal: another pult has the hub, or it cannot start.
    pub fn start() -> Result<(Self, Pump), String> {
        Self::start_with(Env::from_process())
    }

    /// The port the hub answers on.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// [`Self::start`] with these variables instead of the process's.
    ///
    /// # Errors
    ///
    /// As [`Self::start`].
    pub fn start_with(env: Env) -> Result<(Self, Pump), String> {
        let (started_tx, started_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = oneshot::channel();
        let (hub_link, pump) = link();
        let thread = thread::Builder::new()
            .name("pult-hub".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                let runtime = match runtime {
                    Ok(runtime) => runtime,
                    Err(e) => {
                        let _ = started_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                runtime.block_on(async move {
                    match serve(&env, hub_link).await {
                        Ok((running, dir)) => {
                            let _ = started_tx.send(Ok(running.addr().port()));
                            // A sender that is gone stops the hub as well.
                            let _ = stop_rx.await;
                            registry::remove_record(&dir);
                            registry::remove_secret(&dir);
                            // The session of the command line dies with the hub.
                            let session = crate::client::session_dir(&dir);
                            registry::remove_secret(&session);
                            let _ = std::fs::remove_dir(&session);
                            running.shutdown().await;
                        }
                        Err(why) => {
                            let _ = started_tx.send(Err(why));
                        }
                    }
                });
            })
            .map_err(|e| e.to_string())?;
        let port = match started_rx.recv() {
            Ok(Ok(port)) => port,
            Ok(Err(why)) => {
                let _ = thread.join();
                return Err(why);
            }
            Err(_) => {
                let _ = thread.join();
                return Err("the thread of the hub ended".to_owned());
            }
        };
        Ok((
            Self {
                stop: Some(stop_tx),
                thread: Some(thread),
                port,
            },
            pump,
        ))
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The port to try first: `PULT_PORT`, then the default. A value that is no port is an error:
/// a typo must not put the hub somewhere else without anyone seeing it.
fn wanted_port(env: &Env) -> Result<u16, String> {
    let name = IDENTITY.var("PORT");
    match env.get(&name) {
        Some(value) => value
            .to_str()
            .and_then(|text| text.parse().ok())
            .ok_or_else(|| format!("{name} is not a port")),
        None => Ok(IDENTITY.default_port),
    }
}

/// Starts the server and writes its files. Another hub of pult keeps what it has; a port that
/// something else holds gives way to a free one (`SRV-24`).
async fn serve(
    env: &Env,
    hub_link: Arc<crate::Link>,
) -> Result<(RunningServer, std::path::PathBuf), String> {
    let dir = appkit_hub::instance::config_folder(env, &IDENTITY)?;
    let want = wanted_port(env)?;
    if let Some(other) = appkit_hub::instance::running_hub(&dir, &IDENTITY).await {
        return Err(format!(
            "another pult has it (port {}, process {})",
            other.port, other.pid
        ));
    }
    let control = Secret::generate(TOKEN_BYTES).map_err(|e| e.to_string())?;
    let instance = Secret::generate(CODE_BYTES).map_err(|e| e.to_string())?;
    let make = |port: u16| Start {
        addr: std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        limits: Limits::default(),
        clock: Arc::new(SystemClock::new()),
        pairing_ttl: PAIRING_TTL,
        control_secret: control.clone(),
        instance: instance.clone(),
        status: HubStatus {
            engine_ready: true,
            problem: None,
            capabilities: Vec::new(),
        },
        link: Arc::clone(&hub_link),
    };
    let running = match start(make(want)).await {
        Ok(running) => running,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if appkit_hub::instance::hello_of(want, &IDENTITY)
                .await
                .is_some()
            {
                return Err(format!("another pult has it (port {want})"));
            }
            start(make(0)).await.map_err(|e| e.to_string())?
        }
        Err(e) => return Err(e.to_string()),
    };
    let record = HubRecord {
        app: IDENTITY.app.to_owned(),
        port: running.addr().port(),
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        instance: running.instance().to_owned(),
    };
    let written = registry::write_secret(&dir, &control.to_hex())
        .and_then(|()| registry::write_record(&dir, &record));
    if let Err(e) = written {
        registry::remove_secret(&dir);
        registry::remove_record(&dir);
        running.shutdown().await;
        return Err(e.to_string());
    }
    Ok((running, dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    /// A folder for one test, removed when it ends.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("pult-control-test-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("a folder");
            Self(dir)
        }

        fn env(&self, port: &str) -> Env {
            Env::from_pairs([
                (IDENTITY.var("CONFIG_DIR"), self.0.clone().into_os_string()),
                (IDENTITY.var("PORT"), port.into()),
            ])
        }

        fn has(&self, file: &str) -> bool {
            self.0.join(file).exists()
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn get(port: u16, path: &str) -> String {
        let mut connection = TcpStream::connect(("127.0.0.1", port)).expect("the hub answers");
        write!(
            connection,
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        )
        .expect("a request");
        let mut reply = String::new();
        connection.read_to_string(&mut reply).expect("a reply");
        reply
    }

    fn record_in(dir: &Path) -> HubRecord {
        registry::read_record(dir)
            .expect("a readable record")
            .expect("a record")
    }

    #[test]
    fn it_answers_and_leaves_nothing_behind() {
        let folder = Folder::new("files");
        let (control, _pump) = Control::start_with(folder.env("0")).expect("the hub starts");
        let port = control.port();
        assert!(get(port, "/api/pult/hello").contains(r#""app":"pult""#));
        assert_eq!(record_in(&folder.0).port, port);
        assert!(
            registry::read_secret(&folder.0)
                .expect("readable")
                .is_some()
        );
        drop(control);
        assert!(!folder.has("hub.json"));
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    }

    #[test]
    fn a_second_one_leaves_the_first_alone() {
        let folder = Folder::new("second");
        let (first, _pump) = Control::start_with(folder.env("0")).expect("the hub starts");
        let why = Control::start_with(folder.env("0"))
            .err()
            .expect("the second is refused");
        assert!(why.contains("another pult has it"), "{why}");
        assert_eq!(record_in(&folder.0).port, first.port(), "the file stays");
        assert!(get(first.port(), "/api/pult/hello").contains(r#""app":"pult""#));
    }

    #[test]
    fn another_program_on_the_port_makes_it_take_a_free_one() {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        let port = taken.local_addr().expect("its address").port();
        let folder = Folder::new("taken");
        // Something that holds the port but never answers like a hub.
        let answer = thread::spawn(move || {
            if let Ok((connection, _)) = taken.accept() {
                drop(connection);
            }
            taken
        });
        let (control, _pump) =
            Control::start_with(folder.env(&port.to_string())).expect("the hub starts");
        assert_ne!(control.port(), port);
        drop(control);
        drop(answer.join().expect("the other program"));
    }

    #[test]
    fn a_stale_file_does_not_keep_it_from_starting() {
        let folder = Folder::new("stale");
        let (first, _pump) = Control::start_with(folder.env("0")).expect("the hub starts");
        let stale = record_in(&folder.0);
        drop(first);
        registry::write_record(&folder.0, &stale).expect("a stale file");
        let (control, _pump) = Control::start_with(folder.env("0")).expect("it starts anyway");
        assert_ne!(record_in(&folder.0).instance, stale.instance);
        drop(control);
    }

    #[test]
    fn a_bad_port_is_said() {
        let folder = Folder::new("bad");
        let why = Control::start_with(folder.env("seventeen"))
            .err()
            .expect("refused");
        assert!(why.contains("PULT_PORT"), "{why}");
    }

    #[test]
    fn an_open_connection_does_not_hold_up_the_end() {
        let folder = Folder::new("end");
        let (control, _pump) = Control::start_with(folder.env("0")).expect("the hub starts");
        let port = control.port();
        let _idle = TcpStream::connect(("127.0.0.1", port)).expect("a connection");
        let begun = Instant::now();
        drop(control);
        assert!(
            begun.elapsed() < Duration::from_secs(5),
            "{:?}",
            begun.elapsed()
        );
    }
}
