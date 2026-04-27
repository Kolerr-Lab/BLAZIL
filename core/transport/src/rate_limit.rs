//! Lock-free token bucket rate limiter using atomic operations only.
//!
//! Prevents OOM under extreme load by rejecting requests that exceed
//! the configured rate limit (e.g., 55,000 TPS).
//!
//! ## Algorithm
//!
//! Classic token bucket:
//! - Tokens refill at a constant rate (e.g., 55,000 per second)
//! - Each request consumes 1 token
//! - If bucket is empty, request is rejected (gRPC ResourceExhausted)
//!
//! ## Lock-Free Implementation
//!
//! Uses only atomic operations (no mutexes):
//! - `tokens`: AtomicI64 (current token count)
//! - `last_refill_ns`: AtomicU64 (timestamp of last refill)
//! - Refill logic: Calculate elapsed time → add tokens → CAS loop
//! - Consume logic: Check if token available → CAS decrement
//!
//! This matches LMAX Disruptor philosophy: no locks on hot path.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Lock-free token bucket rate limiter.
///
/// All operations use atomics only (no locks). Safe for concurrent use
/// across multiple threads without contention.
pub struct TokenBucket {
    /// Current number of tokens (can be negative during burst).
    tokens: AtomicI64,
    /// Timestamp (nanoseconds since UNIX_EPOCH) of last refill.
    last_refill_ns: AtomicU64,
    /// Refill rate: tokens per second.
    rate: u64,
    /// Maximum burst capacity (max tokens in bucket).
    capacity: i64,
}

impl TokenBucket {
    /// Creates a new `TokenBucket`.
    ///
    /// - `rate`: tokens refilled per second (e.g., 55,000 for 55K TPS limit)
    /// - `capacity`: max burst size (e.g., 1,000 for 1-second burst headroom)
    pub fn new(rate: u64, capacity: u64) -> Self {
        let now_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos() as u64;

        Self {
            tokens: AtomicI64::new(capacity as i64),
            last_refill_ns: AtomicU64::new(now_ns),
            rate,
            capacity: capacity as i64,
        }
    }

    /// Attempts to consume one token.
    ///
    /// Returns `true` if token was available (request allowed).
    /// Returns `false` if bucket is empty (request should be rejected).
    ///
    /// This method is lock-free and safe to call from multiple threads.
    pub fn try_consume(&self) -> bool {
        // Step 1: Refill tokens based on elapsed time
        self.refill();

        // Step 2: Try to consume one token atomically
        loop {
            let current = self.tokens.load(Ordering::Acquire);
            if current <= 0 {
                // Bucket empty — reject request
                return false;
            }

            // Try to decrement atomically (CAS loop)
            match self.tokens.compare_exchange(
                current,
                current - 1,
                Ordering::Release,
                Ordering::Acquire,
            ) {
                Ok(_) => return true, // Successfully consumed token
                Err(_) => continue,   // CAS failed, retry
            }
        }
    }

    /// Refills tokens based on elapsed time since last refill.
    ///
    /// Uses atomic CAS to ensure only one thread refills at a time,
    /// but other threads can proceed without blocking.
    fn refill(&self) {
        let now_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos() as u64;

        let last_ns = self.last_refill_ns.load(Ordering::Acquire);
        let elapsed_ns = now_ns.saturating_sub(last_ns);

        if elapsed_ns == 0 {
            // No time elapsed, no refill needed
            return;
        }

        // Calculate tokens to add: (rate * elapsed_ns) / 1_000_000_000
        let tokens_to_add = (self.rate as u128 * elapsed_ns as u128) / 1_000_000_000;
        if tokens_to_add == 0 {
            // Less than 1 nanosecond worth of tokens
            return;
        }

        // Try to update last_refill_ns (CAS to prevent double-refill)
        if self
            .last_refill_ns
            .compare_exchange(last_ns, now_ns, Ordering::Release, Ordering::Acquire)
            .is_err()
        {
            // Another thread already refilled, skip
            return;
        }

        // Add tokens atomically (capped at capacity)
        loop {
            let current = self.tokens.load(Ordering::Acquire);
            let new_tokens = (current + tokens_to_add as i64).min(self.capacity);

            match self.tokens.compare_exchange(
                current,
                new_tokens,
                Ordering::Release,
                Ordering::Acquire,
            ) {
                Ok(_) => break,     // Successfully refilled
                Err(_) => continue, // CAS failed, retry
            }
        }
    }

    /// Returns the current number of available tokens (for observability).
    pub fn available(&self) -> i64 {
        self.tokens.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_token_bucket_basic() {
        let bucket = TokenBucket::new(10, 10);

        // Should allow 10 requests immediately
        for _ in 0..10 {
            assert!(bucket.try_consume());
        }

        // 11th request should fail (bucket empty)
        assert!(!bucket.try_consume());

        // Wait for refill (100ms = 1 token at 10 TPS)
        thread::sleep(Duration::from_millis(100));

        // Now 1 more request should succeed
        assert!(bucket.try_consume());
    }

    #[test]
    fn test_token_bucket_refill() {
        let bucket = TokenBucket::new(1000, 100);

        // Drain bucket
        for _ in 0..100 {
            assert!(bucket.try_consume());
        }
        assert!(!bucket.try_consume());

        // Wait 50ms = 50 tokens at 1000 TPS
        thread::sleep(Duration::from_millis(50));

        // Should allow ~50 requests
        let mut allowed = 0;
        for _ in 0..60 {
            if bucket.try_consume() {
                allowed += 1;
            }
        }
        assert!((45..=55).contains(&allowed), "expected ~50, got {allowed}");
    }
}
