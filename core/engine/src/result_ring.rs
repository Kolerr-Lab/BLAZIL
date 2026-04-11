//! Zero-contention SPSC result ring for the pipeline hot path.
//!
//! # Design v2 — AtomicU8 status + raw TransferId storage
//!
//! ## Problem with v1 (AtomicBool + MaybeUninit<TransactionResult>)
//!
//! v1 stored the full `TransactionResult` enum (~48 bytes) per slot:
//! 262 144 slots × 48 bytes ≈ **12 MB** — spills out of per-core L2 cache
//! (typically 256 KB – 1 MB) and into shared L3.  Every drain loop iteration
//! that checks an unready slot still brings a 48-byte cache line into L2, even
//! though the useful information is just 1 bit.
//!
//! Measured: `stall_ms` 128 ms; TPS 35 K with v1 at window=131 072.
//!
//! ## Solution
//!
//! Two separate arrays:
//!
//! | Array | Element | Count | Size |
//! |-------|---------|-------|------|
//! | `status` | `AtomicU8` | 262 144 | **256 KB** — fits in L2 |
//! | `transfer_ids` | `UnsafeCell<MaybeUninit<[u8; 16]>>` | 262 144 | 4 MB — L3 |
//!
//! The drain loop's hot check only touches `status[]` (256 KB, one byte per
//! slot).  Once a slot is ready (`status[idx] == COMMITTED`), the 16-byte
//! transfer-ID is read from `transfer_ids[idx]` — a cold path that fires at
//! most once per event.
//!
//! Rejected results (rare at high load — from validation / risk / TB errors)
//! bypass the ring entirely and go to the `DashMap` fallback so no rejection
//! storage is needed here.
//!
//! ## Protocol (SPSC per slot)
//!
//! | Side | Action |
//! |------|--------|
//! | Writer | write `transfer_ids[idx]`, then `status[idx].store(COMMITTED, Release)` |
//! | Reader | `if status[idx].load(Acquire) == COMMITTED { read id; status.store(PENDING, Release) }` |
//!
//! ## Alias-freedom invariant
//!
//! `cap ≥ pipeline_capacity` (262 144 ≥ ring-buffer capacity) ensures no two
//! in-flight sequence numbers alias the same slot.

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicU8, Ordering};

use blazil_common::ids::TransferId;

// ── Status constants ──────────────────────────────────────────────────────────

/// Slot is empty / has been consumed by the serve thread.
const PENDING: u8 = 0;
/// Slot contains a committed TransferId written by an async TB task.
const COMMITTED: u8 = 1;

// ── ResultRing ────────────────────────────────────────────────────────────────

/// Lock-free O(1) committed-result store for the pipeline hot path.
///
/// Replaces `DashMap<i64, TransactionResult>` for the TB-committed code path.
/// Rejected results continue to use the small `DashMap` fallback.
///
/// # Memory layout
///
/// ```text
/// status[]        : [AtomicU8; 262_144] = 256 KB  ← drain hot-check (L2)
/// transfer_ids[]  : [[u8;16]; 262_144]  =   4 MB  ← cold read on hit (L3)
/// ```
pub struct ResultRing {
    /// One byte per slot: `PENDING` (0) or `COMMITTED` (1).
    status: Vec<AtomicU8>,
    /// Raw UUID bytes for the committed TransferId.
    /// Protected by `status`: writer stores bytes then sets status=COMMITTED
    /// with Release; reader loads status with Acquire before reading bytes.
    transfer_ids: Vec<UnsafeCell<MaybeUninit<[u8; 16]>>>,
    mask: usize,
}

// SAFETY: Each slot has a single writer (one async task) and a single reader
// (the serve thread).  The Release/Acquire on `status` provides the required
// happens-before edge.  No two threads access the same slot concurrently.
unsafe impl Send for ResultRing {}
unsafe impl Sync for ResultRing {}

