//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! A stream's hard quota (`Stream.quota`): admission refuses a batch once the
//! stream's stored bytes reach it. Nothing is deleted.

use std::sync::PoisonError;
use std::sync::atomic::Ordering;

use norn_core::StreamId;

use super::Ingest;
use super::decision::{Admission, Decision, Reason};
use super::shared::Shared;

/// The refusal for a stream at its quota: once its stored bytes reach it. Only
/// stored bytes count, the unit the quota is in, so the stream can pass it by what
/// its write buffers hold, compressed. One atomic load without a quota.
pub(super) fn refusal(shared: &Shared) -> Option<Admission> {
    let quota = shared.quota.load(Ordering::Acquire);
    if quota == 0 {
        return None;
    }
    let stored = shared
        .stats
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .stored_bytes;
    (stored >= quota).then(|| {
        let message = format!("the stream holds {stored} bytes, at its quota of {quota}");
        Admission::rejected(Decision::reject(Reason::QuotaExceeded, None, message))
    })
}

impl Ingest {
    /// Takes `stream`'s new quota, `None` for none, from its next batch. A stream
    /// not yet running reads its quota from the registry when it starts.
    pub fn set_quota(&self, stream: StreamId, quota: Option<u64>) {
        if let Some(shared) = self.running(stream) {
            shared.quota.store(quota.unwrap_or(0), Ordering::Release);
        }
    }
}
