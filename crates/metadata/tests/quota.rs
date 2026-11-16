//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! Stream quotas in the registry: validated against retention, durable, and
//! raising the log to version 3 only once one is written, so a database that
//! never sets a quota stays readable by an older Norn.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{Dir, open, t};
use norn_metadata::{MetadataError, Retention, StreamPatch, StreamSpec, Write, inspect_log};

fn version(dir: &Dir) -> u32 {
    inspect_log(&dir.0).unwrap().version
}

fn set(r: &norn_metadata::Registry, quota: Option<u64>, at: i64) -> Result<(), MetadataError> {
    let patch = StreamPatch::default().quota(quota);
    r.update_stream("plant", patch, None, t(at), Write::Commit)
        .map(|_| ())
}

#[test]
fn a_quota_outlives_a_restart_and_raises_the_log_to_version_3_only_when_written() {
    let dir = Dir::new();
    let r = open(&dir.0);
    r.create_stream("plant", StreamSpec::default(), t(1), Write::Commit)
        .unwrap();
    assert_eq!(
        version(&dir),
        2,
        "no quota yet: an older Norn must still read it"
    );
    set(&r, Some(4096), 2).unwrap();
    assert_eq!(version(&dir), 3);
    drop(r);
    let r = open(&dir.0);
    assert_eq!(r.stream("plant").unwrap().quota, Some(4096));
    set(&r, None, 3).unwrap();
    drop(r);
    let r = open(&dir.0);
    assert_eq!(r.stream("plant").unwrap().quota, None);
    assert_eq!(
        version(&dir),
        3,
        "a raised header is not lowered by an append"
    );
}

#[test]
fn a_rewrite_with_no_quota_left_writes_version_2_again() {
    let dir = Dir::new();
    let r = open(&dir.0);
    let spec = StreamSpec::default().quota(Some(4096));
    r.create_stream("plant", spec, t(1), Write::Commit).unwrap();
    assert_eq!(version(&dir), 3);
    set(&r, None, 2).unwrap();
    // Enough dead records that the log rewrites itself from the live state.
    for at in 3..1_100 {
        let patch = StreamPatch::new(Some(format!("n{at}")), None, None);
        r.update_stream("plant", patch, None, t(at), Write::Commit)
            .unwrap();
    }
    assert!(
        inspect_log(&dir.0).unwrap().records < 100,
        "the log never rewrote"
    );
    assert_eq!(version(&dir), 2);
    drop(r);
    assert_eq!(open(&dir.0).stream("plant").unwrap().quota, None);
}

#[test]
fn a_quota_must_be_positive_and_above_a_size_retention() {
    let dir = Dir::new();
    let r = open(&dir.0);
    let sized = StreamSpec::new(String::new(), Retention::MaxBytes(1000), Default::default());
    r.create_stream("plant", sized, t(1), Write::Commit)
        .unwrap();
    for quota in [0, 999, 1000] {
        let refused = set(&r, Some(quota), 2).unwrap_err();
        assert!(
            matches!(
                refused,
                MetadataError::Invalid {
                    field: "quota.max_bytes",
                    ..
                }
            ),
            "{quota}: {refused}"
        );
    }
    set(&r, Some(1001), 2).unwrap();
    // Nor may retention grow to meet the quota afterwards.
    let patch = StreamPatch::new(None, Some(Retention::MaxBytes(2000)), None);
    let refused = r.update_stream("plant", patch, None, t(3), Write::Commit);
    assert!(matches!(refused, Err(MetadataError::Invalid { .. })));
    assert_eq!(version(&dir), 3);
    let created = StreamSpec::new(String::new(), Retention::MaxBytes(10), Default::default());
    let refused = r.create_stream("bad", created.quota(Some(10)), t(4), Write::Commit);
    assert!(matches!(refused, Err(MetadataError::Invalid { .. })));
}
