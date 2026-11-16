//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! The started servers: the local one always, the remote one when configured.

use std::net::SocketAddr;
use std::sync::Arc;

use norn_ipc::Endpoint;
use tpp_grpc::{HybridServer, ListenAddr};

use super::DaemonError;

/// What started: the servers, the local endpoint actually bound, and the remote
/// address actually bound, if any.
pub(super) struct Started {
    pub(super) bound: Bound,
    pub(super) endpoint: Endpoint,
    pub(super) remote: Option<SocketAddr>,
}

/// The started servers. Dropped unstopped, they stop themselves on a task, so a
/// bound daemon dropped without running releases its endpoints. Correct from any
/// task.
pub(super) struct Bound(Vec<Arc<HybridServer>>);

impl Bound {
    /// Starts `local`, then `remote`. If the remote server will not start, the
    /// local one is stopped again, so a failed bind leaves nothing listening.
    pub(super) async fn start(
        mut local: HybridServer,
        wanted: &Endpoint,
        remote: Option<HybridServer>,
    ) -> Result<Started, DaemonError> {
        local.start().await.map_err(DaemonError::Server)?;
        let endpoint = match local.grpc_listen_addr() {
            Some(ListenAddr::Tcp(address)) => Endpoint::Tcp(address),
            _ => wanted.clone(),
        };
        let mut bound = Self(vec![Arc::new(local)]);
        let mut remote_addr = None;
        if let Some(mut remote) = remote {
            if let Err(e) = remote.start().await {
                let _ = bound.stop().await;
                return Err(DaemonError::Server(e));
            }
            if let Some(ListenAddr::Tcp(address)) = remote.grpc_listen_addr() {
                remote_addr = Some(address);
            }
            tracing::info!(address = ?remote_addr, "serving gRPC to other machines over TLS");
            bound.0.push(Arc::new(remote));
        }
        Ok(Started {
            bound,
            endpoint,
            remote: remote_addr,
        })
    }

    /// Drains and stops every server, together, and reports the first failure.
    pub(super) async fn stop(mut self) -> Result<(), tpp_grpc::Error> {
        let servers = std::mem::take(&mut self.0);
        let stops = servers.into_iter().map(|s| async move { s.stop().await });
        let mut result = Ok(());
        for outcome in futures_join(stops).await {
            if result.is_ok() {
                result = outcome;
            }
        }
        result
    }
}

/// Runs `futures` concurrently, so one server's drain does not wait on the other's.
async fn futures_join<F, T>(futures: impl Iterator<Item = F>) -> Vec<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let handles: Vec<_> = futures.map(tokio::spawn).collect();
    let mut out = Vec::with_capacity(handles.len());
    for handle in handles {
        match handle.await {
            Ok(value) => out.push(value),
            Err(e) => tracing::error!(error = %e, "a server's stop panicked"),
        }
    }
    out
}

impl Drop for Bound {
    fn drop(&mut self) {
        let servers = std::mem::take(&mut self.0);
        if let (false, Ok(handle)) = (servers.is_empty(), tokio::runtime::Handle::try_current()) {
            for server in servers {
                handle.spawn(async move { server.stop().await });
            }
        }
    }
}
