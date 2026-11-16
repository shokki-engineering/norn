//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! Running a bound daemon: its tasks, until cancelled, then the drain.

use std::sync::Arc;

use norn_runtime::RunReport;
use tokio_util::sync::CancellationToken;

use super::{Daemon, DaemonError, tasks};
use crate::health::reflect;

impl Daemon {
    /// Serves until `cancel` is cancelled, then drains and stops.
    ///
    /// # Errors
    ///
    /// [`DaemonError::Runtime`] when a task fails fatally (the IPC server
    /// erroring, or its drain getting stuck), after every task has stopped.
    pub async fn run(self, cancel: CancellationToken) -> Result<RunReport, DaemonError> {
        let Self {
            runtime,
            server,
            endpoint: _,
            remote: _,
            opening,
            ended,
            reporter,
            ingest,
            recovery: _,
            replays,
            maintenance,
            calls,
            _windows,
        } = self;
        // Nothing has been answered until now; every task below is added first.
        let watch = runtime.health();
        let stop = cancel.clone();
        runtime.add_task("mcp-stdio", move |token| {
            tasks::stop_when(ended, stop, token)
        })?;
        runtime.add_task("health", move |token| reflect(watch, reporter, token))?;
        let draining = Arc::clone(&ingest);
        runtime.add_task("ingest", move |token| tasks::drain_ingest(draining, token))?;
        // Expires idle sessions; on shutdown ends them all and joins their tasks.
        runtime.add_task("replay", move |token| async move {
            replays.run(token).await;
            Ok(())
        })?;
        // Cancelled by dropping the guard, so a panicking worker cannot hang
        // shutdown behind it.
        let quiet = CancellationToken::new();
        let done = quiet.clone().drop_guard();
        runtime.add_task("maintenance", move |token| async move {
            let _done = done;
            maintenance.run(token).await;
            Ok(())
        })?;
        runtime.add_task("ipc", move |token| async move {
            let _ = opening.send(true);
            token.cancelled().await;
            tasks::stop_serving(server, calls, quiet, ingest).await
        })?;
        Ok(runtime.run(cancel).await?)
    }
}
