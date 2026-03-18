//! Pre-allocated, fixed-size, lock-free ring buffer.
//!
//! This is the central data structure of the Disruptor pipeline. All
//! transaction events flow through a single `RingBuffer`, which is
//! allocated **once** at startup and reused indefinitely — there is zero
//! heap allocation on the hot path.
//!
//! # Power-of-two sizing
//!
//! The capacity **must** be a power of two. This allows the slot index to be
//! computed with a single bitwise AND:
//!
//! ```text
//! index = sequence & mask   (mask = capacity − 1)
//! ```
//!
//! A modulo operation (`sequence % capacity`) requires an integer division
//! instruction (~20–90 CPU cycles). A bitmask is a single AND instruction
//! (~1 cycle). At millions of transactions per second, this difference is
//! measurable.
//!
//! # Single-writer principle
//!
//! Each slot has at most **one** writer at any point in time.
//! The producer claims a sequence, writes to the slot, then publishes the
//! sequence. Consumers only access slots whose sequences have been published.
//! This protocol eliminates the need for locks or CAS loops on the hot path.
//!
//! # Default capacity
//!
//! `DEFAULT_CAPACITY = 1024 * 1024` (1 048 576 slots ≈ 1 M events in flight).
//! Each slot holds one [`TransactionEvent`]. For systems with extreme memory
//! constraints, use a smaller power of two (e.g. `65_536`).

use std::cell::UnsafeCell;
use std::sync::Arc;

use blazil_common::error::{BlazerError, BlazerResult};

use crate::event::TransactionEvent;
use crate::sequence::Sequence;

/// Default ring buffer capacity (1 048 576 slots).
pub const DEFAULT_CAPACITY: usize = 1024 * 1024;

// ── RingBuffer ────────────────────────────────────────────────────────────────

/// A pre-allocated, fixed-size, lock-free ring buffer for [`TransactionEvent`]s.
///
/// # Capacity
///
/// Must be a power of two. Validated in [`RingBuffer::new`].
/// Default: [`DEFAULT_CAPACITY`].
///
/// # Safety
///
/// Uses `UnsafeCell` for interior mutability without runtime borrow-checking
/// overhead. Thread safety is enforced by the single-writer principle: only
/// one producer writes to a slot at a time, and consumers only read slots
/// that have been published by the producer.
///
/// # Examples
///
/// ```rust
/// use blazil_engine::ring_buffer::RingBuffer;
///
/// let rb = RingBuffer::new(1024).unwrap();
/// assert_eq!(rb.capacity(), 1024);
/// ```
pub struct RingBuffer {
    slots: Vec<UnsafeCell<TransactionEvent>>,
    capacity: usize,
    /// Bitmask for fast slot index calculation: `sequence & mask`.
    mask: usize,
    /// Producer claim counter. Advances in `next_sequence` BEFORE the slot
    /// is written. Kept separate from `cursor` so consumers cannot observe a
    /// sequence until it has been explicitly published via `publish()`.
    ///
    /// Not wrapped in `Arc` — only the single producer touches this field.
    claim: Sequence,
    /// The highest **published** sequence number.
    ///
    /// Wrapped in `Arc` so consumers can hold a reference independently of
    /// the `RingBuffer`. Only advances in `publish()`, which is called after
    /// the slot has been fully written (Release store), establishing a
    /// happens-before with any subsequent Acquire load by the runner.
    cursor: Arc<Sequence>,
    /// Gating sequences for all consumers (one per worker thread).
    ///
    /// Used to prevent the producer from lapping consumers. When
    /// (claim - MIN(gating_sequences)) >= capacity, the ring buffer is full.
    /// Each consumer updates only its own gating sequence (lock-free).
    gating_sequences: Vec<Arc<Sequence>>,
}

// SAFETY: RingBuffer uses the single-writer principle enforced at the call
// site. Only one producer writes to a given slot (after claiming a sequence
// via `next_sequence`). Consumers only read slots that have been published via
// `publish`. The `UnsafeCell` is never aliased mutably from multiple threads
// simultaneously.
unsafe impl Send for RingBuffer {}
unsafe impl Sync for RingBuffer {}

impl RingBuffer {
    /// Creates a new `RingBuffer` with the given capacity.
    ///
    /// Allocates all slots upfront. No further heap allocation occurs after
    /// this call returns.
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::ValidationError`] if `capacity` is `0` or not a
    /// power of two.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::ring_buffer::RingBuffer;
    ///
    /// assert!(RingBuffer::new(1024).is_ok());
    /// assert!(RingBuffer::new(1000).is_err()); // not a power of two
    /// assert!(RingBuffer::new(0).is_err());
    /// ```
    pub fn new(capacity: usize) -> BlazerResult<Self> {
        if !Self::is_power_of_two(capacity) {
            return Err(BlazerError::ValidationError(format!(
                "RingBuffer capacity must be a power of two, got {capacity}"
            )));
        }

        let mask = capacity - 1;
        let claim = Sequence::new(Sequence::INITIAL_VALUE);
        let cursor = Arc::new(Sequence::new(Sequence::INITIAL_VALUE));
        // Start with one default gating sequence for backward compatibility
        let default_gating = Arc::new(Sequence::new(Sequence::INITIAL_VALUE));
        let gating_sequences = vec![default_gating];

        // Pre-allocate all slots. Use `with_capacity` + extend to avoid
        // excess reallocation. Each slot contains a default TransactionEvent.
        let mut slots = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            let event = Self::default_event();
            slots.push(UnsafeCell::new(event));
        }

