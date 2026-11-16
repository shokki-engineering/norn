//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! Streams that sealed a segment since maintenance last looked.
//!
//! A size limit applied only on the retention schedule let a busy stream run past
//! it for a whole interval, an hour by default. The store thread marks a stream
//! here each time it seals, and maintenance wakes and applies that stream's
//! size limit at once. Marking takes one short lock and never waits on
//! maintenance, so a slow worker cannot hold up a store.

use std::collections::BTreeSet;
use std::sync::{Mutex, PoisonError};

use norn_core::StreamId;
use tokio::sync::Notify;

use super::Ingest;

/// The marked streams and the worker's wake-up.
///
/// Memory: one entry per stream at most, however often it seals.
/// Concurrency: marked from store threads, drained by the maintenance worker.
#[derive(Debug, Default)]
pub(crate) struct SealedStreams {
    marked: Mutex<BTreeSet<StreamId>>,
    wake: Notify,
}

impl SealedStreams {
    /// Marks `stream`. A wake-up with no worker waiting is kept for its next wait.
    pub(crate) fn mark(&self, stream: StreamId) {
        self.marked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(stream);
        self.wake.notify_one();
    }
}

impl Ingest {
    /// Resolves once a stream has sealed a segment since the last
    /// [`take_sealed`](Self::take_sealed). Cancel-safe.
    pub async fn sealed_wake(&self) {
        self.ctx.sealed.wake.notified().await;
    }

    /// The streams marked since the last call, in id order, and clears them.
    #[must_use]
    pub fn take_sealed(&self) -> Vec<StreamId> {
        let mut marked = self
            .ctx
            .sealed
            .marked
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut *marked).into_iter().collect()
    }
}
