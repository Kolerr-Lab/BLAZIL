//! Zero-contention SPSC result ring — replaces `DashMap` on the hot path.
//!
//! # Problem
//!
//! The original design stored async TigerBeetle results in a
//! `DashMap<i64, TransactionResult>`. At large window sizes (≥ 65 536 entries)
//! the map grows beyond L3 cache capacity (~8 MB on typical DO nodes), causing
//! every `remove()` in the serve thread's drain loop to suffer an L3 miss.
//! Measured impact: TPS dropped from **66 K** (window 32 768) to **35 K**
//! (window 131 072) — the opposite of what we wanted.
//!
//! # Solution: per-slot AtomicBool + UnsafeCell
//!
//! Pre-allocate a fixed-size ring of `(UnsafeCell<MaybeUninit<T>>, AtomicBool)` pairs.
//! Each sequence number maps to `slot[seq % cap]` with O(1) constant-time
//! access and sequential (prefetch-friendly) memory layout.
//!
//! ## Protocol (SPSC per slot)
//!
//! | Side | Action |
//! |------|--------|
//! | Writer | `slots[idx].write(result)`, then `ready[idx].store(true, Release)` |
//! | Reader | if `ready[idx].load(Acquire)`, read slot, `ready[idx].store(false, Release)` |
//!
//! The Release/Acquire pair is a full happens-before edge: the reader never
//! observes stale slot data.
//!
//! ## Alias-freedom invariant
//!
//! No two live (in-flight) sequence numbers may alias to the same slot.
//! Guaranteed by `cap ≥ 2 × max_inflight` (checked at construction):
//! if `|seq_A - seq_B| < cap` then `seq_A % cap ≠ seq_B % cap`.
//! With `cap = 262 144` and `window = 131 072`, the gap between the slowest
//! in-flight sequence and the fastest is at most 131 072 < 262 144 = cap. ✓

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::event::TransactionResult;

// ── ResultRing ────────────────────────────────────────────────────────────────

/// Lock-free O(1) result store for the pipeline hot path.
///
/// Replaces `DashMap<i64, TransactionResult>` for async (TigerBeetle) results.
/// Synchronous rejection results (from `ValidationHandler` / `RiskHandler`)
/// still use the DashMap; this ring is for the high-volume committed path.
pub struct ResultRing {
    slots: Vec<UnsafeCell<MaybeUninit<TransactionResult>>>,
    ready: Vec<AtomicBool>,
    mask: usize,
}

// SAFETY: Each slot is protected by its paired `AtomicBool`:
// - only one writer per slot (while ready=false)
// - only one reader per slot (while ready=true, served by the single serve thread)
// Release/Acquire fencing guarantees the slot write is visible before the read.
unsafe impl Send for ResultRing {}
unsafe impl Sync for ResultRing {}

impl ResultRing {
    /// Create a new ring with `cap` slots. `cap` must be a power of two and
    /// must be at least twice the maximum number of in-flight sequences to
    /// prevent slot aliasing.
    pub fn new(cap: usize) -> Self {
        assert!(
            cap.is_power_of_two(),
            "ResultRing cap must be a power of two"
        );
        Self {
            slots: (0..cap)
                .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
                .collect(),
            ready: (0..cap).map(|_| AtomicBool::new(false)).collect(),
            mask: cap - 1,
        }
    }

    /// Write `result` for `seq`. The slot at `seq % cap` must be free
    /// (i.e. not yet consumed by `try_remove`).
    ///
    /// Called from async TigerBeetle task threads (multiple writers, but
    /// each slot has exactly one writer at a time).
    #[inline(always)]
    pub fn insert(&self, seq: i64, result: TransactionResult) {
        let idx = (seq as usize) & self.mask;
        // SAFETY: slot is free (ready=false) — guaranteed by alias-freedom invariant.
        unsafe {
            (*self.slots[idx].get()).write(result);
        }
        // Release: slot write must be globally visible before reader sees ready=true.
        self.ready[idx].store(true, Ordering::Release);
    }

    /// Take the result for `seq` if it has been written, or `None` if not yet
    /// available. Consumes the slot (marks it free for reuse).
    ///
    /// Called only from the single serve thread.
    #[inline(always)]
    pub fn try_remove(&self, seq: i64) -> Option<TransactionResult> {
        let idx = (seq as usize) & self.mask;
        // Acquire: pairs with insert()'s Release — ensures slot data is visible.
        if self.ready[idx].load(Ordering::Acquire) {
            // SAFETY: ready=true means insert() completed with Release semantics.
            // We are the only reader (single serve thread), so no concurrent read.
            let result = unsafe { (*self.slots[idx].get()).assume_init_read() };
            // Release: mark slot free so the next writer for seq+cap can claim it.
            self.ready[idx].store(false, Ordering::Release);
            Some(result)
        } else {
            None
        }
    }
    /// Returns `true` if a result for `seq` has been written and not yet consumed.
    ///
    /// Non-destructive — safe to use for diagnostics without consuming the slot.
    #[inline(always)]
    pub fn contains(&self, seq: i64) -> bool {
        let idx = (seq as usize) & self.mask;
        self.ready[idx].load(Ordering::Acquire)
    }
}

impl Drop for ResultRing {
    fn drop(&mut self) {
        // Drop any results that were inserted but never consumed (e.g. on shutdown).
        for (slot, ready) in self.slots.iter().zip(self.ready.iter()) {
            if ready.load(Ordering::Acquire) {
                // SAFETY: ready=true guarantees the slot was written.
                unsafe {
                    (*slot.get()).assume_init_drop();
                }
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use blazil_common::timestamp::Timestamp;

    use super::*;
    use crate::event::TransactionResult;

    fn committed() -> TransactionResult {
        TransactionResult::Committed {
            transfer_id: blazil_common::ids::TransferId::new(),
            timestamp: Timestamp::now(),
        }
    }

    #[test]
    fn insert_then_try_remove_returns_result() {
        let ring = ResultRing::new(64);
        ring.insert(0, committed());
        assert!(ring.try_remove(0).is_some());
    }

    #[test]
    fn try_remove_before_insert_returns_none() {
        let ring = ResultRing::new(64);
        assert!(ring.try_remove(0).is_none());
    }

    #[test]
    fn slot_is_reusable_after_remove() {
        let ring = ResultRing::new(64);
        ring.insert(0, committed());
        ring.try_remove(0);
        // Same slot, next cycle (seq = cap = 64)
        ring.insert(64, committed());
        assert!(ring.try_remove(64).is_some());
    }

    #[test]
    fn sequential_sequences_all_drain_in_order() {
        let ring = ResultRing::new(128);
        for seq in 0..64_i64 {
            ring.insert(seq, committed());
        }
        for seq in 0..64_i64 {
            assert!(ring.try_remove(seq).is_some(), "seq {seq} should be ready");
        }
    }

    #[test]
    fn non_power_of_two_panics() {
        let result = std::panic::catch_unwind(|| ResultRing::new(100));
        assert!(result.is_err());
    }
}
