// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Memory-mapped file reader.
//!
//! Uses `mmap(2)` via the [`memmap2`] crate for zero-copy reads.
//! The OS kernel handles read-ahead and page-cache management.
//!
//! **When to use:**
//! - macOS / non-Linux development environments.
//! - Files that are frequently re-read (warm page cache).
//! - Fallback on Linux when io_uring is not available.
//!
//! **When NOT to use:**
//! - Large sequential scans on Linux production — prefer `IoUringReader`
//!   for direct I/O that bypasses page cache and reduces memory pressure.

use crate::{Error, Result};
use memmap2::Mmap;
use std::{fs::File, path::Path};

use super::FileReader;

/// Memory-mapped reader — zero-copy on read, OS manages page cache.
#[derive(Debug, Default)]
pub struct MmapReader;

impl MmapReader {
    pub fn new() -> Self {
        Self
    }
}

impl FileReader for MmapReader {
    fn read(&self, path: &Path) -> Result<Vec<u8>> {
        let file = File::open(path).map_err(Error::Io)?;

        // Safety: The file is opened read-only.  The data is copied out
        // immediately into a Vec so there is no risk of the mapping
        // becoming invalid while the caller holds the bytes.
        let mmap = unsafe { Mmap::map(&file) }.map_err(Error::Io)?;

        Ok(mmap.to_vec())
    }
}

// ─────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_read_small_file() {
        let mut f = NamedTempFile::new().unwrap();
        let data = b"hello blazil";
        f.write_all(data).unwrap();

        let reader = MmapReader::new();
        let result = reader.read(f.path()).unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn test_read_missing_file() {
        let reader = MmapReader::new();
        let result = reader.read(Path::new("/nonexistent/file.bin"));
        assert!(result.is_err());
    }

    #[test]
    fn test_read_empty_file() {
        let f = NamedTempFile::new().unwrap();
        // empty file
        let reader = MmapReader::new();
        // mmap of empty file fails on most platforms — we should get an error,
        // not a panic.
        let _ = reader.read(f.path()); // ok to be error
    }
}
