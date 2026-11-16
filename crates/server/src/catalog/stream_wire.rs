//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! A stream record as the wire carries it, and its quota both ways.

use norn_metadata::StreamRecord;
use norn_protocol::stream::{StorageQuota, Stream, StreamStats};
use tonic::Status;

use super::upkeep::retention_to_wire;
use super::{Catalog, annotation_map, etag};
use crate::convert::time_to_wire;
use crate::status::bad_field;

/// A quota off the wire: `None` for none, else positive.
pub(super) fn quota_from_wire(quota: Option<StorageQuota>) -> Result<Option<u64>, Status> {
    let Some(quota) = quota else { return Ok(None) };
    u64::try_from(quota.max_bytes)
        .ok()
        .filter(|n| *n > 0)
        .map(Some)
        .ok_or_else(|| bad_field("quota.max_bytes", "must be positive"))
}

fn quota_to_wire(quota: Option<u64>) -> Option<StorageQuota> {
    quota.map(|n| StorageQuota {
        max_bytes: i64::try_from(n).unwrap_or(i64::MAX),
    })
}

impl Catalog {
    pub(super) fn stream_to_wire(&self, r: &StreamRecord) -> Stream {
        let stats = self.ingest.stats(r.descriptor.id);
        let wide = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
        Stream {
            name: format!("streams/{}", r.descriptor.name),
            display_name: r.display_name.clone(),
            retention: retention_to_wire(r.retention),
            annotations: annotation_map(&r.descriptor.metadata),
            stats: Some(StreamStats {
                sample_count: wide(stats.samples),
                buffered_sample_count: wide(stats.buffered),
                corrupt_sample_count: wide(stats.corrupt_samples),
                stored_bytes: wide(stats.stored_bytes),
                oldest_sample_time: stats.time_range.map(|(a, _)| time_to_wire(a)),
                newest_sample_time: stats.time_range.map(|(_, b)| time_to_wire(b)),
            }),
            create_time: Some(time_to_wire(r.create_time)),
            update_time: Some(time_to_wire(r.update_time)),
            etag: etag(r.revision),
            quota: quota_to_wire(r.quota),
        }
    }
}
