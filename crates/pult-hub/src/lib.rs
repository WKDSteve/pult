//! The local control interface of pult: a hub on `appkit-hub`, loopback only, every request
//! through the kit's guard before a route sees it.
//!
//! pult keeps no vault and has no engine process: what the kit calls the runtime is only the
//! status `hello` reports. The audio crates know nothing of this one, so axum and tokio stay
//! away from the callbacks.
//!
//! The hub never touches the mixer itself; see [`link`]. `docs/CONTROL.md` describes the routes.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use appkit_core::authn::{Clock, Secret};
use appkit_core::identity::AppIdentity;
use appkit_hub::{
    GuardRules, HubStatus, Limits, Page, Pages, RunningServer, Runtime, ServeParts, serve,
};

mod control;
mod link;
mod routes;

pub use control::Control;
pub use link::{Link, Pump, link};

/// The names of pult as a hub. The port is listed in `appkit-testkit::ports`.
pub const IDENTITY: AppIdentity = AppIdentity {
    app: "pult",
    binary: "pult",
    env_prefix: "PULT",
    title: "pult",
    default_port: 17619,
    route_ns: "pult",
};

/// The routes that need a session, below the namespace of the app.
pub const DATA_ROUTES: [&str; 3] = ["state", "stream", "ops"];

/// How to start the hub.
pub struct Start {
    /// Must be a loopback address; port 0 picks a free port.
    pub addr: SocketAddr,
    /// The limits of the guard.
    pub limits: Limits,
    /// The clock of the pairing and of the limits.
    pub clock: Arc<dyn Clock>,
    /// How long a pairing code holds.
    pub pairing_ttl: Duration,
    /// What lets a local program ask for a pairing code.
    pub control_secret: Secret,
    /// Changes with every start.
    pub instance: Secret,
    /// What `hello` says.
    pub status: HubStatus,
    /// The way to the owner of the mixer.
    pub link: Arc<Link>,
}

/// What the kit keeps for the vault that is read: pult has none, only the status and the way
/// to the mixer.
pub(crate) struct Status {
    hello: HubStatus,
    pub(crate) link: Arc<Link>,
    pub(crate) streams: routes::Streams,
}

impl Runtime for Status {
    type Target = ();

    fn status(&self) -> &HubStatus {
        &self.hello
    }

    fn stop(&self) {}
}

/// No page yet: the placeholder of the kit is served.
struct NoPages;

impl Pages for NoPages {
    fn page(&self, _request_path: &str) -> Option<Page> {
        None
    }

    fn has_index(&self) -> bool {
        false
    }
}

/// Starts the hub.
///
/// # Errors
///
/// When the address is not loopback or cannot be bound.
pub async fn start(start: Start) -> io::Result<RunningServer> {
    let link = start.link;
    let again = start.status.clone();
    let same = Arc::clone(&link);
    serve(
        ServeParts {
            addr: start.addr,
            limits: start.limits,
            clock: start.clock,
            pairing_ttl: start.pairing_ttl,
            control_secret: start.control_secret,
            instance: start.instance,
            app: IDENTITY.app,
            version: env!("CARGO_PKG_VERSION"),
            build_id: None,
            guard_rules: GuardRules::new(IDENTITY.route("hello"), [IDENTITY.route("ops")]),
            ui: Arc::new(NoPages),
            runtime: Status {
                hello: start.status,
                link,
                streams: routes::Streams::default(),
            },
            // pult has no vault to choose, so the kit never calls this.
            build: Arc::new(move |()| Status {
                hello: again.clone(),
                link: Arc::clone(&same),
                streams: routes::Streams::default(),
            }),
            vaults: None,
        },
        routes::routes,
    )
    .await
}
