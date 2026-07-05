//! A generic, pre-allocated, lock-free ring buffer for any event type.
//!
//! This is the generic sibling of [`crate::ring_buffer::RingBuffer`], which is
//! typed to [`crate::event::TransactionEvent`] for the financial pipeline.
//! Use `GenericRingBuffer<T>` when you need the same Disruptor-style pipeline
//! for a custom event type — for example, monitoring events, audit logs, or
//! gateway payloads.
//!
//! # Key differences from `RingBuffer`
//!
//! - **Type-generic**: works with any `T: Default + Send`.
//! - **Monotonic cursor**: `publish()` uses [`AtomicI64::fetch_max`] instead of
//!   `store`, so concurrent producers from multiple OS/Tokio threads can never
//!   push the cursor backwards.  This makes `GenericRingBuffer` safe for
//!   **multi-producer** usage while `RingBuffer` assumes a single producer.
//!
//! # Power-of-two sizing
//!
//! The capacity **must** be a power of two so slot indices can be computed with
//! a single bitwise AND (`sequence & mask`) instead of a division.
//!
//! # Single-writer principle (per slot)
//!
//! Multiple producers may call `next_sequence()` concurrently — the atomic
//! `claim.increment()` ensures each producer gets a unique sequence.  However,
//! each *slot* still has exactly one writer at a time: the producer that claimed
//! that sequence.  Consumers only access slots whose sequences have been
//! published via `publish()`.
//!
//! # Examples
//!
//! ```rust
//! use blazil_engine::generic_ring_buffer::GenericRingBuffer;
//!
//! #[derive(Default, Clone)]
//! struct MyEvent { value: u64 }
//!
//! let mut rb = GenericRingBuffer::<MyEvent>::new(64).unwrap();
//! let _gate = rb.add_gating_sequence(); // register consumer backpressure
//!
//! let seq = rb.next_sequence();
//! // SAFETY: single writer, freshly claimed slot.
//! unsafe { (*rb.get_mut(seq)).value = 42; }
//! rb.publish(seq);
//! ```

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use blazil_common::error::{BlazerError, BlazerResult};

use crate::sequence::Sequence;

// ── GenericRingBuffer ─────────────────────────────────────────────────────────

/// A pre-allocated, fixed-size, lock-free ring buffer for any event type `T`.
///
/// # Type parameters
///
/// - `T`: The event type stored in each slot.  Must be `Default` (for
///   pre-allocation) and `Send` (for cross-thread access).
///
/// # Capacity
///
/// Must be a power of two.  Validated in [`GenericRingBuffer::new`].
///
/// # Safety
///
/// Uses `UnsafeCell` for interior mutability without runtime borrow-checking
/// overhead.  Thread safety is enforced by the single-writer-per-slot
/// principle: only the producer that claimed a sequence writes to that slot,
/// and consumers only read slots that have been published.
pub struct GenericRingBuffer<T: Default + Send> {
    slots: Vec<UnsafeCell<T>>,
    capacity: usize,
    /// Bitmask for fast slot index: `sequence & mask` (no division).
    mask: usize,
    /// Producer claim counter — advanced atomically before writing.
    /// Not wrapped in `Arc`; the ring buffer owns it exclusively.
    claim: Sequence,
    /// Highest **published** sequence.
    ///
    /// Uses `AtomicI64` (not `Sequence`) so `publish()` can call
    /// `fetch_max`, ensuring the cursor only ever advances regardless of
    /// the order in which concurrent producers complete their writes.
    cursor: Arc<AtomicI64>,
    /// One gating sequence per consumer thread.
    ///
    /// Prevents the producer from lapping any consumer.  Registered via
    /// [`add_gating_sequence`][GenericRingBuffer::add_gating_sequence].
    gating_sequences: Vec<Arc<Sequence>>,
}

