//! The security matrix of appkit (guard, pairing, limits), run for the hub of pult. It came
//! before the first route with data (`SRV-23`), so that no later route can be open by accident.

use appkit_core::identity::AppIdentity;
use appkit_hub::{BoxFuture, RunningServer};
use appkit_testkit::subject::{StartConfig, Subject};

/// The hub of pult, as the matrix starts it.
pub struct Pult;

impl Subject for Pult {
    fn identity(&self) -> &'static AppIdentity {
        &pult_hub::IDENTITY
    }

    fn start(&self, cfg: StartConfig) -> BoxFuture<'_, std::io::Result<RunningServer>> {
        Box::pin(pult_hub::start(pult_hub::Start {
            addr: cfg.addr,
            limits: cfg.limits,
            clock: cfg.clock,
            pairing_ttl: cfg.pairing_ttl,
            control_secret: cfg.control_secret,
            instance: cfg.instance,
            status: cfg.status,
        }))
    }

    fn data_routes(&self) -> Vec<&'static str> {
        pult_hub::DATA_ROUTES.to_vec()
    }
}

mod pult {
    appkit_testkit::security_matrix!(crate::Pult);
}
