//! Atomic sequence counter for the Disruptor ring buffer.
//!
//! Every slot in the ring buffer is identified by a monotonically increasing
//! sequence number. Producers *claim* sequences before writing; consumers
//! *track* sequences to know how far they have read.
//!
//! # False sharing
//!
//! A `Sequence` is padded to exactly one CPU cache line (64 bytes). Without
//! this padding the producer and consumer `Sequence` values would share a
//! cache line, causing every write by the producer to invalidate the cache
//! entry on the consumer's CPU core — and vice versa. This is *false sharing*
//! and it destroys throughput. The padding eliminates it entirely.

use std::sync::atomic::{AtomicI64, Ordering};

// ── Sequence ──────────────────────────────────────────────────────────────────

/// Atomic sequence counter, padded to 64 bytes to prevent false sharing.
///
/// One CPU cache line is exactly 64 bytes on all modern x86-64 and aarch64
/// processors. Placing `Sequence` values on separate cache lines ensures that
/// updating the producer sequence never invalidates the consumer's cache line.
///
/// # Examples
///
/// ```rust
/// use blazil_engine::sequence::Sequence;
///
/// let seq = Sequence::new(0);
/// assert_eq!(seq.get(), 0);
/// let new_val = seq.increment();
/// assert_eq!(new_val, 1);
/// assert_eq!(seq.get(), 1);
/// ```
#[repr(C)]
pub struct Sequence {
    value: AtomicI64,
    /// Padding to align the struct to a full 64-byte CPU cache line.
    /// `AtomicI64` is 8 bytes; 64 − 8 = 56 bytes of padding.
    _padding: [u8; 56],
}

impl Sequence {
    /// Sequence value used to indicate "no event published yet".
    /// Consumers start at this value; the first valid sequence is `0`.
    pub const INITIAL_VALUE: i64 = -1;

    /// Creates a new `Sequence` initialized to `initial`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::sequence::Sequence;
    ///
    /// let seq = Sequence::new(Sequence::INITIAL_VALUE);
    /// assert_eq!(seq.get(), -1);
    /// ```
    pub fn new(initial: i64) -> Self {
        Self {
            value: AtomicI64::new(initial),
            _padding: [0u8; 56],
        }
    }

    /// Reads the current sequence value.
    ///
    /// Uses `Acquire` ordering so all writes that happened before the matching
    /// `Release` are visible to this thread.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::sequence::Sequence;
    ///
    /// let seq = Sequence::new(42);
    /// assert_eq!(seq.get(), 42);
    /// ```
    #[inline]
    pub fn get(&self) -> i64 {
        self.value.load(Ordering::Acquire)
    }

    /// Overwrites the sequence value.
    ///
    /// Uses `Release` ordering so all preceding writes are visible to any
    /// thread that subsequently reads this sequence with `Acquire`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::sequence::Sequence;
    ///
    /// let seq = Sequence::new(0);
    /// seq.set(10);
    /// assert_eq!(seq.get(), 10);
    /// ```
    #[inline]
    pub fn set(&self, value: i64) {
        self.value.store(value, Ordering::Release);
    }

    /// Atomically increments the sequence by 1 and returns the **new** value.
    ///
    /// Uses `AcqRel` ordering to form a full memory barrier. This is the only
    /// correct ordering for a read-modify-write operation that must be
    /// sequentially consistent with both producers and consumers.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::sequence::Sequence;
    ///
    /// let seq = Sequence::new(0);
    /// assert_eq!(seq.increment(), 1);
    /// assert_eq!(seq.increment(), 2);
    /// ```
    #[inline]
    pub fn increment(&self) -> i64 {
        self.value.fetch_add(1, Ordering::AcqRel) + 1
    }
}

// SAFETY: Sequence is a wrapper around AtomicI64, which is already Sync + Send.
unsafe impl Send for Sequence {}
unsafe impl Sync for Sequence {}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    /// CRITICAL: Sequence MUST be exactly 64 bytes to occupy one cache line.
    /// If this test fails, false sharing is reintroduced.
    #[test]
    fn sequence_is_exactly_64_bytes() {
        assert_eq!(size_of::<Sequence>(), 64);
    }

    #[test]
    fn new_zero_get_returns_zero() {
        let seq = Sequence::new(0);
        assert_eq!(seq.get(), 0);
    }

    #[test]
    fn new_with_initial_value_minus_one() {
        let seq = Sequence::new(Sequence::INITIAL_VALUE);
        assert_eq!(seq.get(), -1);
    }

    #[test]
    fn increment_returns_new_value() {
        let seq = Sequence::new(0);
        assert_eq!(seq.increment(), 1);
        assert_eq!(seq.get(), 1);
    }

    #[test]
    fn multiple_increments_are_monotonically_increasing() {
        let seq = Sequence::new(0);
        let mut prev = seq.get();
        for _ in 0..10 {
            let next = seq.increment();
            assert!(next > prev);
            prev = next;
        }
    }

    #[test]
    fn set_overwrites_value() {
        let seq = Sequence::new(0);
        seq.set(100);
        assert_eq!(seq.get(), 100);
    }

    #[test]
    fn initial_value_constant_is_minus_one() {
        assert_eq!(Sequence::INITIAL_VALUE, -1);
    }
}
