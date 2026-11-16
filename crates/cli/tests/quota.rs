//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! `nornctl quota` against a real daemon: set, read back, refuse a bad one, remove.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use support::Running;

#[tokio::test(flavor = "multi_thread")]
async fn quota_sets_reads_refuses_and_removes() {
    let d = Running::start().await;
    let run = |words: Vec<String>| {
        let d = &d;
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        d.nornctl(&words)
    };
    let ok = |words: &[&str]| d.ok(words);
    tokio::task::block_in_place(|| {
        ok(&["streams", "create", "plant"]);
        let set = ok(&["quota", "set", "plant", "10000000000b"]);
        assert_eq!(set, "streams/plant max_bytes=10000000000\n");
        assert_eq!(ok(&["quota", "get", "plant"]), set);
        ok(&["retention", "set", "plant", "5000b"]);
        let below = run(vec![
            "quota".into(),
            "set".into(),
            "plant".into(),
            "4000b".into(),
        ]);
        assert_eq!(below.status.code(), Some(1), "{below:?}");
        let said = String::from_utf8_lossy(&below.stderr);
        assert!(said.contains("InvalidArgument"), "{said}");
        assert_eq!(
            ok(&["quota", "set", "plant", "none"]),
            "streams/plant none\n"
        );
        let usage = run(vec![
            "quota".into(),
            "set".into(),
            "plant".into(),
            "12".into(),
        ]);
        assert_eq!(usage.status.code(), Some(1), "{usage:?}");
    });
    d.stop().await;
}
