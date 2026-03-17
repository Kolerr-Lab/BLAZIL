//! Backpressure detection for the ring buffer.
//!
//! [`BackpressureGuard`] monitors the gap between the producer cursor and
//! the consumer cursor. When this gap exceeds the configured
//! `high_watermark` fraction of the ring buffer capacity, the transport
//! layer should reject incoming connections or send a retry response.
//!
//! # Ratio calculation
//!
//! ```text
//! pending       = producer_cursor − consumer_cursor
//! pressure_ratio = pending / capacity
//! is_pressured  = pressure_ratio > high_watermark
//! ```
//!
//! Note: `pressure_ratio` and `high_watermark` use `f64` for ratio arithmetic.
//! These are **not** monetary values — the "no f32/f64 for money" rule does
//! not apply here.

use std::sync::Arc;

use blazil_engine::ring_buffer::RingBuffer;
use blazil_engine::sequence::Sequence;

// ── BackpressureGuard ─────────────────────────────────────────────────────────

/// Detects when the ring buffer is approaching capacity.
///
/// When [`is_pressured`][BackpressureGuard::is_pressured] returns `true`,
/// transports should reject new requests with a retry-after response rather
/// than blocking or publishing into the buffer.
///
/// # Default watermark
///
/// `0.75` — start rejecting when the buffer is 75% full.
///
/// # Examples
///
/// ```rust
/// use std::sync::Arc;
/// use blazil_engine::ring_buffer::RingBuffer;
/// use blazil_transport::backpressure::BackpressureGuard;
///
/// let rb = Arc::new(RingBuffer::new(1024).unwrap());
/// let guard = BackpressureGuard::new(Arc::clone(&rb), 0.75);
/// assert!(!guard.is_pressured());
/// assert_eq!(guard.pressure_ratio(), 0.0);
/// ```
pub struct BackpressureGuard {
    ring_buffer: Arc<RingBuffer>,
    /// Fraction of capacity at which the buffer is considered pressured.
    /// Must be in the range `(0.0, 1.0]`. Default: `0.75`.
    high_watermark: f64,
    /// Tracks how far the consumer has processed.
    ///
    /// Starts at [`Sequence::INITIAL_VALUE`] (−1). Updated externally by
    /// the connection handler after each result is received. Shared with
    /// any number of connection tasks via `Arc`.
    consumer_cursor: Arc<Sequence>,
}

impl BackpressureGuard {
    /// Creates a new `BackpressureGuard`.
    ///
    /// # Arguments
    ///
    /// - `ring_buffer` — the ring buffer to monitor.
    /// - `high_watermark` — fraction of capacity `(0.0, 1.0]` above which
    ///   [`is_pressured`][BackpressureGuard::is_pressured] returns `true`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::sync::Arc;
    /// use blazil_engine::ring_buffer::RingBuffer;
    /// use blazil_transport::backpressure::BackpressureGuard;
    ///
    /// let rb = Arc::new(RingBuffer::new(64).unwrap());
    /// let guard = BackpressureGuard::new(Arc::clone(&rb), 0.75);
    /// ```
    pub fn new(ring_buffer: Arc<RingBuffer>, high_watermark: f64) -> Self {
        let consumer_cursor = Arc::new(Sequence::new(Sequence::INITIAL_VALUE));
        Self {
            ring_buffer,
            high_watermark,
            consumer_cursor,
        }
    }

    /// Returns a clone of the consumer cursor handle.
    ///
    /// Connection handlers use this to update the consumer position after
    /// each event's result is received, keeping the pressure ratio accurate.
    pub fn consumer_cursor(&self) -> Arc<Sequence> {
        Arc::clone(&self.consumer_cursor)
    }

    /// Returns `true` when the buffer fill ratio exceeds the high watermark.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::sync::Arc;
    /// use blazil_engine::ring_buffer::RingBuffer;
    /// use blazil_transport::backpressure::BackpressureGuard;
    ///
    /// let rb = Arc::new(RingBuffer::new(1024).unwrap());
    /// let guard = BackpressureGuard::new(Arc::clone(&rb), 0.75);
    /// assert!(!guard.is_pressured());
    /// ```
    pub fn is_pressured(&self) -> bool {
        self.pressure_ratio() > self.high_watermark
    }

