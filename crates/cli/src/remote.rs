//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! Reaching a daemon's `[remote]` listener: TLS to `HOST:PORT`, with the bearer
//! token from a file on every call.

use std::path::PathBuf;
use std::time::Duration;

use norn_ipc::Conn;

use crate::CliError;

/// What `--remote` names: the address, the token file, and a CA to trust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    /// `HOST:PORT`; HOST is also the name the certificate must carry.
    pub address: String,
    /// The file holding the daemon's bearer token.
    pub token_file: PathBuf,
    /// A PEM CA certificate to trust instead of the system's, for a private CA.
    pub ca_file: Option<PathBuf>,
}

/// The credentials for the token in `path`: its contents with surrounding
/// whitespace trimmed, which must be printable ASCII without spaces.
#[cfg(feature = "remote")]
fn bearer(path: &std::path::Path) -> Result<norn_ipc::Credentials, CliError> {
    let fail = |why: String| CliError::Usage(format!("--token-file {}: {why}", path.display()));
    let bytes = std::fs::read(path).map_err(|e| fail(e.to_string()))?;
    let token = String::from_utf8_lossy(bytes.trim_ascii()).into_owned();
    norn_ipc::Credentials::bearer(&token)
        .ok_or_else(|| fail("not a token: printable ASCII without spaces".into()))
}

/// Connects to `remote` over TLS, bounding the connection and each call by
/// `timeout`, and returns a connection presenting its token.
///
/// # Errors
/// [`CliError::Usage`] for an unreadable token or CA file;
/// [`CliError::Remote`] when the daemon cannot be reached or the TLS handshake fails.
#[cfg(feature = "remote")]
pub async fn connect(remote: &Remote, timeout: Duration) -> Result<Conn, CliError> {
    use tonic::transport::{Certificate, ClientTlsConfig, Endpoint};
    let header = bearer(&remote.token_file)?;
    let unreachable = |why: String| CliError::Remote {
        address: remote.address.clone(),
        why,
    };
    let mut tls = ClientTlsConfig::new().with_native_roots();
    if let Some(path) = &remote.ca_file {
        let pem = std::fs::read(path)
            .map_err(|e| CliError::Usage(format!("--ca-file {}: {e}", path.display())))?;
        tls = tls.ca_certificate(Certificate::from_pem(pem));
    }
    let channel = Endpoint::from_shared(format!("https://{}", remote.address))
        .map_err(|e| unreachable(e.to_string()))?
        .tls_config(tls)
        .map_err(|e| unreachable(e.to_string()))?
        .connect_timeout(timeout)
        .timeout(timeout)
        .connect()
        .await
        .map_err(|e| unreachable(full(&e)))?;
    Ok(Conn::new(channel, header))
}

/// Without the `remote` feature there is no TLS client to reach one with.
///
/// # Errors
/// Always [`CliError::Usage`].
#[cfg(not(feature = "remote"))]
pub async fn connect(_: &Remote, _: Duration) -> Result<Conn, CliError> {
    Err(CliError::Usage(
        "--remote: this nornctl was built without remote gRPC (the `remote` feature)".into(),
    ))
}

/// `e` and its causes, one after another: a TLS failure says why only deep down.
#[cfg(feature = "remote")]
fn full(e: &dyn std::error::Error) -> String {
    let mut text = e.to_string();
    let mut next = e.source();
    while let Some(cause) = next {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        next = cause.source();
    }
    text
}
