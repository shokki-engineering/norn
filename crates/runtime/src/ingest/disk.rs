//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! The daemon-wide disk budget (security review L1), as admission sees it.
//!
//! Admission never touches a disk, so it cannot measure one: maintenance
//! measures, and leaves here whether the budget is spent and why. Admission reads
//! one flag per batch, and the message only when refusing. Before the first
//! measurement the budget counts as unspent.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use super::Ingest;
use super::decision::{Admission, Decision, Reason};

/// How long a client refused for a spent budget waits before retrying.
const DISK_DELAY: Duration = Duration::from_secs(1);

/// The limits, and what the last measurement concluded.
///
/// Concurrency: written by the maintenance worker, read by every admission.
#[derive(Debug, Default)]
pub(crate) struct DiskGate {
    /// `[storage] max_disk_bytes`; zero is no limit.
    pub(crate) max_disk_bytes: u64,
    /// `[storage] min_free_bytes`; zero is no floor.
    pub(crate) min_free_bytes: u64,
    spent: AtomicBool,
    why: Mutex<String>,
}

impl DiskGate {
    pub(crate) fn new(max_disk_bytes: u64, min_free_bytes: u64) -> Self {
        Self {
            max_disk_bytes,
            min_free_bytes,
            ..Self::default()
        }
    }
}

impl Ingest {
    /// The disk limits this daemon runs with: (`max_disk_bytes`, `min_free_bytes`).
    pub(crate) fn disk_limits(&self) -> (u64, u64) {
        let gate = &self.ctx.disk;
        (gate.max_disk_bytes, gate.min_free_bytes)
    }

    /// Records a measurement: `Some(why)` while the budget is spent, `None` once
    /// there is room again. Batches are refused from the next admission.
    pub(crate) fn set_disk(&self, spent: Option<String>) {
        let gate = &self.ctx.disk;
        let mut why = gate.why.lock().unwrap_or_else(PoisonError::into_inner);
        gate.spent.store(spent.is_some(), Ordering::Release);
        *why = spent.unwrap_or_default();
    }

    /// The refusal for every batch while the budget is spent.
    pub(super) fn disk_refusal(&self) -> Option<Admission> {
        let gate = &self.ctx.disk;
        if !gate.spent.load(Ordering::Acquire) {
            return None;
        }
        let why = gate.why.lock().unwrap_or_else(PoisonError::into_inner);
        let message = format!("the disk budget is spent: {why}");
        let decision = Decision::reject(Reason::StorageUnavailable, Some(DISK_DELAY), message);
        Some(Admission::rejected(decision))
    }
}
