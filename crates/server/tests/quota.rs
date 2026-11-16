//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! `Stream.quota`: batches refused with QUOTA_EXCEEDED once the stream's stored
//! bytes reach it, nothing deleted, admitted again when it is raised, kept across a
//! restart, and refused at or below a size retention.
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
use norn_protocol::stream::stream_service_client::StreamServiceClient;
use norn_protocol::stream::{StorageQuota, Stream, UpdateStreamRequest};
use norn_protocol::telemetry::AcknowledgementLevel::Persisted;
use norn_protocol::telemetry::BackpressureReason;
use norn_protocol::telemetry::telemetry_service_client::TelemetryServiceClient;
use norn_server::DaemonConfig;
use query_common::{all, samples};
use tokio::sync::mpsc;
use upkeep_common::{PLANT, set_policy, stats, stream};

const PER_BATCH: i64 = 200;

fn config() -> DaemonConfig {
    let mut storage = StorageConfig::default();
    storage.max_chunk_age = Duration::from_millis(10);
    storage.min_free_bytes = 0;
    DaemonConfig::new(Config::builder().storage(storage).build().unwrap())
}

async fn set_quota(d: &Running, max_bytes: Option<i64>) -> Result<Stream, tonic::Status> {
    let request = UpdateStreamRequest {
        stream: Some(Stream {
            name: PLANT.into(),
            quota: max_bytes.map(|max_bytes| StorageQuota { max_bytes }),
            ..Stream::default()
        }),
        update_mask: Some(prost_types::FieldMask {
            paths: vec!["quota".into()],
        }),
        validate_only: false,
    };
    let mut client = StreamServiceClient::new(d.channel.clone());
    client.update_stream(request).await.map(|r| r.into_inner())
}

/// Batch `id`, of values that do not compress; `Ok` once persisted and stored.
async fn send(d: &Running, id: i64) -> Result<(), i32> {
    let (tx, rx) = mpsc::channel(1);
    let list = (id * PER_BATCH..(id + 1) * PER_BATCH)
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
    let mut out = Err(0);
    while let Some(reply) = replies.message().await.unwrap() {
        if reply.error.is_some() {
            return Err(reply.admission.map_or(0, |a| a.reason));
        }
        if reply.ack_level == Persisted as i32 {
            out = Ok(());
        }
    }
    let started = Instant::now();
    while stats(&d.channel).await.buffered_sample_count > 0 {
        assert!(started.elapsed() < Duration::from_secs(10), "never stored");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    out
}

async fn until_refused(d: &Running, from: i64) -> (i64, i32) {
    for id in from..from + 1_000 {
        if let Err(reason) = send(d, id).await {
            return (id, reason);
        }
    }
    panic!("never refused");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stream_at_its_quota_refuses_batches_keeps_everything_and_admits_once_raised() {
    let d = start(config(), temp("quota")).await;
    seed(&d.channel, &[("t", ValueType::Double)]).await;
    set_quota(&d, Some(16 << 10)).await.unwrap();

    let (refused_at, reason) = until_refused(&d, 0).await;
    assert_eq!(reason, BackpressureReason::QuotaExceeded as i32);
    assert!(refused_at > 0, "refused before anything was stored");
    let held = samples(&d.channel, all()).await.len();
    assert_eq!(
        held,
        usize::try_from(refused_at * PER_BATCH).unwrap(),
        "lost or kept a refused batch"
    );
    assert!(stats(&d.channel).await.stored_bytes >= 16 << 10);

    // Raised, the very next batch is admitted: no restart, nothing deleted.
    set_quota(&d, Some(1 << 30)).await.unwrap();
    send(&d, refused_at).await.unwrap();
    assert_eq!(
        samples(&d.channel, all()).await.len(),
        held + PER_BATCH as usize
    );

    let data = d.stop_keeping().await;
    let d = start(config(), data).await;
    let kept = stream(&d.channel).await.quota.unwrap();
    assert_eq!(
        kept.max_bytes,
        1 << 30,
        "the quota did not outlive a restart"
    );
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_quota_at_or_below_a_size_retention_or_not_positive_is_invalid() {
    let d = start(config(), temp("quota-bad")).await;
    seed(&d.channel, &[("t", ValueType::Double)]).await;
    set_policy(&d.channel, Limit::MaxBytes(1 << 20)).await;
    for max_bytes in [0, -1, 1 << 20] {
        let refused = set_quota(&d, Some(max_bytes)).await.unwrap_err();
        assert_eq!(
            refused.code(),
            tonic::Code::InvalidArgument,
            "{max_bytes}: {refused:?}"
        );
    }
    set_quota(&d, Some((1 << 20) + 1)).await.unwrap();
    // And a quota can be removed.
    assert!(set_quota(&d, None).await.unwrap().quota.is_none());
    d.stop().await;
}
