//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! `nornctl --remote` against a real daemon's `[remote]` listener: the token and
//! TLS with a private CA, the same database as the local endpoint, and clear
//! refusals. Needs both `remote` features: `just test` turns them on. The test
//! certificate is norn-server's (crates/server/tests/remote.rs says how it was made).
#![cfg(feature = "remote")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use norn_runtime::Health;
use norn_server::{Daemon, DaemonConfig, RemoteConfig};
use support::TempDir;
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "0123456789abcdef0123456789abcdef-cli";

fn fixture(name: &str) -> PathBuf {
    let server = Path::new(env!("CARGO_MANIFEST_DIR")).join("../server");
    server.join("tests/fixtures/tls").join(name)
}

/// Writes `token` to an owner-only file in `dir`.
fn token_file(dir: &Path, name: &str, token: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("{token}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    path
}

/// The real `nornctl`, through the remote listener at `port` with `token`.
fn nornctl(port: u16, token: &Path, words: &[&str]) -> Output {
    let address = format!("localhost:{port}");
    let ca = fixture("ca.pem");
    Command::new(env!("CARGO_BIN_EXE_nornctl"))
        .args(["--remote", &address, "--token-file"])
        .arg(token)
        .arg("--ca-file")
        .arg(ca)
        .args(["--timeout", "30"])
        .args(words)
        .output()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn nornctl_reaches_a_remote_daemon_with_its_token_and_only_with_it() {
    let dir = TempDir::new();
    let any: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let good = token_file(&dir.0, "token", TOKEN);
    let wrong = token_file(&dir.0, "wrong", &TOKEN.replace('0', "1"));
    let remote = RemoteConfig::new(any, fixture("cert.pem"), fixture("key.pem"), good.clone());
    let config = DaemonConfig::default()
        .listen("tcp:127.0.0.1:0".parse().unwrap())
        .data_dir(dir.0.join("data"))
        .remote(Some(remote));
    let daemon = Daemon::bind(config)
        .await
        .expect("norn-server needs its `remote` feature");
    let port = daemon.remote_addr().unwrap().port();
    let local = daemon.local_endpoint().to_string();
    let mut health = daemon.health();
    let cancel = CancellationToken::new();
    let task = tokio::spawn(daemon.run(cancel.clone()));
    assert_eq!(health.reached(Health::Serving).await, Health::Serving);

    let created = tokio::task::spawn_blocking(move || {
        let made = nornctl(port, &good, &["streams", "create", "plant"]);
        let refused = nornctl(port, &wrong, &["streams"]);
        (made, refused)
    });
    let (made, refused) = created.await.unwrap();
    assert!(made.status.success(), "{made:?}");
    assert_eq!(refused.status.code(), Some(1), "{refused:?}");
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("Unauthenticated"), "{said}");

    // What the remote call made, the local endpoint sees.
    let listed = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_nornctl"))
            .args(["--endpoint", &local, "streams"])
            .output()
            .unwrap()
    });
    let listed = listed.await.unwrap();
    assert!(
        String::from_utf8_lossy(&listed.stdout).contains("streams/plant"),
        "{listed:?}"
    );
    cancel.cancel();
    let _ = task.await;
}

#[test]
fn remote_flags_that_do_not_go_together_are_a_usage_error() {
    let run = |words: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_nornctl"))
            .args(words)
            .output()
            .unwrap();
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    let (code, said) = run(&["--remote", "localhost:7443", "status"]);
    assert_eq!(code, Some(2));
    assert!(said.contains("--remote needs --token-file"), "{said}");
    let (code, _) = run(&["--token-file", "/t", "status"]);
    assert_eq!(code, Some(2));
    let both = [
        "--remote",
        "h:1",
        "--token-file",
        "/t",
        "--endpoint",
        "tcp:127.0.0.1:1",
    ];
    let (code, said) = run(&[&both[..], &["status"]].concat());
    assert_eq!(code, Some(2));
    assert!(said.contains("give one"), "{said}");
}