// SAFETY: `GenericRingBuffer` uses the single-writer-per-slot principle.
// Only the producer that claimed a sequence writes to that slot.
// Consumers only read slots that have been published.
// `UnsafeCell` is never aliased mutably from multiple threads simultaneously.
unsafe impl<T: Default + Send> Send for GenericRingBuffer<T> {}
unsafe impl<T: Default + Send> Sync for GenericRingBuffer<T> {}

impl<T: Default + Send> GenericRingBuffer<T> {
    /// Creates a new `GenericRingBuffer` with the given capacity.
    ///
    /// All slots are pre-allocated with `T::default()`.  No further heap
    /// allocation occurs after this call returns.
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::ValidationError`] if `capacity` is `0` or not a
    /// power of two.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::generic_ring_buffer::GenericRingBuffer;
    ///
    /// assert!(GenericRingBuffer::<u64>::new(1024).is_ok());
    /// assert!(GenericRingBuffer::<u64>::new(1000).is_err()); // not a power of two
    /// assert!(GenericRingBuffer::<u64>::new(0).is_err());
    /// ```
    pub fn new(capacity: usize) -> BlazerResult<Self> {
        if !Self::is_power_of_two(capacity) {
            return Err(BlazerError::ValidationError(format!(
                "GenericRingBuffer capacity must be a power of two, got {capacity}"
            )));
        }

        let mask = capacity - 1;
        let claim = Sequence::new(Sequence::INITIAL_VALUE);
        let cursor = Arc::new(AtomicI64::new(Sequence::INITIAL_VALUE));

        let mut slots = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            slots.push(UnsafeCell::new(T::default()));
        }