        Ok(Self {
            slots,
            capacity,
            mask,
            claim,
            cursor,
            gating_sequences,
        })
    }

    /// Claims the next available sequence number for a producer.
    ///
    /// The producer **must** write to the slot at this sequence (via
    /// [`get_mut`][RingBuffer::get_mut]) and then call
    /// [`publish`][RingBuffer::publish] to make it visible to consumers.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::ring_buffer::RingBuffer;
    ///
    /// let rb = RingBuffer::new(64).unwrap();
    /// let seq = rb.next_sequence();
    /// assert_eq!(seq, 0); // first claimed sequence is 0
    /// ```
    #[inline(always)]
    pub fn next_sequence(&self) -> i64 {
        // Advance only the claim counter. The cursor (visible to the runner)
        // is NOT moved until `publish()` is called after the slot is written.
        self.claim.increment()
    }

    /// Publishes a sequence, making it visible to consumers.
    ///
    /// Call this **after** writing the event to the slot. `publish` issues a
    /// Release store on the cursor, establishing a happens-before with the
    /// runner's subsequent Acquire load. This guarantees that consumers never
    /// observe a sequence before the corresponding slot has been fully written.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::ring_buffer::RingBuffer;
    ///
    /// let rb = RingBuffer::new(64).unwrap();
    /// let seq = rb.next_sequence();
    /// rb.publish(seq);
    /// ```
    #[inline(always)]
    pub fn publish(&self, sequence: i64) {
        self.cursor.set(sequence);
    }

    /// Returns a raw mutable pointer to the slot at `sequence`.
    ///
    /// Index is computed as `sequence & mask` (no division).
    ///
    /// # Safety
    ///
    /// The caller must ensure the single-writer principle:
    /// - Only one thread writes to a given slot at a time.
    /// - The slot must have been claimed via [`next_sequence`][RingBuffer::next_sequence].
    /// - No consumer may be reading the same slot concurrently.
    #[inline(always)]
    pub fn get_mut(&self, sequence: i64) -> *mut TransactionEvent {
        // SAFETY: sequence & mask is always in bounds: mask = capacity - 1,
        // so the index is in [0, capacity). The UnsafeCell<T> guarantees that
        // mutable aliasing is possible; the single-writer protocol ensures no
        // two threads hold a mutable reference to the same slot simultaneously.
        let index = (sequence as usize) & self.mask;
        self.slots[index].get()
    }

    /// Returns a raw read-only pointer to the slot at `sequence`.
    ///
    /// Consumers call this after confirming the sequence has been published.
    ///
    /// # Safety
    ///
    /// The caller must ensure the slot has been fully written and published
    /// by the producer before reading (i.e., `sequence <= cursor.get()`).
    #[inline(always)]
    pub fn get(&self, sequence: i64) -> *const TransactionEvent {
        let index = (sequence as usize) & self.mask;
        self.slots[index].get()
    }

    /// Returns a reference to the cursor `Arc<Sequence>`.
    ///
    /// Consumers use this to know how far the producer has published.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::ring_buffer::RingBuffer;
    /// use blazil_engine::sequence::Sequence;
    ///
    /// let rb = RingBuffer::new(64).unwrap();
    /// assert_eq!(rb.cursor().get(), Sequence::INITIAL_VALUE);
    /// ```
    #[inline]
    pub fn cursor(&self) -> &Arc<Sequence> {
        &self.cursor
    }

    /// Returns a reference to the first gating sequence `Arc<Sequence>`.
    ///
    /// For single-consumer pipelines, this is the only gating sequence.
    /// For multi-consumer pipelines, each consumer has its own gating sequence
    /// registered via [`add_gating_sequence`][RingBuffer::add_gating_sequence].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::ring_buffer::RingBuffer;
    /// use blazil_engine::sequence::Sequence;
    ///
    /// let rb = RingBuffer::new(64).unwrap();
    /// assert_eq!(rb.gating_sequence().get(), Sequence::INITIAL_VALUE);
    /// ```
    #[inline]
    pub fn gating_sequence(&self) -> &Arc<Sequence> {
        &self.gating_sequences[0]
    }

    /// Adds a new gating sequence for a consumer (worker thread).
    ///
    /// Returns a cloned Arc to the newly added gating sequence for the consumer to update.
    /// The producer will compute MIN across all gating sequences to prevent lapping.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::ring_buffer::RingBuffer;
    ///
    /// let mut rb = RingBuffer::new(64).unwrap();
    /// let worker1_gate = rb.add_gating_sequence();
    /// let worker2_gate = rb.add_gating_sequence();
    /// // Now rb has 3 gating sequences (1 default + 2 added)
    /// ```
    pub fn add_gating_sequence(&mut self) -> Arc<Sequence> {
        let new_gate = Arc::new(Sequence::new(Sequence::INITIAL_VALUE));
        self.gating_sequences.push(Arc::clone(&new_gate));
        new_gate
    }

    /// Checks if the ring buffer has available capacity for the producer
    /// to claim a new slot without lapping the slowest consumer.
    ///
    /// Returns `true` if there is space, `false` if the buffer is full.
    /// For multi-consumer pipelines, computes MIN across all gating sequences.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::ring_buffer::RingBuffer;
    ///
    /// let rb = RingBuffer::new(64).unwrap();
    /// assert!(rb.has_available_capacity()); // empty buffer has capacity
    /// ```
    #[inline]
    pub fn has_available_capacity(&self) -> bool {
        let next_claim = self.claim.get() + 1;
        // Compute minimum gating sequence across all consumers (slowest consumer)
        let min_gate = self
            .gating_sequences
            .iter()
            .map(|seq| seq.get())
            .min()
            .unwrap_or(Sequence::INITIAL_VALUE);
        (next_claim - min_gate) < self.capacity as i64
    }

    /// Returns the ring buffer capacity.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::ring_buffer::RingBuffer;
    ///
    /// let rb = RingBuffer::new(512).unwrap();
    /// assert_eq!(rb.capacity(), 512);
    /// ```
    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns `true` if `n` is a power of two (and non-zero).
    fn is_power_of_two(n: usize) -> bool {
        n != 0 && (n & (n - 1)) == 0
    }

    /// Returns a minimal default `TransactionEvent` for slot initialisation.
    ///
    /// This event is never "processed" — it will be overwritten before the
    /// slot is consumed. We need a concrete default to fill the Vec at
    /// allocation time.
    fn default_event() -> TransactionEvent {
        use blazil_common::ids::{AccountId, LedgerId, TransactionId};

        TransactionEvent::new(
            TransactionId::new(),
            AccountId::new(),
            AccountId::new(),
            0_u64,
            LedgerId::USD,
            0,
        )
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use blazil_common::ids::{AccountId, LedgerId, TransactionId};

    fn make_event() -> TransactionEvent {
        TransactionEvent::new(
            TransactionId::new(),
            AccountId::new(),
            AccountId::new(),
            10_000_u64, // $100.00 in cents
            LedgerId::USD,
            1,
        )
    }

    #[test]
    fn new_1024_succeeds() {
        assert!(RingBuffer::new(1024).is_ok());
    }

    #[test]
    fn new_non_power_of_two_fails() {
        assert!(RingBuffer::new(1000).is_err());
        assert!(RingBuffer::new(999).is_err());
        assert!(RingBuffer::new(3).is_err());
    }

    #[test]
    fn new_zero_fails() {
        assert!(RingBuffer::new(0).is_err());
    }

    #[test]
    fn capacity_returns_correct_value() {
        let rb = RingBuffer::new(1024).unwrap();
        assert_eq!(rb.capacity(), 1024);
    }

    #[test]
    fn mask_equals_capacity_minus_one() {
        let rb = RingBuffer::new(1024).unwrap();
        assert_eq!(rb.mask, 1024 - 1);
    }

    #[test]
    fn write_event_to_slot_then_read_back_fields_match() {
        let rb = RingBuffer::new(64).unwrap();
        let seq = rb.next_sequence();

        let event = make_event();
        let tx_id = event.transaction_id;
        let debit_id = event.debit_account_id;
        let credit_id = event.credit_account_id;

        // SAFETY: single-threaded test; we own the sequence.
        unsafe {
            let slot = rb.get_mut(seq);
            *slot = event;
        }
        rb.publish(seq);

        // SAFETY: sequence was published above, no concurrent writes.
        unsafe {
            let slot = &*rb.get(seq);
            assert_eq!(slot.transaction_id, tx_id);
            assert_eq!(slot.debit_account_id, debit_id);
            assert_eq!(slot.credit_account_id, credit_id);
            assert_eq!(slot.code, 1);
        }
    }

    #[test]
    fn cursor_starts_at_initial_value_before_any_publish() {
        let rb = RingBuffer::new(64).unwrap();
        // next_sequence increments, so after construction cursor is INITIAL_VALUE
        assert_eq!(rb.cursor().get(), Sequence::INITIAL_VALUE);
    }

    #[test]
    fn is_power_of_two_helper() {
        assert!(RingBuffer::is_power_of_two(1));
        assert!(RingBuffer::is_power_of_two(2));
        assert!(RingBuffer::is_power_of_two(1024));
        assert!(RingBuffer::is_power_of_two(1 << 20));
        assert!(!RingBuffer::is_power_of_two(0));
        assert!(!RingBuffer::is_power_of_two(3));
        assert!(!RingBuffer::is_power_of_two(1000));
    }
}
