//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! Measuring the daemon-wide disk budget (security review L1).
//!
//! Every [`DISK_CHECK`] the worker sums the bytes every stream's segments take
//! and asks the OS how much of the data directory's volume is free, then tells
//! admission whether to refuse. It runs with background maintenance off too: it
//! is protection, not upkeep. Nothing here deletes anything; a stream's retention
//! policy is what makes room by itself.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::Maintenance;

/// How often the budget is measured. Picked: writes between two checks are what
/// can overshoot a limit, and `min_free_bytes` is the headroom that absorbs them.
pub(super) const DISK_CHECK: Duration = Duration::from_secs(1);

/// What a measurement concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Level {
    Room,
    /// Within an eighth of a limit: logged, nothing refused.
    Low(String),
    /// At or past a limit: every batch refused.
    Spent(String),
}

impl Level {
    fn rank(&self) -> u8 {
        match self {
            Self::Room => 0,
            Self::Low(_) => 1,
            Self::Spent(_) => 2,
        }
    }
}

/// Judges `used` segment bytes and `free` volume bytes (unknown: `None`) against
/// `max` and `min`, each zero for off. The worse of the two limits wins.
pub(super) fn judge(used: u64, free: Option<u64>, max: u64, min: u64) -> Level {
    let mut level = Level::Room;
    if max > 0 {
        let what = format!("stored samples take {used} bytes of max_disk_bytes = {max}");
        if used >= max {
            return Level::Spent(what);
        }
        if used >= max - max / 8 {
            level = Level::Low(what);
        }
    }
    if let (Some(free), true) = (free, min > 0) {
        let what = format!("{free} bytes free on the data volume, min_free_bytes = {min}");
        if free <= min {
            return Level::Spent(what);
        }
        if free <= min.saturating_add(min / 8) {
            level = Level::Low(what);
        }
    }
    level
}

impl Maintenance {
    /// Measures the disk budget on the blocking pool: asking the OS for free
    /// space is a system call that a slow volume can hold up.
    pub(super) async fn run_disk_check(self: &Arc<Self>) {
        let worker = Arc::clone(self);
        if tokio::task::spawn_blocking(move || worker.check_disk())
            .await
            .is_err()
        {
            tracing::error!("a disk budget check panicked");
        }
    }

    /// Measures, tells admission, and logs a change of level.
    fn check_disk(&self) {
        let (max, min) = self.ingest.disk_limits();
        if max == 0 && min == 0 {
            return;
        }
        let used: u64 = (self.registry.view().stream_ids().into_iter())
            .map(|id| self.ingest.stats(id).stored_bytes)
            .sum();
        let free = match fs4::available_space(self.ingest.root()) {
            Ok(free) => Some(free),
            Err(e) => {
                tracing::warn!(error = %e, "cannot read the data volume's free space");
                None
            }
        };
        let level = judge(used, free, max, min);
        let was = self.disk_level.swap(level.rank(), Ordering::AcqRel);
        match &level {
            Level::Spent(why) => self.ingest.set_disk(Some(why.clone())),
            _ => self.ingest.set_disk(None),
        }
        if was == level.rank() {
            return;
        }
        match level {
            Level::Room => tracing::info!(used, free, "disk budget has room again"),
            Level::Low(why) => tracing::warn!(%why, "disk budget is nearly spent"),
            Level::Spent(why) => tracing::error!(%why, "disk budget spent: refusing every batch"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Level, judge};

    #[test]
    fn a_limit_of_zero_is_off() {
        assert_eq!(judge(u64::MAX, Some(0), 0, 0), Level::Room);
    }

    #[test]
    fn segments_are_low_within_an_eighth_and_spent_at_the_limit() {
        assert_eq!(judge(874, None, 1000, 0), Level::Room);
        assert!(matches!(judge(875, None, 1000, 0), Level::Low(_)));
        assert!(matches!(judge(1000, None, 1000, 0), Level::Spent(_)));
    }

    #[test]
    fn free_space_is_low_within_an_eighth_and_spent_at_the_floor() {
        assert_eq!(judge(0, Some(1126), 0, 1000), Level::Room);
        assert!(matches!(judge(0, Some(1125), 0, 1000), Level::Low(_)));
        assert!(matches!(judge(0, Some(1000), 0, 1000), Level::Spent(_)));
    }

    #[test]
    fn unknown_free_space_spends_nothing_and_the_worse_limit_wins() {
        assert_eq!(judge(0, None, 0, 1000), Level::Room);
        assert!(matches!(judge(900, Some(0), 1000, 1000), Level::Spent(_)));
        assert!(matches!(
            judge(1000, Some(5000), 1000, 1000),
            Level::Spent(_)
        ));
    }
}
