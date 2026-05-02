//! Priority-aware multi-stream Aeron publisher.
//!
//! Wraps 3 [`AeronPublication`] instances (critical, high, normal) and routes
//! outbound messages to the appropriate stream based on [`EventPriority`].
//!
//! # Architecture
//!
//! ```text
//!           ┌──────────────────┐
//!           │ PriorityPublisher│
//!           └────────┬─────────┘
//!                    │
//!         ┌──────────┼──────────┐
//!         │          │          │
//!    ┌────▼───┐ ┌───▼────┐ ┌───▼────┐
//!    │Critical│ │  High  │ │ Normal │
//!    │  Pub   │ │  Pub   │ │  Pub   │
//!    │ (100)  │ │ (200)  │ │ (300)  │
//!    └────────┘ └────────┘ └────────┘
//! ```
//!
//! Each publication has independent backpressure handling, preventing critical
//! events from being blocked by congestion on normal streams.
//!
//! # Drop ordering
//!
//! Drop this *before* the [`super::context::AeronContext`] it was created from.

use std::time::Duration;

use blazil_common::error::BlazerResult;

use super::context::AeronContext;
use super::publication::AeronPublication;
use crate::priority::{
    EventPriority, STREAM_CRITICAL_REQ, STREAM_CRITICAL_RSP, STREAM_HIGH_REQ, STREAM_HIGH_RSP,
    STREAM_NORMAL_REQ, STREAM_NORMAL_RSP,
};

// ── PriorityPublisher ─────────────────────────────────────────────────────────

/// Multi-stream Aeron publisher with priority-based routing.
///
/// Creates 3 independent publications on stream IDs 100 (critical), 200 (high),
/// 300 (normal). Messages are routed based on [`EventPriority`].
///
/// # Example (server-side)
///
/// ```no_run
/// # use blazil_transport::aeron::{AeronContext, PriorityPublisher};
/// # use blazil_transport::EventPriority;
/// # use std::time::Duration;
/// # fn example(ctx: &AeronContext) -> blazil_common::error::BlazerResult<()> {
/// // Server publishes responses to clients
/// let publisher = PriorityPublisher::new_for_responses(
///     ctx,
///     "aeron:ipc",
///     Duration::from_secs(5),
/// )?;
///
/// // Critical response bypasses all other traffic
/// let margin_call_resp = b"MARGIN_CALL_ACK:12345";
/// publisher.offer(EventPriority::Critical, margin_call_resp)?;
///
/// // Normal response uses standard stream
/// let transaction_resp = b"TXN_ACK:67890";
/// publisher.offer(EventPriority::Normal, transaction_resp)?;
/// # Ok(())
/// # }
/// ```
///
/// **Not `Send` or `Sync`** — must be used from the thread that created it.
pub struct PriorityPublisher {
    /// Critical priority publication (stream 100).
    critical: AeronPublication,
    /// High priority publication (stream 200).
    high: AeronPublication,
    /// Normal priority publication (stream 300).
    normal: AeronPublication,
}