        Ok(Self {
            slots,
            capacity,
            mask,
            claim,
            cursor,
            gating_sequences: vec![],
        })
    }

    /// Claims the next available sequence number.
    ///
    /// Uses an atomic fetch-add — safe for **concurrent** callers from
    /// multiple producer threads.  Each caller receives a unique sequence.
    ///
    /// The producer **must** write to the slot at this sequence (via
    /// [`get_mut`][GenericRingBuffer::get_mut]) and then call
    /// [`publish`][GenericRingBuffer::publish].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::generic_ring_buffer::GenericRingBuffer;
    ///
    /// let rb = GenericRingBuffer::<u64>::new(64).unwrap();
    /// let seq = rb.next_sequence();
    /// assert_eq!(seq, 0); // first claimed sequence is 0
    /// ```
    #[inline(always)]
    pub fn next_sequence(&self) -> i64 {
        self.claim.increment()
    }

    /// Publishes a sequence, making it visible to consumers.
    ///
    /// Uses [`AtomicI64::fetch_max`] with `Release` ordering so the cursor
    /// only ever advances — even when multiple producers call `publish`
    /// concurrently in an arbitrary order, the highest sequence wins and
    /// the cursor never regresses.
    ///
    /// Call this **after** fully writing the event to the slot.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::generic_ring_buffer::GenericRingBuffer;
    ///
    /// let rb = GenericRingBuffer::<u64>::new(64).unwrap();
    /// let seq = rb.next_sequence();
    /// rb.publish(seq);
    /// ```
    #[inline(always)]
    pub fn publish(&self, sequence: i64) {
        self.cursor.fetch_max(sequence, Ordering::Release);
    }

    /// Returns `true` if the producer can claim a new slot without lapping
    /// the slowest registered consumer.
    ///
    /// When no consumers are registered, always returns `true` (no
    /// backpressure).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::generic_ring_buffer::GenericRingBuffer;
    ///
    /// let rb = GenericRingBuffer::<u64>::new(64).unwrap();
    /// assert!(rb.has_available_capacity()); // no consumers → always true
    /// ```
    #[inline]
    pub fn has_available_capacity(&self) -> bool {
        if self.gating_sequences.is_empty() {
            return true;
        }
        let next_claim = self.claim.get() + 1;
        let min_gate = self.gating_sequences.iter().map(|s| s.get()).min().unwrap();
        (next_claim - min_gate) < self.capacity as i64
    }

    /// Returns a raw mutable pointer to the slot at `sequence`.
    ///
    /// Index is computed as `sequence & mask` (one AND instruction, no
    /// division).
    ///
    /// # Safety
    ///
    /// The caller must uphold the single-writer-per-slot invariant:
    /// - Only the thread that claimed this sequence via `next_sequence()` may
    ///   write to it.
    /// - The write must complete before calling `publish()`.
    /// - No consumer may be reading the same slot concurrently.
    #[inline(always)]
    pub fn get_mut(&self, sequence: i64) -> *mut T {
        let index = (sequence as usize) & self.mask;
        self.slots[index].get()
    }

    /// Returns a raw read-only pointer to the slot at `sequence`.
    ///
    /// # Safety
    ///
    /// The caller must ensure the slot has been fully written and published
    /// (i.e. `sequence <= cursor.load(Acquire)`) before reading.
    #[inline(always)]
    pub fn get(&self, sequence: i64) -> *const T {
        let index = (sequence as usize) & self.mask;
        self.slots[index].get()
    }

    /// Returns the cursor `Arc` for consumers to poll.
    ///
    /// Consumers load the cursor with `Acquire` ordering to observe all
    /// slot writes that preceded the corresponding `publish()` call.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::generic_ring_buffer::GenericRingBuffer;
    /// use blazil_engine::sequence::Sequence;
    /// use std::sync::atomic::Ordering;
    ///
    /// let rb = GenericRingBuffer::<u64>::new(64).unwrap();
    /// assert_eq!(rb.cursor().load(Ordering::Acquire), Sequence::INITIAL_VALUE);
    /// ```
    #[inline]
    pub fn cursor(&self) -> &Arc<AtomicI64> {
        &self.cursor
    }

    /// Returns the first registered gating sequence.
    ///
    /// # Panics
    ///
    /// Panics if no gating sequence has been registered via
    /// [`add_gating_sequence`][GenericRingBuffer::add_gating_sequence].
    #[inline]
    pub fn gating_sequence(&self) -> &Arc<Sequence> {
        self.gating_sequences
            .first()
            .expect("gating_sequence() called before add_gating_sequence()")
    }

    /// Registers a new consumer gating sequence.
    ///
    /// Returns a shared `Arc` for the consumer to update as it advances.
    /// The producer checks the minimum across all gating sequences to
    /// prevent lapping any consumer.
    ///
    /// Call this **before** wrapping the ring buffer in an `Arc` and before
    /// spawning consumer threads.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::generic_ring_buffer::GenericRingBuffer;
    ///
    /// let mut rb = GenericRingBuffer::<u64>::new(64).unwrap();
    /// let gate = rb.add_gating_sequence();
    /// // pass `gate` to the consumer thread; consumer calls gate.set(seq)
    /// ```
    pub fn add_gating_sequence(&mut self) -> Arc<Sequence> {
        let gate = Arc::new(Sequence::new(Sequence::INITIAL_VALUE));
        self.gating_sequences.push(Arc::clone(&gate));
        gate
    }

    /// Returns the ring buffer capacity.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    fn is_power_of_two(n: usize) -> bool {
        n != 0 && (n & (n - 1)) == 0
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::thread;

    use super::*;

    #[test]
    fn new_power_of_two_succeeds() {
        assert!(GenericRingBuffer::<u64>::new(64).is_ok());
        assert!(GenericRingBuffer::<u64>::new(1024).is_ok());
        assert!(GenericRingBuffer::<u64>::new(1 << 20).is_ok());
    }

    #[test]
    fn new_non_power_of_two_fails() {
        assert!(GenericRingBuffer::<u64>::new(1000).is_err());
        assert!(GenericRingBuffer::<u64>::new(3).is_err());
        assert!(GenericRingBuffer::<u64>::new(0).is_err());
    }

    #[test]
    fn capacity_returns_correct_value() {
        let rb = GenericRingBuffer::<u64>::new(512).unwrap();
        assert_eq!(rb.capacity(), 512);
    }

    #[test]
    fn first_sequence_is_zero() {
        let rb = GenericRingBuffer::<u64>::new(64).unwrap();
        assert_eq!(rb.next_sequence(), 0);
    }

    #[test]
    fn cursor_starts_at_initial_value() {
        let rb = GenericRingBuffer::<u64>::new(64).unwrap();
        assert_eq!(rb.cursor().load(Ordering::Acquire), Sequence::INITIAL_VALUE);
    }

    #[test]
    fn single_thread_write_read() {
        let rb = GenericRingBuffer::<u64>::new(64).unwrap();

        let seq = rb.next_sequence();
        assert_eq!(seq, 0);

        // SAFETY: single-threaded test, we own the sequence.
        unsafe {
            *rb.get_mut(seq) = 99;
        }
        rb.publish(seq);

        assert_eq!(rb.cursor().load(Ordering::Acquire), 0);

        // SAFETY: slot was published above, no concurrent writes.
        unsafe {
            assert_eq!(*rb.get(seq), 99);
        }
    }

    #[test]
    fn publish_is_monotonic() {
        // Simulate out-of-order concurrent publish: seq 1 publishes before seq 0.
        // fetch_max ensures cursor ends up at 1, not regressed to 0.
        let rb = GenericRingBuffer::<u64>::new(64).unwrap();
        let seq0 = rb.next_sequence(); // 0
        let seq1 = rb.next_sequence(); // 1

        rb.publish(seq1); // publish 1 first
        assert_eq!(rb.cursor().load(Ordering::Acquire), 1);

        rb.publish(seq0); // publish 0 late — must NOT push cursor back
        assert_eq!(rb.cursor().load(Ordering::Acquire), 1); // cursor stays at 1
    }

    #[test]
    fn has_available_capacity_without_consumer() {
        let rb = GenericRingBuffer::<u64>::new(64).unwrap();
        // No gating sequences → always has capacity
        assert!(rb.has_available_capacity());
    }

    #[test]
    fn has_available_capacity_with_consumer() {
        let mut rb = GenericRingBuffer::<u64>::new(4).unwrap();
        let gate = rb.add_gating_sequence();

        // Consume capacity: publish 4 events without advancing the consumer gate.
        for _ in 0..4 {
            let seq = rb.next_sequence();
            rb.publish(seq);
        }
        // Buffer is full (4 published, consumer gate still at INITIAL_VALUE = -1).
        // next_claim = 4, min_gate = -1, diff = 5 >= capacity 4 → full
        assert!(!rb.has_available_capacity());

        // Advance consumer gate → capacity freed
        gate.set(3);
        assert!(rb.has_available_capacity());
    }

    #[test]
    fn multi_thread_single_producer_consumer() {
        let mut inner = GenericRingBuffer::<i32>::new(1024).unwrap();
        let gate = inner.add_gating_sequence();
        let rb = Arc::new(inner);

        let producer_rb = Arc::clone(&rb);
        let consumer_rb = Arc::clone(&rb);

        let producer = thread::spawn(move || {
            for i in 0..100_i32 {
                let seq = producer_rb.next_sequence();
                // SAFETY: single producer, freshly claimed slot.
                unsafe {
                    *producer_rb.get_mut(seq) = i;
                }
                producer_rb.publish(seq);
            }
        });

        let consumer = thread::spawn(move || {
            let cursor = consumer_rb.cursor();
            let mut expected: i64 = 0;
            while expected < 100 {
                let available = cursor.load(Ordering::Acquire);
                if expected <= available {
                    // SAFETY: sequence was published (expected <= available).
                    unsafe {
                        assert_eq!(*consumer_rb.get(expected), expected as i32);
                    }
                    gate.set(expected);
                    expected += 1;
                }
            }
        });

        producer.join().unwrap();
        consumer.join().unwrap();
    }
}
