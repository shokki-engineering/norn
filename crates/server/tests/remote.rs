//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! Remote gRPC (`[remote]`): TLS, a bearer token on every call, the same database
//! as the local endpoint, and a refused start for a token file that is not a secret.
//!
//! `fixtures/tls` holds a test CA (`ca.pem`) and a P-256 certificate and key for
//! `localhost` and 127.0.0.1 that it signed (`cert.pem`, `key.pem`), valid until
//! 2126. Test-only: the CA's key was discarded. Made with OpenSSL:
//!
//! ```text
//! openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 36500 \
//!   -subj /CN=norn-test-ca -keyout ca.key -out ca.pem \
//!   -addext basicConstraints=critical,CA:TRUE -addext keyUsage=critical,keyCertSign
//! openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj /CN=localhost \
//!   -keyout key.pem -out leaf.csr
//! openssl x509 -req -in leaf.csr -CA ca.pem -CAkey ca.key -CAcreateserial -days 36500 \
//!   -out cert.pem -extfile ext   # ext: subjectAltName=DNS:localhost,IP:127.0.0.1,
//!                                # basicConstraints=critical,CA:FALSE,
//!                                # keyUsage=critical,digitalSignature, extendedKeyUsage=serverAuth
//! ```
#![cfg(feature = "remote")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;
mod query_common;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use common::{Requests, Running, batch, double, seed, start, temp};
use norn_protocol::admin::GetServerRequest;
use norn_protocol::admin::admin_service_client::AdminServiceClient;
use norn_protocol::common::ValueType;
use norn_protocol::telemetry::AcknowledgementLevel::Durable;
use norn_protocol::telemetry::telemetry_service_client::TelemetryServiceClient;
use norn_server::{DaemonConfig, DaemonError, RemoteConfig};
use query_common::{all, samples};
use tokio::sync::mpsc;
use tonic::Code;
use tonic::transport::{Certificate, Channel, ClientTlsConfig};

const TOKEN: &str = "0123456789abcdef0123456789abcdef-test";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tls")
        .join(name)
}

/// Writes `token` to a file of mode `mode` (Unix) in a fresh directory.
fn token_file(tag: &str, token: &str, mode: u32) -> PathBuf {
    let path = temp(tag).with_extension("token");
    std::fs::write(&path, format!("{token}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = mode;
    path
}

fn config(token: PathBuf) -> DaemonConfig {
    let any: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let remote = RemoteConfig::new(any, fixture("cert.pem"), fixture("key.pem"), token);
    DaemonConfig::default().remote(Some(remote))
}

/// A daemon serving locally and remotely, and a TLS channel to the remote side.
async fn started(tag: &str) -> (Running, Channel) {
    let d = start(config(token_file(tag, TOKEN, 0o600)), temp(tag)).await;
    let addr = d.remote.expect("no remote listener");
    let ca = Certificate::from_pem(std::fs::read(fixture("ca.pem")).unwrap());
    let tls = ClientTlsConfig::new()
        .ca_certificate(ca)
        .domain_name("localhost");
    let url = format!("https://localhost:{}", addr.port());
    let channel = Channel::from_shared(url).unwrap().tls_config(tls).unwrap();
    (d, channel.connect().await.unwrap())
}

/// `GetServer` over `channel` presenting `token`, if any.
async fn get_server(channel: &Channel, token: Option<&str>) -> Result<(), tonic::Status> {
    let get = GetServerRequest {
        name: "server".into(),
    };
    let mut request = tonic::Request::new(get);
    if let Some(token) = token {
        let value = format!("Bearer {token}").parse().unwrap();
        request.metadata_mut().insert("authorization", value);
    }
    let mut client = AdminServiceClient::new(channel.clone());
    client.get_server(request).await.map(|_| ())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_without_the_right_token_is_unauthenticated() {
    let (d, remote) = started("remote-token").await;
    for token in [None, Some("wrong"), Some(&TOKEN[..TOKEN.len() - 1])] {
        let code = get_server(&remote, token).await.unwrap_err().code();
        assert_eq!(code, Code::Unauthenticated, "token {token:?}");
    }
    get_server(&remote, Some(TOKEN)).await.unwrap();
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_remote_append_is_stored_in_the_database_the_local_endpoint_reads() {
    let (d, remote) = started("remote-append").await;
    seed(&d.channel, &[("t", ValueType::Double)]).await;
    let (tx, rx) = mpsc::channel(1);
    let list = (0..10).map(|n| double("t", n)).collect();
    tx.send(batch(1, list, Durable)).await.unwrap();
    drop(tx);
    let mut request = tonic::Request::new(Requests(rx));
    let value = format!("Bearer {TOKEN}").parse().unwrap();
    request.metadata_mut().insert("authorization", value);
    let mut client = TelemetryServiceClient::new(remote);
    let mut replies = client.append_samples(request).await.unwrap().into_inner();
    let mut reached = 0;
    while let Some(reply) = replies.message().await.unwrap() {
        assert!(reply.error.is_none(), "{:?}", reply.error);
        reached = reached.max(reply.ack_level);
    }
    assert_eq!(reached, Durable as i32);
    // Read back on the local socket, which needs no token.
    assert_eq!(samples(&d.channel, all()).await.len(), 10);
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn plaintext_is_not_served_on_the_remote_listener() {
    let (d, _) = started("remote-plain").await;
    let url = format!("http://127.0.0.1:{}", d.remote.unwrap().port());
    let plain = Channel::from_shared(url).unwrap().connect().await;
    if let Ok(channel) = plain {
        assert!(get_server(&channel, Some(TOKEN)).await.is_err());
    }
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_file_that_is_short_or_readable_by_others_stops_the_daemon() {
    let mut cases = vec![token_file("remote-short", "too-short", 0o600)];
    if cfg!(unix) {
        cases.push(token_file("remote-mode", TOKEN, 0o640));
    }
    for token in cases {
        let local = "tcp:127.0.0.1:0".parse().unwrap();
        let config = config(token.clone())
            .listen(local)
            .data_dir(temp("remote-bad"));
        let refused = norn_server::Daemon::bind(config).await.unwrap_err();
        assert!(
            matches!(refused, DaemonError::RemoteToken(_)),
            "{token:?}: {refused}"
        );
    }
}
