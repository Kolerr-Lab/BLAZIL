// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Low-level I/O readers for efficient data loading.
//!
//! Two paths depending on OS:
//!
//! - **Linux (production)**: [`IoUringReader`] — submits batched `read_fixed`
//!   requests via io_uring for maximum NVMe throughput.
//! - **All platforms (dev + fallback)**: [`MmapReader`] — memory-mapped reads,
//!   OS manages page-cache warming.
//!
//! Both readers expose the same [`FileReader`] trait so the caller is agnostic.

pub mod mmap;

#[cfg(target_os = "linux")]
pub mod io_uring;

pub use mmap::MmapReader;

#[cfg(target_os = "linux")]
pub use io_uring::IoUringReader;

use crate::Result;
use std::path::Path;

/// Synchronous file reader abstraction.
///
/// All implementations must be `Send + Sync` so they can be used across
/// `spawn_blocking` workers.
pub trait FileReader: Send + Sync {
    /// Read the entire contents of `path` into a `Vec<u8>`.
    fn read(&self, path: &Path) -> Result<Vec<u8>>;
}
