//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! The daemon-wide disk budget (security review L1), as admission sees it.
//!
//! Admission never touches a disk, so it cannot measure one: maintenance
//! measures, and leaves here whether the budget is spent, why, and how many bytes
//! the free-space floor left room for. Between two measurements admission counts
//! what it admits against that room, so Norn's own writes, however fast, cannot run
//! past the floor to the disk filling. A batch is counted at its in-memory size,
//! more than the journal writes for it, so the count errs towards refusing early;
//! it restarts at every measurement. Another program's writes, and
//! `max_disk_bytes`, are caught by the measurements alone.
//! Before the first measurement the budget counts as unspent and unbounded.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    /// Bytes the last measurement left before a limit; `u64::MAX` for no bound.
    room: AtomicU64,
    /// That limit's setting, for a refusal's message.
    room_limit: Mutex<&'static str>,
    /// Bytes admitted since the last measurement.
    admitted: AtomicU64,
}

impl DiskGate {
    pub(crate) fn new(max_disk_bytes: u64, min_free_bytes: u64) -> Self {
        Self {
            max_disk_bytes,
            min_free_bytes,
            room: AtomicU64::new(u64::MAX),
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
    /// there is room again, and the room it leaves (`u64::MAX` for no bound) with
    /// the setting that bounds it.
    /// The count of admitted bytes starts again from zero. A batch admitted between
    /// the measurement and this call goes uncounted; the next measurement sees it.
    pub(crate) fn set_disk(&self, spent: Option<String>, (room, limit): (u64, &'static str)) {
        let gate = &self.ctx.disk;
        let mut why = gate.why.lock().unwrap_or_else(PoisonError::into_inner);
        *gate
            .room_limit
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = limit;
        gate.room.store(room, Ordering::Release);
        gate.admitted.store(0, Ordering::Release);
        gate.spent.store(spent.is_some(), Ordering::Release);
        *why = spent.unwrap_or_default();
    }

    /// The refusal for a batch of `cost` bytes: every batch while the budget is
    /// spent, and one that would pass the room left since the last measurement.
    /// Counts an admitted batch's bytes, so call it once, last before admitting.
    /// Performance: one atomic add while unspent; the lock only to refuse.
    pub(super) fn disk_refusal(&self, cost: u64) -> Option<Admission> {
        let gate = &self.ctx.disk;
        let why = if gate.spent.load(Ordering::Acquire) {
            gate.why
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        } else {
            let room = gate.room.load(Ordering::Acquire);
            if room == u64::MAX {
                return None;
            }
            let before = gate.admitted.fetch_add(cost, Ordering::AcqRel);
            if before.saturating_add(cost) <= room {
                return None;
            }
            gate.admitted.fetch_sub(cost, Ordering::AcqRel);
            let limit = *gate
                .room_limit
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            format!("writes since the last measurement used the {room} bytes {limit} left")
        };
        let message = format!("the disk budget is spent: {why}");
        let decision = Decision::reject(Reason::StorageUnavailable, Some(DISK_DELAY), message);
        Some(Admission::rejected(decision))
    }
}
