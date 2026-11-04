//! The routes with data, over the wire, against a `FakeDesk` that a thread pumps as the owner of
//! a mixer would.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use appkit_core::authn::{CODE_BYTES, ManualClock, Secret, TOKEN_BYTES};
use appkit_hub::{HubStatus, Limits, RunningServer};
use appkit_testkit::wire::{Reply, request, send};
use pult_core::protocol::State;
use pult_core::view::{Desk, FakeDesk};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

struct Mixer {
    server: RunningServer,
    control: Secret,
    stop: Arc<AtomicBool>,
    owner: Option<thread::JoinHandle<()>>,
}

impl Drop for Mixer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(owner) = self.owner.take() {
            let _ = owner.join();
        }
    }
}

async fn mixer(pumped: bool) -> Mixer {
    let (link, mut pump) = pult_hub::link();
    let control = Secret::generate(TOKEN_BYTES).unwrap();
    let server = pult_hub::start(pult_hub::Start {
        addr: "127.0.0.1:0".parse().unwrap(),
        limits: Limits::default(),
        clock: Arc::new(ManualClock::new()),
        pairing_ttl: Duration::from_secs(60),
        control_secret: control.clone(),
        instance: Secret::generate(CODE_BYTES).unwrap(),
        status: HubStatus {
            engine_ready: true,
            problem: None,
            capabilities: Vec::new(),
        },
        link,
    })
    .await
    .expect("the hub starts");
    let stop = Arc::new(AtomicBool::new(false));
    let until = Arc::clone(&stop);
    let owner = thread::spawn(move || {
        let mut desk = FakeDesk::new();
        while !until.load(Ordering::Relaxed) {
            if pumped {
                let view = desk.view();
                pump.pump(&mut desk, &view);
            }
            thread::sleep(Duration::from_millis(5));
        }
    });
    Mixer {
        server,
        control,
        stop,
        owner: Some(owner),
    }
}

