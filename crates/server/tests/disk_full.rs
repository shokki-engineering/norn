//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! A full disk (security review L1): every sample acknowledged `DURABLE` before
//! the disk filled is there after a restart, none twice, and ingest resumes once
//! space returns. Needs a small filesystem of its own, so it is ignored unless
//! `just disk-full` provides one in `NORN_SMALL_FS`. The disk budget's free-space
//! floor is turned off here, since it exists to stop ingest before this happens.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;
mod query_common;

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use common::{Requests, Running, batch, double, seed, start};
use norn_core::{Config, StorageConfig};
use norn_protocol::common::ValueType;
use norn_protocol::common::value::Kind;
use norn_protocol::telemetry::AcknowledgementLevel::Durable;
use norn_protocol::telemetry::telemetry_service_client::TelemetryServiceClient;
use norn_server::DaemonConfig;
use query_common::{all, samples};
use tokio::sync::mpsc;

/// Samples per batch: about 40 KiB on the wire, well under the message limit.
const PER_BATCH: i64 = 1000;
/// Left free beside the pad, so the daemon fills it in a few hundred batches.
const HEADROOM: u64 = 2 << 20;

/// The defaults, with this free-space floor.
fn floor(min_free_bytes: u64) -> DaemonConfig {
    let mut storage = StorageConfig::default();
    storage.min_free_bytes = min_free_bytes;
    DaemonConfig::new(Config::builder().storage(storage).build().unwrap())
}

/// Fills `root` with a pad file, then gives back [`HEADROOM`].
fn pad(root: &Path) -> PathBuf {
    let path = root.join(format!("pad-{}", std::process::id()));
    let mut file = std::fs::File::create(&path).unwrap();
    let block = vec![0x5a_u8; 1 << 20];
    while file.write_all(&block).is_ok() {}
    let len = file.metadata().unwrap().len();
    file.set_len(len.saturating_sub(HEADROOM)).unwrap();
    file.sync_all().unwrap();
    path
}

/// Batch `id`'s samples, with values that do not compress, so the disk fills.
fn noisy(id: i64) -> Vec<norn_protocol::common::Sample> {
    (id * PER_BATCH..(id + 1) * PER_BATCH)
        .map(|seq| {
            let mut s = double("t", seq);
            let bits = (seq as u64)
                .wrapping_mul(0x9e37_79b9_7f4a_7c15)
                .rotate_left(17);
            let kind = Kind::DoubleValue(f64::from_bits(bits >> 2));
            s.value.as_mut().unwrap().kind = Some(kind);
            s
        })
        .collect()
}

/// Appends batch `id` and says whether it reached `DURABLE`. An error of the
/// call itself counts as not durable, which is all a client can conclude.
async fn durable(d: &Running, id: i64) -> bool {
    refusal(d, id).await.is_none()
}

/// As [`durable`], with the error's message when it is not.
async fn refusal(d: &Running, id: i64) -> Option<String> {
    let (tx, rx) = mpsc::channel(1);
    tx.send(batch(id, noisy(id), Durable)).await.unwrap();
    drop(tx);
    let mut client = TelemetryServiceClient::new(d.channel.clone());
    let mut replies = match client.append_samples(Requests(rx)).await {
        Ok(replies) => replies.into_inner(),
        Err(e) => return Some(format!("call refused: {e}")),
    };
    let mut out = Some("never durable".to_owned());
    while let Ok(Some(reply)) = replies.message().await {
        if let Some(e) = &reply.error {
            eprintln!("batch {id}: {}", e.message);
            out = Some(e.message.clone());
        } else if reply.ack_level == Durable as i32 {
            out = None;
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a small filesystem in NORN_SMALL_FS: run `just disk-full`"]
async fn a_full_disk_loses_no_acknowledged_sample_and_ingest_resumes_once_space_returns() {
    let root = PathBuf::from(std::env::var_os("NORN_SMALL_FS").expect("NORN_SMALL_FS"));
    let data = root.join(format!("full-{}", std::process::id()));
    let d = start(floor(0), data).await;
    seed(&d.channel, &[("t", ValueType::Double)]).await;
    let pad = pad(&root);

    let mut acked = BTreeSet::new();
    let mut refused = 0;
    for id in 0..2_000 {
        if durable(&d, id).await {
            acked.insert(id);
        } else {
            refused += 1;
            if refused == 3 {
                break;
            }
        }
    }
    assert_eq!(refused, 3, "the disk never filled");
    assert!(
        !acked.is_empty(),
        "nothing was acknowledged before the disk filled"
    );
    // Still answering on a full disk.
    let before = samples(&d.channel, all()).await;
    eprintln!(
        "acknowledged {} batches; {} samples readable",
        acked.len(),
        before.len()
    );

    d.cancel.cancel();
    let stopped = d.task.await.unwrap();
    eprintln!("stop on a full disk: {:?}", stopped.as_ref().map(|_| ()));
    std::fs::remove_file(pad).unwrap();

    let d = start(floor(0), d.data).await;
    let after = samples(&d.channel, all()).await;
    let seqs: Vec<i64> = after.iter().map(|s| s.sequence).collect();
    let unique: BTreeSet<i64> = seqs.iter().copied().collect();
    assert_eq!(unique.len(), seqs.len(), "a sample was stored twice");
    for id in &acked {
        for seq in id * PER_BATCH..(id + 1) * PER_BATCH {
            assert!(unique.contains(&seq), "acknowledged sample {seq} lost");
        }
    }
    assert!(
        durable(&d, 10_000).await,
        "ingest did not resume once space returned"
    );
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a small filesystem in NORN_SMALL_FS: run `just disk-full`"]
async fn the_free_space_floor_refuses_before_the_disk_fills_and_lifts_without_a_restart() {
    let root = PathBuf::from(std::env::var_os("NORN_SMALL_FS").expect("NORN_SMALL_FS"));
    let data = root.join(format!("floor-{}", std::process::id()));
    let d = start(floor(HEADROOM / 2), data).await;
    seed(&d.channel, &[("t", ValueType::Double)]).await;
    let pad = pad(&root);
    let mut why = None;
    for id in 0..2_000 {
        why = refusal(&d, id).await;
        if why.is_some() {
            break;
        }
    }
    let why = why.expect("the floor never refused");
    assert!(why.contains("min_free_bytes"), "not the floor: {why}");
    std::fs::remove_file(pad).unwrap();
    let started = std::time::Instant::now();
    while refusal(&d, 10_000).await.is_some() {
        assert!(
            started.elapsed().as_secs() < 5,
            "still refused once room returned"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    d.stop().await;
}