impl PriorityPublisher {
    /// Create priority-aware publisher for **request** streams (client→server).
    ///
    /// Publishes to streams 100 (critical), 200 (high), 300 (normal).
    /// Use this on the **client side** to send prioritized requests to the server.
    ///
    /// # Arguments
    /// * `ctx` - Aeron client context (must outlive this publisher)
    /// * `channel` - Aeron channel URI (e.g., `"aeron:ipc"` or `"aeron:udp?endpoint=0.0.0.0:20121"`)
    /// * `timeout` - Maximum time to wait for each stream to register with the Media Driver
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::Transport`] if:
    /// - Any of the 3 publications fails to register within `timeout`
    /// - The channel URI is invalid
    /// - The Aeron Media Driver is not running
    pub fn new_for_requests(
        ctx: &AeronContext,
        channel: &str,
        timeout: Duration,
    ) -> BlazerResult<Self> {
        tracing::info!(
            channel,
            "Creating priority-aware request publisher (client side)"
        );

        let critical = AeronPublication::new(ctx, channel, STREAM_CRITICAL_REQ, timeout)?;
        tracing::debug!(
            stream_id = STREAM_CRITICAL_REQ,
            "Critical request publication registered"
        );

        let high = AeronPublication::new(ctx, channel, STREAM_HIGH_REQ, timeout)?;
        tracing::debug!(
            stream_id = STREAM_HIGH_REQ,
            "High request publication registered"
        );

        let normal = AeronPublication::new(ctx, channel, STREAM_NORMAL_REQ, timeout)?;
        tracing::debug!(
            stream_id = STREAM_NORMAL_REQ,
            "Normal request publication registered"
        );

        tracing::info!("Priority-aware request publisher initialized");

        Ok(Self {
            critical,
            high,
            normal,
        })
    }

    /// Create priority-aware publisher for **response** streams (server→client).
    ///
    /// Publishes to streams 101 (critical), 201 (high), 301 (normal).
    /// Use this on the **server side** to send prioritized responses to clients.
    ///
    /// # Arguments
    /// * `ctx` - Aeron client context (must outlive this publisher)
    /// * `channel` - Aeron channel URI (e.g., `"aeron:ipc"` or `"aeron:udp?endpoint=0.0.0.0:20121"`)
    /// * `timeout` - Maximum time to wait for each stream to register with the Media Driver
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::Transport`] if:
    /// - Any of the 3 publications fails to register within `timeout`
    /// - The channel URI is invalid
    /// - The Aeron Media Driver is not running
    pub fn new_for_responses(
        ctx: &AeronContext,
        channel: &str,
        timeout: Duration,
    ) -> BlazerResult<Self> {
        tracing::info!(
            channel,
            "Creating priority-aware response publisher (server side)"
        );

        let critical = AeronPublication::new(ctx, channel, STREAM_CRITICAL_RSP, timeout)?;
        tracing::debug!(
            stream_id = STREAM_CRITICAL_RSP,
            "Critical response publication registered"
        );

        let high = AeronPublication::new(ctx, channel, STREAM_HIGH_RSP, timeout)?;
        tracing::debug!(
            stream_id = STREAM_HIGH_RSP,
            "High response publication registered"
        );

        let normal = AeronPublication::new(ctx, channel, STREAM_NORMAL_RSP, timeout)?;
        tracing::debug!(
            stream_id = STREAM_NORMAL_RSP,
            "Normal response publication registered"
        );

        tracing::info!("Priority-aware response publisher initialized");

        Ok(Self {
            critical,
            high,
            normal,
        })
    }

    /// Offer data to subscribers with the specified priority.
    ///
    /// Routes the message to the appropriate stream based on `priority`:
    /// - `Critical` → stream 100
    /// - `High` → stream 200
    /// - `Normal` → stream 300
    ///
    /// # Arguments
    /// * `priority` - Event priority level (determines which stream to use)
    /// * `data` - Payload bytes to publish
    ///
    /// # Returns
    ///
    /// On success, returns the stream position where the message was written.
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::Transport`] if:
    /// - No subscriber is connected to the selected stream
    /// - The publication is closed
    /// - Back-pressure persists beyond the internal timeout (50ms)
    /// - An unrecoverable Aeron error occurs
    ///
    /// # Performance
    ///
    /// Each priority stream has independent backpressure. Critical messages
    /// are never blocked by congestion on normal streams.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use blazil_transport::aeron::PriorityPublisher;
    /// # use blazil_transport::EventPriority;
    /// # fn example(publisher: &PriorityPublisher) -> blazil_common::error::BlazerResult<()> {
    /// // Emergency: margin call requires immediate liquidation
    /// let margin_call = b"MARGIN_CALL:account=12345,amount=1000000";
    /// let pos = publisher.offer(EventPriority::Critical, margin_call)?;
    /// tracing::info!(position = pos, "Margin call published");
    ///
    /// // Standard: regular customer transaction
    /// let transaction = b"TXN:from=A,to=B,amount=100";
    /// publisher.offer(EventPriority::Normal, transaction)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn offer(&self, priority: EventPriority, data: &[u8]) -> BlazerResult<i64> {
        let publication = match priority {
            EventPriority::Critical => &self.critical,
            EventPriority::High => &self.high,
            EventPriority::Normal => &self.normal,
        };

        publication.offer(data)
    }

    /// Check if at least one subscriber is connected to the critical stream.
    ///
    /// Useful for health checks and monitoring. If no subscriber is connected,
    /// [`offer`](Self::offer) will return an error.
    pub fn is_critical_connected(&self) -> bool {
        self.critical.is_connected()
    }

    /// Check if at least one subscriber is connected to the high priority stream.
    pub fn is_high_connected(&self) -> bool {
        self.high.is_connected()
    }

    /// Check if at least one subscriber is connected to the normal priority stream.
    pub fn is_normal_connected(&self) -> bool {
        self.normal.is_connected()
    }

    /// Check if all priority streams have at least one subscriber.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use blazil_transport::aeron::PriorityPublisher;
    /// # fn example(publisher: &PriorityPublisher) {
    /// if !publisher.is_all_connected() {
    ///     tracing::warn!("Some priority streams have no subscribers");
    /// }
    /// # }
    /// ```
    pub fn is_all_connected(&self) -> bool {
        self.critical.is_connected() && self.high.is_connected() && self.normal.is_connected()
    }
}

impl Drop for PriorityPublisher {
    fn drop(&mut self) {
        tracing::debug!("Closing priority-aware Aeron publisher");
        // Publications are automatically closed by their Drop implementations
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_routing() {
        // This test verifies that EventPriority correctly maps to stream IDs.
        // Actual Aeron integration tests require a running Media Driver and are
        // in the tests/ directory.

        assert_eq!(
            EventPriority::Critical.request_stream_id(),
            STREAM_CRITICAL_REQ
        );
        assert_eq!(EventPriority::High.request_stream_id(), STREAM_HIGH_REQ);
        assert_eq!(EventPriority::Normal.request_stream_id(), STREAM_NORMAL_REQ);

        assert_eq!(STREAM_CRITICAL_REQ, 100);
        assert_eq!(STREAM_HIGH_REQ, 200);
        assert_eq!(STREAM_NORMAL_REQ, 300);
    }
}
