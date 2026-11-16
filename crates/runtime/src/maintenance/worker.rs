//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! The worker's loop: client jobs in order, and passes on the schedule.

use std::sync::{Arc, PoisonError};
use std::time::Duration;

use norn_core::Timestamp;
use norn_metadata::Retention;
use tokio_util::sync::CancellationToken;

use super::Maintenance;
use super::disk::DISK_CHECK;
use super::months;
use super::ops::{Failure, Operation, Outcome, Request};

impl Maintenance {
    /// Runs jobs, scheduled passes and the check for segments open too long
    /// until `token` is cancelled, then ends every queued job as cancelled and
    /// returns. The job in progress stops as
    /// [`Operation::cancel`] says.
    pub async fn run(self: Arc<Self>, token: CancellationToken) {
        let taken = self
            .rx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(mut rx) = taken else { return };
        let c = self.config;
        let compaction = c.compaction_interval;
        let retention = c.retention_interval;
        // Sealing by age runs whether or not background passes do: it is not
        // upkeep a client could ask for, and without it a quiet stream never
        // reaches a sealed segment at all.
        let sealing = self.ingest.seal_check_interval();
        let (mut compact_in, mut retain_in, mut seal_in) = (compaction, retention, sealing);
        let mut disk_in = Duration::ZERO;
        let mut last = self.clock.now();
        loop {
            // The disk budget is measured whether or not background passes run.
            if disk_in.is_zero() {
                disk_in = DISK_CHECK;
                self.run_disk_check().await;
            }
            let upkeep = if c.background {
                compact_in.min(retain_in)
            } else {
                sealing
            };
            let wait = upkeep.min(seal_in).min(disk_in);
            tokio::select! {
                () = token.cancelled() => break,
                op = rx.recv() => match op {
                    Some(op) => self.run_job(op, &token).await,
                    None => break,
                },
                () = self.ingest.sealed_wake(), if c.background => {
                    self.run_size_limits(&token).await;
                },
                () = self.clock.sleep_until(after(last, wait)) => {}
            }
            // Measured, not assumed: a job or a seal ends the wait early, and the
            // schedule must still advance, or frequent seals would starve it.
            let now = self.clock.now();
            let waited = since(last, now);
            last = now;
            seal_in = seal_in.saturating_sub(waited);
            disk_in = disk_in.saturating_sub(waited);
            if seal_in.is_zero() {
                seal_in = sealing;
                let asked = self.ingest.seal_aged(now);
                if asked > 0 {
                    tracing::debug!(streams = asked, "sealing segments by age");
                }
            }
            if c.background {
                compact_in = compact_in.saturating_sub(waited);
                retain_in = retain_in.saturating_sub(waited);
                let (compact, retain) = (compact_in.is_zero(), retain_in.is_zero());
                if compact {
                    compact_in = compaction;
                }
                if retain {
                    retain_in = retention;
                }
                if compact || retain {
                    self.run_pass(compact, retain, &token).await;
                }
            }
        }
        rx.close();
        while let Ok(op) = rx.try_recv() {
            op.finish(self.clock.now(), Err(Failure::Cancelled));
        }
    }

    async fn run_job(self: &Arc<Self>, op: Arc<Operation>, token: &CancellationToken) {
        let worker = Arc::clone(self);
        let cancel = op.cancel.clone();
        let stop = token.clone();
        // Shutdown stops the job the way a client's cancel would.
        let relay = stop.child_token();
        let watch = {
            let (relay, cancel) = (relay.clone(), cancel.clone());
            tokio::spawn(async move {
                relay.cancelled().await;
                cancel.cancel();
            })
        };
        let job = Arc::clone(&op);
        let done = tokio::task::spawn_blocking(move || worker.execute(&job, &cancel)).await;
        watch.abort();
        let result = done.unwrap_or_else(|_| Err(Failure::Failed("the job panicked".to_owned())));
        op.finish(self.clock.now(), result);
    }

    fn execute(&self, op: &Operation, cancel: &CancellationToken) -> Result<Outcome, Failure> {
        let stream = op.stream;
        match &op.request {
            Request::Compact { interval } => self
                .compact_stream(stream, *interval, Some(op), cancel, usize::MAX)
                .map(Outcome::Compacted),
            Request::Verify => self
                .verify_stream(stream, op, cancel)
                .map(Outcome::Verified),
            Request::Retention { dry_run } => self
                .apply_retention(stream, *dry_run)
                .map(Outcome::Retained),
            Request::Backup(spec) => self.backup(spec, op, cancel).map(Outcome::BackedUp),
        }
    }

    /// Applies the size limit of each stream that sealed since the last look.
    /// Other limits wait for the schedule: only size grows with every seal.
    async fn run_size_limits(self: &Arc<Self>, token: &CancellationToken) {
        let worker = Arc::clone(self);
        let cancel = token.clone();
        let done = tokio::task::spawn_blocking(move || {
            for id in worker.ingest.take_sealed() {
                if cancel.is_cancelled() {
                    return;
                }
                if matches!(worker.policy(id), Some(Retention::MaxBytes(_))) {
                    let _ = worker.apply_retention(id, false);
                }
            }
        });
        if done.await.is_err() {
            tracing::error!("a size-limit pass panicked");
        }
    }

    async fn run_pass(self: &Arc<Self>, compact: bool, retain: bool, token: &CancellationToken) {
        let worker = Arc::clone(self);
        let cancel = token.clone();
        let done = tokio::task::spawn_blocking(move || worker.pass(compact, retain, &cancel));
        if done.await.is_err() {
            tracing::error!("a maintenance pass panicked");
        }
    }

    fn pass(&self, compact: bool, retain: bool, cancel: &CancellationToken) {
        let ids = self.registry.view().stream_ids();
        let minute = months::minute_of_day(self.clock.now());
        let compact = compact && self.config.in_window(minute);
        let max = self.config.max_runs_per_pass;
        for id in ids {
            if cancel.is_cancelled() {
                return;
            }
            if retain {
                let _ = self.apply_retention(id, false);
            }
            if compact {
                let _ = self.compact_stream(id, None, None, cancel, max);
            }
        }
    }
}

/// `wait` after `from`, or the furthest instant the clock can name.
fn after(from: Timestamp, wait: Duration) -> Timestamp {
    let nanos = i64::try_from(wait.as_nanos()).unwrap_or(i64::MAX);
    Timestamp::from_nanos(from.as_nanos().saturating_add(nanos))
}

/// Time from `from` to `to`; zero if the clock went back.
fn since(from: Timestamp, to: Timestamp) -> Duration {
    let nanos = to.as_nanos().saturating_sub(from.as_nanos());
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(0))
}