impl Mixer {
    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.server.addr().port())
    }

    async fn call(&self, method: &str, path: &str, bearer: Option<&str>, body: &str) -> Reply {
        let host = self.host();
        let auth = bearer.map(|b| format!("Bearer {b}"));
        let length = body.len().to_string();
        let mut headers = vec![("Host", host.as_str()), ("Content-Length", length.as_str())];
        if let Some(auth) = &auth {
            headers.push(("Authorization", auth.as_str()));
        }
        if method == "POST" {
            headers.push(("Content-Type", "application/json"));
        }
        // Whoever comes without a credential comes from the page.
        let origin = format!("http://{host}");
        if bearer.is_none() {
            headers.push(("Origin", origin.as_str()));
        }
        send(
            self.server.addr(),
            request(method, path, &headers, body.as_bytes()),
        )
        .await
    }

    async fn session(&self) -> String {
        let secret = self.control.to_hex();
        let code = self
            .call("POST", "/api/control/pair", Some(&secret), "{}")
            .await;
        assert_eq!(code.status, 200, "{code:?}");
        let body = format!(r#"{{"code":"{}"}}"#, code.json()["code"].as_str().unwrap());
        let token = self.call("POST", "/auth/exchange", None, &body).await;
        assert_eq!(token.status, 200, "{token:?}");
        token.json()["token"].as_str().unwrap().to_owned()
    }

    /// The state, once the owner has published one.
    async fn state(&self, token: &str) -> State {
        for _ in 0..100 {
            let reply = self.call("GET", "/api/pult/state", Some(token), "").await;
            if reply.status == 200 {
                return serde_json::from_str(&reply.body).expect("a state");
            }
            assert_eq!(reply.status, 503, "{reply:?}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no state came");
    }

    /// Opens a stream and returns its connection with what it has said so far.
    async fn open_stream(&self, token: &str) -> (TcpStream, String) {
        let mut connection = TcpStream::connect(self.server.addr()).await.unwrap();
        let head = format!(
            "GET /api/pult/stream HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {token}\r\n\r\n",
            self.host()
        );
        connection.write_all(head.as_bytes()).await.unwrap();
        let mut said = Vec::new();
        let mut chunk = [0_u8; 4096];
        let read = connection.read(&mut chunk).await.unwrap();
        said.extend_from_slice(&chunk[..read]);
        (connection, String::from_utf8_lossy(&said).into_owned())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_reads_the_state_and_nobody_else_does() {
    let mixer = mixer(true).await;
    let reply = mixer.call("GET", "/api/pult/state", None, "").await;
    assert_eq!(reply.status, 401, "{reply:?}");
    let token = mixer.session().await;
    let state = mixer.state(&token).await;
    assert_eq!(state.version, pult_core::protocol::VERSION);
    assert_eq!(state.inputs.len(), 3);
    assert_eq!(state.buses[0].name, "A1");
    assert_eq!(
        mixer
            .call("GET", "/api/pult/state?x=1", Some(&token), "")
            .await
            .status,
        400
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_operation_changes_the_mixer_and_a_refusal_comes_back_unchanged() {
    let mixer = mixer(true).await;
    let token = mixer.session().await;
    let done = mixer
        .call(
            "POST",
            "/api/pult/ops",
            Some(&token),
            r#"{"op":"set","address":"input/1/gain","value":-12}"#,
        )
        .await;
    assert_eq!(done.status, 200, "{done:?}");
    assert!(done.json()["lines"][0].as_str().unwrap().contains("-12.0"));
    let mut gain = 0.0;
    for _ in 0..100 {
        gain = mixer.state(&token).await.inputs[0].gain_db;
        if (gain + 12.0).abs() < f32::EPSILON {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!((gain + 12.0).abs() < f32::EPSILON, "{gain}");

    // The mixer's own rule: an input stays on a bus.
    let refused = mixer
        .call(
            "POST",
            "/api/pult/ops",
            Some(&token),
            r#"{"op":"set","address":"matrix/3/A2","value":false}"#,
        )
        .await;
    assert_eq!(refused.status, 422, "{refused:?}");
    assert_eq!(refused.json()["error"]["code"], "refused");
    assert!(
        refused.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("would feed no bus"),
        "{refused:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn only_an_operation_of_the_known_form_gets_through() {
    let mixer = mixer(true).await;
    let token = mixer.session().await;
    for body in [
        "not json",
        "{}",
        r#"{"op":"delete"}"#,
        r#"["save"]"#,
        r#"{"op":"save","path":"C:\\evil.toml"}"#,
    ] {
        let reply = mixer
            .call("POST", "/api/pult/ops", Some(&token), body)
            .await;
        assert_eq!(reply.status, 400, "{body}: {reply:?}");
        assert_eq!(reply.json()["error"]["code"], "bad_op");
        assert!(!reply.body.contains("evil"), "no echo: {reply:?}");
    }
    let unsafe_gain = mixer
        .call(
            "POST",
            "/api/pult/ops",
            Some(&token),
            r#"{"op":"set","address":"bus/A1/gain","value":90}"#,
        )
        .await;
    assert_eq!(unsafe_gain.status, 422, "{unsafe_gain:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_owner_that_does_not_look_in_makes_a_client_wait_and_no_longer() {
    let mixer = mixer(false).await;
    let token = mixer.session().await;
    let reply = mixer.call("GET", "/api/pult/state", Some(&token), "").await;
    assert_eq!(reply.status, 503, "{reply:?}");
    assert_eq!(reply.json()["error"]["code"], "not_ready");
    let reply = mixer
        .call("POST", "/api/pult/ops", Some(&token), r#"{"op":"save"}"#)
        .await;
    assert_eq!(reply.status, 504, "{reply:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stream_sends_states_and_a_session_holds_only_so_many() {
    let mixer = mixer(true).await;
    let token = mixer.session().await;
    mixer.state(&token).await;
    let (first, said) = mixer.open_stream(&token).await;
    assert!(said.starts_with("HTTP/1.1 200"), "{said}");
    assert!(said.contains("text/event-stream"), "{said}");

    let mut open = vec![first];
    let max = Limits::default().max_streams_per_session;
    while open.len() < max {
        let (connection, said) = mixer.open_stream(&token).await;
        assert!(said.starts_with("HTTP/1.1 200"), "{said}");
        open.push(connection);
    }
    let (_refused, said) = mixer.open_stream(&token).await;
    assert!(said.starts_with("HTTP/1.1 429"), "{said}");

    // A place comes back with its connection.
    drop(open.pop());
    for _ in 0..200 {
        let (connection, said) = mixer.open_stream(&token).await;
        if said.starts_with("HTTP/1.1 200") {
            open.push(connection);
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the place did not come back");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_stream_says_the_state_in_events() {
    let mixer = mixer(true).await;
    let token = mixer.session().await;
    mixer.state(&token).await;
    let (mut connection, mut said) = mixer.open_stream(&token).await;
    let mut chunk = [0_u8; 8192];
    for _ in 0..50 {
        if said.matches("event: state\n").count() >= 2 {
            break;
        }
        let read = tokio::time::timeout(Duration::from_secs(2), connection.read(&mut chunk))
            .await
            .expect("the stream goes on")
            .unwrap();
        said.push_str(&String::from_utf8_lossy(&chunk[..read]));
    }
    assert!(said.matches("event: state\n").count() >= 2, "{said}");
    assert!(said.contains(r#""version":1"#), "{said}");
}