    /// Returns the current fill ratio of the ring buffer `[0.0, 1.0]`.
    ///
    /// `0.0` means the buffer is empty; `1.0` means every slot is pending.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::sync::Arc;
    /// use blazil_engine::ring_buffer::RingBuffer;
    /// use blazil_transport::backpressure::BackpressureGuard;
    ///
    /// let rb = Arc::new(RingBuffer::new(1024).unwrap());
    /// let guard = BackpressureGuard::new(Arc::clone(&rb), 0.75);
    /// assert_eq!(guard.pressure_ratio(), 0.0);
    /// ```
    pub fn pressure_ratio(&self) -> f64 {
        let producer = self.ring_buffer.cursor().get();
        let consumer = self.consumer_cursor.get();
        let capacity = self.ring_buffer.capacity() as i64;

        if capacity == 0 {
            return 0.0;
        }

        // Both cursors start at INITIAL_VALUE (−1).
        // pending = 0 when producer == consumer (nothing in flight).
        let pending = producer - consumer;
        if pending <= 0 {
            return 0.0;
        }

        let ratio = (pending as f64) / (capacity as f64);
        // Clamp to 1.0 in case the consumer has fallen more than one full
        // rotation behind (should not happen with correct usage).
        ratio.min(1.0)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use blazil_common::ids::{AccountId, LedgerId, TransactionId};
    use blazil_engine::event::TransactionEvent;

    fn make_ring_buffer(capacity: usize) -> Arc<RingBuffer> {
        Arc::new(RingBuffer::new(capacity).unwrap())
    }

    fn make_event() -> TransactionEvent {
        TransactionEvent::new(
            TransactionId::new(),
            AccountId::new(),
            AccountId::new(),
            1_00_u64, // $1.00 in cents
            LedgerId::USD,
            1,
        )
    }

    #[test]
    fn empty_ring_buffer_is_not_pressured() {
        let rb = make_ring_buffer(1024);
        let guard = BackpressureGuard::new(Arc::clone(&rb), 0.75);
        assert!(!guard.is_pressured());
    }

    #[test]
    fn pressure_ratio_is_zero_on_empty_buffer() {
        let rb = make_ring_buffer(1024);
        let guard = BackpressureGuard::new(Arc::clone(&rb), 0.75);
        assert_eq!(guard.pressure_ratio(), 0.0);
    }

    #[test]
    fn high_watermark_respected() {
        // Use capacity=16 for easy arithmetic.
        let rb = make_ring_buffer(16);
        let guard = BackpressureGuard::new(Arc::clone(&rb), 0.75);

        // Publish 12 events (75% of 16) — should be exactly at watermark.
        for _ in 0..12 {
            let event = make_event();
            let seq = rb.next_sequence();
            // SAFETY: single-threaded test; we own the sequence.
            unsafe {
                *rb.get_mut(seq) = event;
            }
            rb.publish(seq);
        }

        // consumer_cursor is still at INITIAL_VALUE (−1), so
        // pending = 11 − (−1) = 12, ratio = 12/16 = 0.75.
        // 0.75 is NOT > 0.75 (strictly greater), so not yet pressured.
        assert!(!guard.is_pressured());

        // Publish one more — pending = 13, ratio = 13/16 = 0.8125 > 0.75.
        let event2 = make_event();
        let seq = rb.next_sequence();
        unsafe {
            *rb.get_mut(seq) = event2;
        }
        rb.publish(seq);

        assert!(guard.is_pressured());
    }

    #[test]
    fn consumer_cursor_reduces_pressure() {
        let rb = make_ring_buffer(16);
        let guard = BackpressureGuard::new(Arc::clone(&rb), 0.75);
        let consumer = guard.consumer_cursor();

        // Fill to pressure.
        for _ in 0..13 {
            let event = make_event();
            let seq = rb.next_sequence();
            unsafe {
                *rb.get_mut(seq) = event;
            }
            rb.publish(seq);
        }
        assert!(guard.is_pressured());

        // Consumer processes all events — advance its cursor to producer cursor.
        consumer.set(rb.cursor().get());
        assert!(!guard.is_pressured());
    }
}
