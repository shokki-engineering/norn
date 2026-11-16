//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! The daemon-wide disk budget: once `max_disk_bytes` is reached every batch is
//! refused, nothing is deleted, and batches are admitted again once room returns,
//! with no restart.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;
mod query_common;
mod upkeep_common;

use std::time::{Duration, Instant};

use common::{Requests, Running, batch, double, seed, start, temp};
use norn_core::{Config, StorageConfig};
use norn_protocol::common::ValueType;
use norn_protocol::common::value::Kind;
use norn_protocol::stream::retention_policy::Limit;
use norn_protocol::telemetry::AcknowledgementLevel::Persisted;
use norn_protocol::telemetry::BackpressureReason;
use norn_protocol::telemetry::telemetry_service_client::TelemetryServiceClient;
use norn_server::DaemonConfig;
use query_common::{all, samples};
use tokio::sync::mpsc;
use upkeep_common::{retention, set_policy, stats};

/// Room for many sealed segments, so that retention has some to remove: each
/// batch spans more than `max_segment_age`, so each fills a segment of its own.
/// Batches go as fast as they are persisted, and the budget is measured each
/// second, so this is passed by however much a second brings.
const LIMIT: u64 = 128 << 10;

/// A small limit, no free-space floor, chunks stored within milliseconds and
/// segments rolled every five seconds of sample time.
fn config() -> DaemonConfig {
    let mut storage = StorageConfig::default();
    storage.max_chunk_age = Duration::from_millis(10);
    storage.max_segment_age = Duration::from_secs(5);
    storage.max_disk_bytes = LIMIT;
    storage.min_free_bytes = 0;
    DaemonConfig::new(Config::builder().storage(storage).build().unwrap())
}

/// Samples per batch. Each batch waits for `PERSISTED`, not `DURABLE`: the test
/// is about admission, and an fsync per batch made it take over a minute.
const PER_BATCH: i64 = 1000;

/// Appends batch `id`: `Ok` once persisted, else the refusal's reason and message.
async fn send(d: &Running, id: i64) -> Result<(), (i32, String)> {
    let (tx, rx) = mpsc::channel(1);
    let first = id * PER_BATCH;
    // Values that do not compress: sequential ones encode to almost nothing.
    let list = (first..first + PER_BATCH)
        .map(|seq| {
            let mut s = double("t", seq);
            let bits = (seq as u64)
                .wrapping_mul(0x9e37_79b9_7f4a_7c15)
                .rotate_left(17);
            s.value.as_mut().unwrap().kind = Some(Kind::DoubleValue(f64::from_bits(bits >> 2)));
            s
        })
        .collect();
    tx.send(batch(id, list, Persisted)).await.unwrap();
    drop(tx);
    let mut client = TelemetryServiceClient::new(d.channel.clone());
    let mut replies = client
        .append_samples(Requests(rx))
        .await
        .unwrap()
        .into_inner();
    let mut out = Err((0, "no reply".to_owned()));
    while let Some(reply) = replies.message().await.unwrap() {
        out = match (reply.error, reply.admission) {
            (Some(e), a) => Err((a.map_or(0, |a| a.reason), e.message)),
            (None, _) if reply.ack_level == Persisted as i32 => Ok(()),
            (None, _) => continue,
        };
    }
    out
}

/// Sends batches from `from` until one is refused, giving its id and refusal.
/// Each admitted batch is stored before the next is sent, so each is a chunk, and
/// a segment, of its own: retention then has sealed segments to remove, however
/// a platform's timers would otherwise group batches into chunks.
async fn until_refused(d: &Running, from: i64) -> (i64, (i32, String)) {
    for id in from..from + 2_000 {
        if let Err(refusal) = send(d, id).await {
            return (id, refusal);
        }
        let started = Instant::now();
        while stats(&d.channel).await.buffered_sample_count > 0 {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "batch {id} never stored"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    panic!("never refused");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_spent_budget_refuses_every_batch_deletes_nothing_and_lifts_once_room_returns() {
    let d = start(config(), temp("disk-budget")).await;
    seed(&d.channel, &[("t", ValueType::Double)]).await;

    let (refused_at, (reason, message)) = until_refused(&d, 0).await;
    assert_eq!(reason, BackpressureReason::StorageUnavailable as i32);
    assert!(message.contains("max_disk_bytes"), "{message}");
    let stored = samples(&d.channel, all()).await.len();
    assert_eq!(
        stored,
        usize::try_from(refused_at * PER_BATCH).unwrap(),
        "lost or kept a refused batch"
    );

    // Making room is the operator's or a policy's decision, never the budget's.
    set_policy(&d.channel, Limit::MaxBytes(1)).await;
    let freed = retention(&d.channel, false).await;
    assert!(
        freed.removed_segment_count > 0,
        "refused at batch {refused_at}, yet nothing to remove: {} segments on disk",
        upkeep_common::segments(&d.data).len()
    );
    let started = Instant::now();
    let mut admitted = send(&d, refused_at).await;
    while admitted.is_err() && started.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(50)).await;
        admitted = send(&d, refused_at).await;
    }
    assert_eq!(admitted, Ok(()), "still refused once room returned");
    d.stop().await;
}
