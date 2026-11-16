//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! The remote listener (`[remote]`): a second runtime-rs server over the same
//! services, for other machines. TLS from the configured certificate, and the
//! token gate in front of everything else, so an unauthenticated call is refused
//! before it is decoded or charged to any budget. No MCP, no reflection and no
//! gateway: what other machines reach is the gRPC API and its health, nothing more.

use std::path::Path;

use tokio::sync::watch;
use tonic_health::pb::health_server::Health;
use tpp_grpc::options::{Options, ServerEnvironment};
use tpp_grpc::{BannerOutput, HybridServer};

use super::DaemonError;
use super::serve::{Registered, limits};
use crate::config::{DaemonConfig, RemoteConfig};
use crate::metered::MeterLayer;
use crate::opening::OpeningLayer;
use crate::token_gate::TokenGateLayer;
use crate::version_gate::VersionGateLayer;

/// The name the remote server reports. Its hyphen keeps environment overrides
/// out, as the local server's does.
const NAME: &str = "norn-remote";

/// The shortest token accepted: 32 bytes, as `openssl rand -hex 16` makes.
const MIN_TOKEN_BYTES: usize = 32;

/// The largest token file read.
const MAX_TOKEN_BYTES: u64 = 4096;

/// Reads the token from `path`: owner-only on Unix, at least [`MIN_TOKEN_BYTES`]
/// once surrounding whitespace is trimmed, and printable ASCII, since it travels
/// in a header.
///
/// # Errors
/// [`DaemonError::RemoteToken`], naming the file and what is wrong with it.
pub(super) fn token(path: &Path) -> Result<Vec<u8>, DaemonError> {
    let fail = |why: String| DaemonError::RemoteToken(format!("{}: {why}", path.display()));
    let meta = std::fs::metadata(path).map_err(|e| fail(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(fail(format!("mode {mode:o}; it must be owner-only (0600)")));
        }
    }
    if meta.len() > MAX_TOKEN_BYTES {
        return Err(fail(format!("larger than {MAX_TOKEN_BYTES} bytes")));
    }
    let bytes = std::fs::read(path).map_err(|e| fail(e.to_string()))?;
    let token = bytes.trim_ascii().to_vec();
    if token.len() < MIN_TOKEN_BYTES {
        return Err(fail(format!("shorter than {MIN_TOKEN_BYTES} bytes")));
    }
    if !token.iter().all(|b| b.is_ascii_graphic()) {
        return Err(fail(
            "only printable ASCII without spaces may be a token".into(),
        ));
    }
    Ok(token)
}

/// The remote server, configured and not yet started.
pub(super) fn build<H: Health>(
    config: &DaemonConfig,
    remote: &RemoteConfig,
    token: Vec<u8>,
    open: watch::Receiver<bool>,
    s: Registered<H>,
    meter: MeterLayer,
    window: u32,
) -> HybridServer {
    let drain = config.drain();
    let options = Options {
        service_name: NAME.into(),
        description: "Norn, a temporal data engine, for other machines".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        grpc: tpp_grpc::options::Endpoint::new(
            remote.listen.ip().to_string(),
            remote.listen.port(),
        ),
        enable_mcp: false,
        environment: ServerEnvironment::Production,
        ..Options::default()
    };
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let server = HybridServer::new(options)
        .with_banner(BannerOutput::Off)
        .with_certificates(path(&remote.cert_file), path(&remote.key_file))
        .with_grpc_limits(limits(remote.max_clients, drain, window))
        .with_shutdown_timeout(drain.saturating_add(super::serve::STOP_SLACK))
        .with_grpc_layer(TokenGateLayer::new(token))
        .with_grpc_layer(OpeningLayer(open))
        .with_grpc_layer(VersionGateLayer)
        .with_grpc_layer(meter);
    s.register(server)
}
