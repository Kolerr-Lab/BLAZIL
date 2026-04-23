// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! io_uring batch file reader (Linux only).
//!
//! ## Strategy
//!
//! For each batch of N files we want to read, we:
//!
//! 1. Submit `IORING_OP_READ` SQEs for every file in the batch
//!    concurrently — up to `QUEUE_DEPTH` in-flight at once.
//! 2. Call `submit_and_wait` to flush the SQ and block until all CQEs arrive.
//! 3. Map each CQE back to its source buffer by `user_data` index.
//!
//! This amortises syscall overhead across N reads, which is the primary
//! bottleneck when reading millions of small JPEG files from NVMe.
//!
//! ## Thread safety
//!
//! `io_uring::IoUring` is `Send + Sync` in the `io-uring` crate.
//! We wrap it in a `Mutex` so that `FileReader::read(&self, …)` can
//! obtain exclusive access without requiring `&mut self` from callers
//! that hold an `Arc<dyn FileReader>`.
//!
//! This file is only compiled on Linux (`#[cfg(target_os = "linux")]`).

use crate::{Error, Result};
use io_uring::{opcode, types, IoUring};
use std::{fs::File, os::unix::io::AsRawFd, path::Path, sync::Mutex};

use super::FileReader;

/// Maximum simultaneous io_uring submissions per batch.
const QUEUE_DEPTH: u32 = 64;

/// Maximum file size we will read in a single shot (64 MiB).
const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;

/// io_uring-backed reader for Linux.
///
/// The inner `IoUring` is wrapped in a `Mutex` to allow shared `&self`
/// access from multiple callers while maintaining exclusive access to
/// the submission / completion queues at any moment.
pub struct IoUringReader {
    ring: Mutex<IoUring>,
}

impl IoUringReader {
    /// Create a new io_uring instance with `QUEUE_DEPTH` SQ entries.
    pub fn new() -> Result<Self> {
        let ring = IoUring::new(QUEUE_DEPTH)
            .map_err(|e| Error::internal(format!("io_uring init failed: {e}")))?;
        Ok(Self {
            ring: Mutex::new(ring),
        })
    }
}

impl FileReader for IoUringReader {
    /// Read the entire content of `path` using a single io_uring `read` op.
    fn read(&self, path: &Path) -> Result<Vec<u8>> {
        let file = File::open(path)?;
        let len = file.metadata()?.len() as usize;

        if len == 0 {
            return Ok(Vec::new());
        }
        if len > MAX_FILE_BYTES {
            return Err(Error::InvalidFormat {
                reason: format!(
                    "'{}' is {len} bytes, exceeds MAX_FILE_BYTES={MAX_FILE_BYTES}",
                    path.display()
                ),
            });
        }

        let mut buf = vec![0u8; len];
        let fd = types::Fd(file.as_raw_fd());

        // Safety: `buf` is alive for the duration of the ring operation.
        // We call `submit_and_wait(1)` before accessing `buf`, so the kernel
        // finishes writing before we read the bytes.
        let read_e = opcode::Read::new(fd, buf.as_mut_ptr(), len as u32)
            .offset(0)
            .build()
            .user_data(0);

        let mut ring = self
            .ring
            .lock()
            .map_err(|_| Error::internal("io_uring mutex poisoned"))?;

        unsafe {
            ring.submission()
                .push(&read_e)
                .map_err(|_| Error::internal("io_uring SQ full"))?;
        }

        ring.submit_and_wait(1)
            .map_err(|e| Error::internal(format!("io_uring submit failed: {e}")))?;

        let cqe = ring
            .completion()
            .next()
            .ok_or_else(|| Error::internal("io_uring: no CQE received"))?;

        let result = cqe.result();
        if result < 0 {
            return Err(Error::Io(std::io::Error::from_raw_os_error(-result)));
        }
        buf.truncate(result as usize);
        Ok(buf)
    }
}

