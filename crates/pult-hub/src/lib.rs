//! The local control interface of pult: a hub on `appkit-hub`, loopback only, every request
//! through the kit's guard before a route sees it.
//!
//! pult keeps no vault and has no engine process: what the kit calls the runtime is only the
//! status `hello` reports. The audio crates know nothing of this one, so axum and tokio stay
//! away from the callbacks.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use appkit_core::authn::{Clock, Secret};
use appkit_core::identity::AppIdentity;
use appkit_hub::{
    Core, GuardRules, HubStatus, Limits, Page, Pages, RunningServer, Runtime, ServeParts, serve,
};
use axum::Router;

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
pub const DATA_ROUTES: [&str; 2] = ["state", "ops"];

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
}

/// What the kit keeps for the vault that is read: pult has neither, only the status.
struct Status(HubStatus);

impl Runtime for Status {
    type Target = ();

    fn status(&self) -> &HubStatus {
        &self.0
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

fn routes(_core: &Arc<Core<Status>>) -> Router<Arc<Core<Status>>> {
    Router::new()
}

/// Starts the hub.
///
/// # Errors
///
/// When the address is not loopback or cannot be bound.
pub async fn start(start: Start) -> io::Result<RunningServer> {
    let idle = HubStatus {
        engine_ready: false,
        problem: None,
        capabilities: Vec::new(),
    };
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
            runtime: Status(start.status),
            build: Arc::new(move |()| Status(idle.clone())),
            vaults: None,
        },
        routes,
    )
    .await
}
