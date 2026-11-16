//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! The daemon: a runtime whose tasks serve gRPC over local IPC.

mod bound;
mod claim;
mod error;
pub(crate) mod open;
mod reflection;
#[cfg(feature = "remote")]
mod remote;
mod router;
mod run;
mod serve;
mod tasks;

pub use error::DaemonError;

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use norn_ipc::Endpoint;
use norn_runtime::ingest::{Ingest, IngestRecovery};
use norn_runtime::maintenance::{CompactionPhase, Maintenance};
use norn_runtime::sessions::Replays;
use norn_runtime::{HealthWatch, Reservation, Runtime};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tonic_health::server::HealthReporter;

use self::claim::claim_endpoints;
use self::open::open_database;
use self::router::build_router;
use crate::config::DaemonConfig;

/// Slack the runtime gives the IPC task beyond its own drain bound, so the
/// server finishes aborting connections before the runtime aborts it.
const SHUTDOWN_SLACK: Duration = Duration::from_secs(5);

/// The Norn daemon, bound to its endpoint and ready to run.
///
/// Lifecycle: [`bind`](Daemon::bind) checks the endpoint (so a second daemon
/// fails there, before touching a database), opens the database and runs crash
/// recovery, builds every service and opens the listener, whose calls wait until
/// [`run`](Daemon::run) starts. So a client's first call
/// sees the recovered state. Dropping a bound daemon stops the listener, on a
/// task of the current runtime, and releases the database. `run` serves until
/// cancelled, then drains in-flight calls (bounded by the configured drain
/// timeout), removes the socket, and returns once every task has stopped.
///
/// Concurrency: `Send`; it is consumed by `run`. Take a
/// [`health`](Daemon::health) watch first to observe it while it runs.
pub struct Daemon {
    runtime: Runtime,
    server: bound::Bound,
    endpoint: Endpoint,
    /// The remote listener's address, when `[remote]` is configured.
    remote: Option<SocketAddr>,
    /// Whether calls are answered yet.
    opening: watch::Sender<bool>,
    /// Cancelled when MCP's stdio client leaves.
    ended: CancellationToken,
    reporter: HealthReporter,
    ingest: Arc<Ingest>,
    recovery: IngestRecovery,
    replays: Arc<Replays>,
    maintenance: Arc<Maintenance>,
    /// The tasks calls spawn, waited for before ingestion closes.
    calls: TaskTracker,
    /// The half of `ipc_bytes` promised to flow-control windows.
    _windows: Reservation,
}

impl fmt::Debug for Daemon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Daemon")
            .field("endpoint", &self.endpoint)
            .field("health", &self.runtime.health().get())
            .finish_non_exhaustive()
    }
}

impl Daemon {
    /// Validates `config`, binds its endpoint and builds the services.
    ///
    /// # Errors
    ///
    /// [`DaemonError::Runtime`] if the engine configuration cannot be run
    /// on this platform; [`DaemonError::Ipc`] if the endpoint cannot be
    /// claimed (a live daemon already holds it, an insecure directory...);
    /// [`DaemonError::Metadata`] if the registry cannot be opened, as when
    /// another daemon holds the database; [`DaemonError::Storage`] if recovery
    /// cannot assess a stream.
    pub async fn bind(config: DaemonConfig) -> Result<Self, DaemonError> {
        let drain = config.drain();
        let runtime =
            Runtime::new(*config.engine())?.shutdown_timeout(drain.saturating_add(SHUTDOWN_SLACK));
        claim_endpoints(&config).await?;
        let database = open_database(&config, &runtime).await?;
        let (ingest, recovery) = (Arc::clone(&database.ingest), database.recovery.clone());
        let (opening, open) = watch::channel(false);
        let ended = CancellationToken::new();
        let wiring = serve::Wiring {
            open,
            ended: ended.clone(),
        };
        let built = build_router(&config, &runtime, database, wiring).await?;
        let started = bound::Bound::start(built.server, &config.endpoint(), built.remote).await?;
        Ok(Self {
            runtime,
            server: started.bound,
            endpoint: started.endpoint,
            remote: started.remote,
            opening,
            ended,
            reporter: built.reporter,
            ingest,
            recovery,
            replays: built.replays,
            maintenance: built.maintenance,
            calls: built.calls,
            _windows: built.windows,
        })
    }

    /// Calls `probe` at each phase of every compaction, on the worker's thread. For
    /// tests that must stop a real process at an exact point; a `norn` never sets
    /// one. Before [`run`](Daemon::run).
    pub fn compaction_probe(&self, probe: Box<dyn Fn(CompactionPhase) + Send + Sync>) {
        self.maintenance.set_probe(probe);
    }

    /// What startup recovery found. Also published as `Server.recovery`.
    #[must_use]
    pub fn recovery(&self) -> &IngestRecovery {
        &self.recovery
    }

    /// The endpoint actually bound (with any ephemeral port resolved).
    #[must_use]
    pub fn local_endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// The remote listener's address (with any ephemeral port resolved), when
    /// `[remote]` is configured.
    #[must_use]
    pub fn remote_addr(&self) -> Option<SocketAddr> {
        self.remote
    }

    /// A watch on the daemon's lifecycle; valid after `run` consumes it.
    #[must_use]
    pub fn health(&self) -> HealthWatch {
        self.runtime.health()
    }
}
