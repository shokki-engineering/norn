//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! The `[remote]` section: off unless `listen` is set, every file required with it,
//! the guide's example exact, and refused by a build without the `remote` feature.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use norn_server::{ConfigError, DaemonConfig};

/// The `[remote]` example in docs/guides/configuration.md.
fn guide_example() -> String {
    let guide = include_str!("../../../docs/guides/configuration.md").replace("\r\n", "\n");
    let (_, rest) = guide.split_once("```toml\n[remote]\n").unwrap();
    let (body, _) = rest.split_once("```").unwrap();
    format!("[remote]\n{body}")
}

fn refused(text: &str) -> &'static str {
    match DaemonConfig::from_toml(text) {
        Err(ConfigError::Invalid { setting, .. }) => setting,
        other => panic!("{text:?} was not refused: {other:?}"),
    }
}

#[test]
fn no_section_or_an_empty_one_serves_nothing_remotely() {
    for text in ["", "[remote]\n"] {
        assert!(
            DaemonConfig::from_toml(text)
                .unwrap()
                .remote_settings()
                .is_none()
        );
    }
}

#[test]
fn a_file_without_listen_is_refused_rather_than_ignored() {
    assert_eq!(refused("[remote]\ntoken_file = \"/t\"\n"), "remote.listen");
}

#[cfg(feature = "remote")]
#[test]
fn the_guides_example_parses_to_what_it_says() {
    let config = DaemonConfig::from_toml(&guide_example()).unwrap();
    let remote = config.remote_settings().unwrap();
    assert_eq!(
        remote.max_clients(),
        norn_server::config::DEFAULT_REMOTE_CLIENTS
    );
    let expected = norn_server::RemoteConfig::new(
        "0.0.0.0:7443".parse().unwrap(),
        "/etc/norn/tls/cert.pem".into(),
        "/etc/norn/tls/key.pem".into(),
        "/etc/norn/token".into(),
    );
    assert_eq!(remote, &expected);
}

#[cfg(feature = "remote")]
#[test]
fn listen_needs_every_file_and_a_sane_client_limit() {
    let base = "[remote]\nlisten = \"0.0.0.0:7443\"\n";
    let cert = "cert_file = \"/c\"\n";
    let key = "key_file = \"/k\"\n";
    let token = "token_file = \"/t\"\n";
    assert_eq!(refused(&format!("{base}{key}{token}")), "remote.cert_file");
    assert_eq!(refused(&format!("{base}{cert}{token}")), "remote.key_file");
    assert_eq!(refused(&format!("{base}{cert}{key}")), "remote.token_file");
    let all = format!("{base}{cert}{key}{token}");
    assert_eq!(
        refused(&format!("{all}max_clients = 0\n")),
        "remote.max_clients"
    );
    let bad = "[remote]\nlisten = \"example.com\"\n";
    assert_eq!(
        refused(&format!("{bad}{cert}{key}{token}")),
        "remote.listen"
    );
}

#[cfg(not(feature = "remote"))]
#[test]
fn a_build_without_the_feature_refuses_the_section() {
    assert_eq!(refused(&guide_example()), "remote");
}
