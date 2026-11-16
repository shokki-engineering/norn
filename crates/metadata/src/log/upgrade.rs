//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! Raising a log's version for a record that needs a newer one.

use std::fs::OpenOptions;
use std::io::Write;

use super::{Log, VERSION, io, replay};
use crate::error::MetadataError;
use crate::op::Op;

/// The lowest version a log holding `ops` can be: never below [`VERSION`].
pub(super) fn needed(ops: &[Op]) -> u32 {
    ops.iter().map(Op::min_version).fold(VERSION, u32::max)
}

impl Log {
    /// Before `ops` are appended: raises the header to the version they need,
    /// and fsyncs it, so no record ever sits behind a header too old for it.
    /// Through a second handle, so the appending one keeps its position. Once
    /// per log at most; nothing when the header already suffices.
    pub(super) fn raise_for(&mut self, ops: &[Op]) -> Result<(), MetadataError> {
        let need = needed(ops);
        if need <= self.version {
            return Ok(());
        }
        let mut head = OpenOptions::new()
            .write(true)
            .open(&self.path)
            .map_err(io("open", &self.path))?;
        // 16 bytes in the first sector, at offset 0: written whole or not at all.
        head.write_all(&replay::header_of(need))
            .map_err(io("write", &self.path))?;
        head.sync_all().map_err(io("fsync", &self.path))?;
        self.version = need;
        Ok(())
    }
}