impl ResultRing {
    /// Create a new ring with `cap` slots.
    ///
    /// `cap` must be a power of two and `≥ pipeline_capacity`.
    pub fn new(cap: usize) -> Self {
        assert!(
            cap.is_power_of_two(),
            "ResultRing cap must be a power of two"
        );
        Self {
            status: (0..cap).map(|_| AtomicU8::new(PENDING)).collect(),
            transfer_ids: (0..cap)
                .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
                .collect(),
            mask: cap - 1,
        }
    }

    /// Record a committed result for `seq`.
    ///
    /// Only called for committed outcomes — rejections go to the `DashMap`.
    /// Called from async TigerBeetle task threads.
    #[inline(always)]
    pub fn insert(&self, seq: i64, transfer_id: TransferId) {
        let idx = (seq as usize) & self.mask;
        // Write bytes BEFORE the Release store.
        // SAFETY: slot is empty (status=PENDING) — alias-freedom invariant.
        unsafe {
            (*self.transfer_ids[idx].get()).write(*transfer_id.as_uuid().as_bytes());
        }
        self.status[idx].store(COMMITTED, Ordering::Release);
    }

    /// Consume and return the `TransferId` for `seq` if committed, or `None`.
    ///
    /// Called only from the single serve thread.
    #[inline(always)]
    pub fn try_remove(&self, seq: i64) -> Option<TransferId> {
        let idx = (seq as usize) & self.mask;
        if self.status[idx].load(Ordering::Acquire) == COMMITTED {
            // SAFETY: Acquire pairs with insert's Release; bytes are visible.
            let bytes = unsafe { (*self.transfer_ids[idx].get()).assume_init_read() };
            self.status[idx].store(PENDING, Ordering::Release);
            Some(TransferId::from_bytes(bytes))
        } else {
            None
        }
    }

    /// Returns `true` if a committed result for `seq` is ready.
    /// Non-destructive — safe for diagnostics.
    #[inline(always)]
    pub fn contains(&self, seq: i64) -> bool {
        let idx = (seq as usize) & self.mask;
        self.status[idx].load(Ordering::Acquire) == COMMITTED
    }
}

impl Drop for ResultRing {
    fn drop(&mut self) {
        // AtomicU8 and [u8;16] have no destructors.
        // Vec handles the heap allocations.
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_then_try_remove_returns_transfer_id() {
        let ring = ResultRing::new(64);
        let tid = TransferId::new();
        ring.insert(0, tid);
        let got = ring.try_remove(0).expect("should be ready");
        assert_eq!(got.as_uuid(), tid.as_uuid());
    }

    #[test]
    fn try_remove_before_insert_returns_none() {
        let ring = ResultRing::new(64);
        assert!(ring.try_remove(0).is_none());
    }

    #[test]
    fn slot_is_reusable_after_remove() {
        let ring = ResultRing::new(64);
        let tid = TransferId::new();
        ring.insert(0, tid);
        ring.try_remove(0);
        let tid2 = TransferId::new();
        ring.insert(64, tid2);
        let got = ring.try_remove(64).expect("should be ready after reuse");
        assert_eq!(got.as_uuid(), tid2.as_uuid());
    }

    #[test]
    fn contains_reflects_insert_state() {
        let ring = ResultRing::new(64);
        assert!(!ring.contains(0));
        ring.insert(0, TransferId::new());
        assert!(ring.contains(0));
        ring.try_remove(0);
        assert!(!ring.contains(0));
    }

    #[test]
    fn sequential_sequences_all_drain() {
        let ring = ResultRing::new(128);
        let tids: Vec<_> = (0..64).map(|_| TransferId::new()).collect();
        for (seq, tid) in tids.iter().enumerate() {
            ring.insert(seq as i64, *tid);
        }
        for (seq, tid) in tids.iter().enumerate() {
            let got = ring.try_remove(seq as i64).expect("should be ready");
            assert_eq!(got.as_uuid(), tid.as_uuid());
        }
    }

    #[test]
    fn non_power_of_two_panics() {
        let result = std::panic::catch_unwind(|| ResultRing::new(100));
        assert!(result.is_err());
    }
}