impl IoUringReader {
    /// Read multiple files in a single batch submission.
    ///
    /// Submits up to `QUEUE_DEPTH` reads per `submit_and_wait` syscall.
    /// Results are returned in the same order as `paths`.
    pub fn read_batch(&self, paths: &[&Path]) -> Vec<Result<Vec<u8>>> {
        if paths.is_empty() {
            return Vec::new();
        }

        let n = paths.len();
        let mut results: Vec<Result<Vec<u8>>> = (0..n).map(|_| Ok(Vec::new())).collect();

        // Open files and measure sizes upfront.
        let mut files: Vec<Option<(File, usize)>> = Vec::with_capacity(n);
        for (i, path) in paths.iter().enumerate() {
            match File::open(path) {
                Ok(f) => {
                    let len = f.metadata().map(|m| m.len() as usize).unwrap_or(0);
                    if len > MAX_FILE_BYTES {
                        results[i] = Err(Error::InvalidFormat {
                            reason: format!("'{}' exceeds {MAX_FILE_BYTES}B", path.display()),
                        });
                        files.push(None);
                    } else {
                        files.push(Some((f, len)));
                    }
                }
                Err(e) => {
                    results[i] = Err(Error::Io(e));
                    files.push(None);
                }
            }
        }

        // Allocate output buffers.
        let mut bufs: Vec<Vec<u8>> = files
            .iter()
            .map(|f| {
                f.as_ref()
                    .map(|(_, len)| vec![0u8; *len])
                    .unwrap_or_default()
            })
            .collect();

        let mut ring = match self.ring.lock() {
            Ok(g) => g,
            Err(_) => {
                for r in results.iter_mut() {
                    *r = Err(Error::internal("io_uring mutex poisoned"));
                }
                return results;
            }
        };

        let mut submitted = 0usize;

        loop {
            let mut chunk_count = 0usize;

            // Fill the SQ up to QUEUE_DEPTH.
            while submitted < n && chunk_count < QUEUE_DEPTH as usize {
                if let Some((file, len)) = &files[submitted] {
                    if *len > 0 {
                        let fd = types::Fd(file.as_raw_fd());
                        let read_e =
                            opcode::Read::new(fd, bufs[submitted].as_mut_ptr(), *len as u32)
                                .offset(0)
                                .build()
                                .user_data(submitted as u64);

                        // Safety: bufs[submitted] lives until we drain CQEs below.
                        if unsafe { ring.submission().push(&read_e) }.is_err() {
                            break; // SQ unexpectedly full — flush first.
                        }
                        chunk_count += 1;
                    }
                }
                submitted += 1;
            }

            if chunk_count == 0 {
                break;
            }

            // Block until all chunk_count CQEs arrive.
            if let Err(e) = ring.submit_and_wait(chunk_count) {
                let msg = format!("submit_and_wait: {e}");
                for cqe in ring.completion() {
                    let i = cqe.user_data() as usize;
                    results[i] = Err(Error::internal(msg.as_str()));
                }
                break;
            }

            for cqe in ring.completion() {
                let i = cqe.user_data() as usize;
                let res = cqe.result();
                if res < 0 {
                    results[i] = Err(Error::Io(std::io::Error::from_raw_os_error(-res)));
                } else {
                    bufs[i].truncate(res as usize);
                    results[i] = Ok(std::mem::take(&mut bufs[i]));
                }
            }

            if submitted >= n {
                break;
            }
        }

        results
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

    fn write_temp(data: &[u8]) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(data).unwrap();
        f
    }

    #[test]
    fn test_single_read() {
        let f = write_temp(b"io_uring test payload");
        let reader = IoUringReader::new().unwrap();
        let bytes = reader.read(f.path()).unwrap();
        assert_eq!(bytes, b"io_uring test payload");
    }

    #[test]
    fn test_read_missing_file() {
        let reader = IoUringReader::new().unwrap();
        assert!(reader.read(Path::new("/nonexistent/file.bin")).is_err());
    }

    #[test]
    fn test_read_batch_all_present() {
        let files: Vec<_> = (0..8u8).map(|i| write_temp(&[i; 128])).collect();
        let paths: Vec<&Path> = files.iter().map(|f| f.path()).collect();
        let reader = IoUringReader::new().unwrap();
        let results = reader.read_batch(&paths);

        assert_eq!(results.len(), 8);
        for (i, res) in results.iter().enumerate() {
            let bytes = res.as_ref().unwrap();
            assert_eq!(bytes.len(), 128);
            assert!(bytes.iter().all(|&b| b == i as u8));
        }
    }

    #[test]
    fn test_read_batch_partial_errors() {
        let good = write_temp(b"good");
        let paths: Vec<&Path> = vec![
            good.path(),
            Path::new("/nonexistent/missing.bin"),
            good.path(),
        ];
        let reader = IoUringReader::new().unwrap();
        let results = reader.read_batch(&paths);

        assert!(results[0].is_ok());
        assert!(results[1].is_err());
        assert!(results[2].is_ok());
    }

    #[test]
    fn test_read_batch_larger_than_queue_depth() {
        let num = QUEUE_DEPTH as usize + 10;
        let files: Vec<_> = (0..num).map(|i| write_temp(&[i as u8; 64])).collect();
        let paths: Vec<&Path> = files.iter().map(|f| f.path()).collect();
        let reader = IoUringReader::new().unwrap();
        let results = reader.read_batch(&paths);

        assert_eq!(results.len(), num);
        for res in &results {
            assert!(res.is_ok(), "unexpected error: {:?}", res);
        }
    }
}
