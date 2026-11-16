//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! Why startup recovery failed.

use std::fmt;

use norn_index::IndexError;
use norn_storage::StorageError;

/// Why startup recovery failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum RecoverError {
    /// A stream could not be assessed.
    Storage(StorageError),
    /// The index could not open.
    Index(IndexError),
}

impl From<StorageError> for RecoverError {
    fn from(e: StorageError) -> Self {
        Self::Storage(e)
    }
}

impl From<IndexError> for RecoverError {
    fn from(e: IndexError) -> Self {
        Self::Index(e)
    }
}

impl fmt::Display for RecoverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(e) => write!(f, "{e}"),
            Self::Index(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RecoverError {}
