//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! A size limit is applied when a segment seals, not an hour later on the
//! retention schedule; and not at all when background maintenance is off.
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

use common::{Running, append, batch, double, seed, start, temp};
use norn_core::{Config, MaintenanceConfig, StorageConfig};
use norn_protocol::common::ValueType;
use norn_protocol::stream::retention_policy::Limit;
use norn_protocol::telemetry::AcknowledgementLevel::Durable;
use norn_server::DaemonConfig;
use query_common::{all, samples};
use upkeep_common::{server, set_policy};

/// Chunks stored within milliseconds, segments rolled every five seconds of
/// sample time, and the retention schedule left at its default of an hour.
fn config(background: bool) -> DaemonConfig {
    let mut storage = StorageConfig::default();
    storage.max_chunk_age = Duration::from_millis(10);
    storage.max_segment_age = Duration::from_secs(5);
    let mut maintenance = MaintenanceConfig::default();
    maintenance.background = background;
    let engine = Config::builder()
        .storage(storage)
        .maintenance(maintenance)
        .build()
        .unwrap();
    DaemonConfig::new(engine)
}

/// A one-byte limit, then six batches of ten seconds of samples, each stored
/// before the next, so the stream seals a segment at least once per batch.
async fn filled(tag: &str, background: bool) -> Running {
    let d = start(config(background), temp(tag)).await;
    seed(&d.channel, &[("t", ValueType::Double)]).await;
    set_policy(&d.channel, Limit::MaxBytes(1)).await;
    for n in 0..6 {
        let list = (n * 10..n * 10 + 10).map(|s| double("t", s)).collect();
        append(&d.channel, vec![batch(n + 1, list, Durable)]).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    d
}

async fn removed(d: &Running) -> i64 {
    let report = server(&d.channel).await.maintenance.unwrap();
    report.retention_segment_count
}

#[tokio::test(flavor = "multi_thread")]
async fn a_size_limit_is_applied_as_segments_seal_without_waiting_for_the_schedule() {
    let d = filled("size-seal", true).await;
    let started = Instant::now();
    while removed(&d).await == 0 && started.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(removed(&d).await > 0, "nothing removed before the schedule");
    let left = samples(&d.channel, all()).await;
    assert!(left.len() < 60, "every sample kept: {}", left.len());
    // The newest samples are never what a size limit takes.
    assert!(left.iter().any(|s| s.sequence == 59));
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn with_background_maintenance_off_a_seal_removes_nothing() {
    let d = filled("size-off", false).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(removed(&d).await, 0);
    assert_eq!(samples(&d.channel, all()).await.len(), 60);
    d.stop().await;
}
