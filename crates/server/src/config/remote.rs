//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! The `[remote]` section: gRPC for other machines, over TLS, with a bearer
//! token. Parsing is pure: the files it names are read when the daemon binds.

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::Deserialize;

use super::ConfigError;

/// Remote clients at once, unless `max_clients` says otherwise. Picked, not
/// measured: each holds a flow-control window from `ipc_bytes`.
pub const DEFAULT_REMOTE_CLIENTS: usize = 16;

/// The most `max_clients` may be, the same ceiling as local clients.
const MAX_REMOTE_CLIENTS: usize = 1024;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Remote {
    listen: Option<String>,
    cert_file: Option<String>,
    key_file: Option<String>,
    token_file: Option<String>,
    max_clients: Option<usize>,
}

/// Validated `[remote]` settings: where to listen, the TLS certificate and key,
/// and the file holding the token every call must present.
///
/// Concurrency: plain data, `Send + Sync`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteConfig {
    pub(crate) listen: SocketAddr,
    pub(crate) cert_file: PathBuf,
    pub(crate) key_file: PathBuf,
    pub(crate) token_file: PathBuf,
    pub(crate) max_clients: usize,
}

impl RemoteConfig {
    /// Listens on `listen` with this certificate, key and token file, and
    /// [`DEFAULT_REMOTE_CLIENTS`]. Port 0 binds an ephemeral port.
    #[must_use]
    pub fn new(
        listen: SocketAddr,
        cert_file: PathBuf,
        key_file: PathBuf,
        token_file: PathBuf,
    ) -> Self {
        Self {
            listen,
            cert_file,
            key_file,
            token_file,
            max_clients: DEFAULT_REMOTE_CLIENTS,
        }
    }

    /// The most remote clients connected at once.
    #[must_use]
    pub fn max_clients(&self) -> usize {
        self.max_clients
    }
}

impl Remote {
    /// The settings, `None` with no `listen`. Every other key needs `listen`, and
    /// `listen` needs the certificate, the key and the token file.
    pub(super) fn config(self) -> Result<Option<RemoteConfig>, ConfigError> {
        let Some(listen) = self.listen else {
            let named = self.cert_file.is_some()
                || self.key_file.is_some()
                || self.token_file.is_some()
                || self.max_clients.is_some();
            return match named {
                true => Err(ConfigError::invalid(
                    "remote.listen",
                    "required by [remote]",
                )),
                false => Ok(None),
            };
        };
        if !cfg!(feature = "remote") {
            let detail = "this norn was built without remote gRPC (the `remote` feature)";
            return Err(ConfigError::invalid("remote", detail));
        }
        let listen = listen.parse().map_err(|_| {
            ConfigError::invalid("remote.listen", format!("{listen:?}: expected ip:port"))
        })?;
        let need = |value: Option<String>, setting| {
            value
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .ok_or_else(|| ConfigError::invalid(setting, "required with remote.listen"))
        };
        let max_clients = self.max_clients.unwrap_or(DEFAULT_REMOTE_CLIENTS);
        if !(1..=MAX_REMOTE_CLIENTS).contains(&max_clients) {
            let detail = format!("{max_clients}: must be 1 to {MAX_REMOTE_CLIENTS}");
            return Err(ConfigError::invalid("remote.max_clients", detail));
        }
        Ok(Some(RemoteConfig {
            listen,
            cert_file: need(self.cert_file, "remote.cert_file")?,
            key_file: need(self.key_file, "remote.key_file")?,
            token_file: need(self.token_file, "remote.token_file")?,
            max_clients,
        }))
    }
}
