//! Priority-based event routing for Aeron transport.
//!
//! Blazil supports multi-stream priority routing to ensure critical events
//! (e.g., margin calls, fraud alerts) bypass normal traffic congestion.
//!
//! # Architecture
//!
//! ```text
//! Producer → Route by Priority → Aeron Multi-Stream → Priority Poller → Pipeline
//!                                       ↓
//!                        ┌──────────────┼──────────────┐
//!                        ▼              ▼              ▼
//!                   CRITICAL         HIGH          NORMAL
//!                  (Stream 1000) (Stream 2000) (Stream 3000)
//!                        │              │              │
//!                        └──────────────┼──────────────┘
//!                                       ▼
//!                              Poll in priority order:
//!                              1. Critical first
//!                              2. High second
//!                              3. Normal last
//! ```
//!
//! # Usage
//!
//! ```rust
//! use blazil_transport::priority::{EventPriority, STREAM_CRITICAL_REQ, STREAM_HIGH_REQ, STREAM_NORMAL_REQ};
//!
//! // Route events by priority
//! let priority = EventPriority::Critical;
//! let stream_id = match priority {
//!     EventPriority::Critical => STREAM_CRITICAL_REQ,
//!     EventPriority::High => STREAM_HIGH_REQ,
//!     EventPriority::Normal => STREAM_NORMAL_REQ,
//! };
//! assert_eq!(stream_id, 100);
//! ```
//!
//! # Performance
//!
//! - **Critical events**: <1ms latency (bypasses all other traffic)
//! - **High priority**: <5ms latency (processed after critical)
//! - **Normal traffic**: Unchanged performance when no high-priority events
//!
//! # Stream ID allocation
//!
//! | Priority | Request Stream | Response Stream | Typical Use Cases |
//! |----------|----------------|-----------------|-------------------|
//! | Critical | 100 | 101 | Margin calls, circuit breakers, fraud alerts |
//! | High | 200 | 201 | Large transactions, VIP customers, time-sensitive orders |
//! | Normal | 300 | 301 | Standard transactions, batch operations, analytics |
//! | Legacy | 1001 | 1002 | Backwards compatibility (maps to Normal priority) |
//!
//! Stream IDs use simple patterns (100s, 200s, 300s) for clarity and to avoid
//! collisions with legacy single-stream setup (1001/1002).

use serde::{Deserialize, Serialize};
use std::fmt;

// ── Stream ID Constants ───────────────────────────────────────────────────────

/// Critical priority request stream (highest priority).
///
/// Use for emergency events that must bypass all traffic:
/// - Margin calls / liquidation triggers
/// - Fraud detection alerts
/// - Circuit breaker activations
/// - Compliance violations
/// - System health critical failures
pub const STREAM_CRITICAL_REQ: i32 = 100;

/// Critical priority response stream.
pub const STREAM_CRITICAL_RSP: i32 = 101;

/// High priority request stream.
///
/// Use for important but not emergency events:
/// - Large transactions (>$1M)
/// - VIP customer requests
/// - Time-sensitive market orders
/// - Real-time risk checks
pub const STREAM_HIGH_REQ: i32 = 200;

/// High priority response stream.
pub const STREAM_HIGH_RSP: i32 = 201;

/// Normal priority request stream (default).
///
/// Use for standard traffic:
/// - Regular transactions
/// - Batch operations
/// - Analytics queries
/// - Historical data requests
pub const STREAM_NORMAL_REQ: i32 = 300;

/// Normal priority response stream.
pub const STREAM_NORMAL_RSP: i32 = 301;

// Legacy single-stream constants for backwards compatibility
/// Legacy request stream ID (used before priority routing).
pub const STREAM_LEGACY_REQ: i32 = 1001;

/// Legacy response stream ID (used before priority routing).
pub const STREAM_LEGACY_RSP: i32 = 1002;

// ── EventPriority ─────────────────────────────────────────────────────────────

/// Event priority level for routing and processing.
///
/// Determines which Aeron stream an event is published to and the polling
/// order in the pipeline.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub enum EventPriority {
    /// Emergency events that must bypass all traffic.
    ///
    /// **Latency target**: <1ms end-to-end
    ///
    /// **Examples**:
    /// - Margin calls requiring immediate liquidation
    /// - Fraud detection requiring account freeze
    /// - Circuit breaker activation
    /// - Critical compliance violations
    Critical = 0,

    /// Important events processed before normal traffic.
    ///
    /// **Latency target**: <5ms end-to-end
    ///
    /// **Examples**:
    /// - Large transactions (>$1M)
    /// - VIP customer requests
    /// - Time-sensitive market orders
    /// - Real-time risk threshold breaches
    High = 1,

    /// Standard traffic (default priority).
    ///
    /// **Latency target**: <50ms end-to-end (same as current)
    ///
    /// **Examples**:
    /// - Regular customer transactions
    /// - Batch operations
    /// - Analytics queries
    /// - Background reconciliation
    #[default]
    Normal = 2,
}

impl EventPriority {
    /// Get the request stream ID for this priority level.
    #[inline]
    pub const fn request_stream_id(self) -> i32 {
        match self {
            EventPriority::Critical => STREAM_CRITICAL_REQ,
            EventPriority::High => STREAM_HIGH_REQ,
            EventPriority::Normal => STREAM_NORMAL_REQ,
        }
    }

    /// Get the response stream ID for this priority level.
    #[inline]
    pub const fn response_stream_id(self) -> i32 {
        match self {
            EventPriority::Critical => STREAM_CRITICAL_RSP,
            EventPriority::High => STREAM_HIGH_RSP,
            EventPriority::Normal => STREAM_NORMAL_RSP,
        }
    }

    /// Parse priority from request stream ID.
    ///
    /// Returns `None` if the stream ID doesn't match any known priority level.
    pub fn from_request_stream_id(stream_id: i32) -> Option<Self> {
        match stream_id {
            STREAM_CRITICAL_REQ => Some(EventPriority::Critical),
            STREAM_HIGH_REQ => Some(EventPriority::High),
            STREAM_NORMAL_REQ => Some(EventPriority::Normal),
            STREAM_LEGACY_REQ => Some(EventPriority::Normal), // Legacy maps to Normal
            _ => None,
        }
    }

    /// Parse priority from response stream ID.
    ///
    /// Returns `None` if the stream ID doesn't match any known priority level.
    pub fn from_response_stream_id(stream_id: i32) -> Option<Self> {
        match stream_id {
            STREAM_CRITICAL_RSP => Some(EventPriority::Critical),
            STREAM_HIGH_RSP => Some(EventPriority::High),
            STREAM_NORMAL_RSP => Some(EventPriority::Normal),
            STREAM_LEGACY_RSP => Some(EventPriority::Normal), // Legacy maps to Normal
            _ => None,
        }
    }

    /// Get human-readable name for logging/metrics.
    #[inline]
    pub const fn name(self) -> &'static str {
        match self {
            EventPriority::Critical => "critical",
            EventPriority::High => "high",
            EventPriority::Normal => "normal",
        }
    }

    /// Get emoji indicator for terminal output (debugging/logs).
    #[inline]
    pub const fn emoji(self) -> &'static str {
        match self {
            EventPriority::Critical => "🚨",
            EventPriority::High => "⚡",
            EventPriority::Normal => "📦",
        }
    }
}

impl fmt::Display for EventPriority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_ordering() {
        // Critical < High < Normal (lower enum value = higher priority)
        assert!(EventPriority::Critical < EventPriority::High);
        assert!(EventPriority::High < EventPriority::Normal);
    }

    #[test]
    fn test_stream_id_mapping() {
        // Request streams
        assert_eq!(
            EventPriority::Critical.request_stream_id(),
            STREAM_CRITICAL_REQ
        );
        assert_eq!(EventPriority::High.request_stream_id(), STREAM_HIGH_REQ);
        assert_eq!(EventPriority::Normal.request_stream_id(), STREAM_NORMAL_REQ);

        // Response streams
        assert_eq!(
            EventPriority::Critical.response_stream_id(),
            STREAM_CRITICAL_RSP
        );
        assert_eq!(EventPriority::High.response_stream_id(), STREAM_HIGH_RSP);
        assert_eq!(
            EventPriority::Normal.response_stream_id(),
            STREAM_NORMAL_RSP
        );
    }

    #[test]
    fn test_parse_from_stream_id() {
        // Request streams
        assert_eq!(
            EventPriority::from_request_stream_id(STREAM_CRITICAL_REQ),
            Some(EventPriority::Critical)
        );
        assert_eq!(
            EventPriority::from_request_stream_id(STREAM_HIGH_REQ),
            Some(EventPriority::High)
        );
        assert_eq!(
            EventPriority::from_request_stream_id(STREAM_NORMAL_REQ),
            Some(EventPriority::Normal)
        );
        assert_eq!(
            EventPriority::from_request_stream_id(STREAM_LEGACY_REQ),
            Some(EventPriority::Normal)
        );
        assert_eq!(EventPriority::from_request_stream_id(9999), None);

        // Response streams
        assert_eq!(
            EventPriority::from_response_stream_id(STREAM_CRITICAL_RSP),
            Some(EventPriority::Critical)
        );
        assert_eq!(
            EventPriority::from_response_stream_id(STREAM_HIGH_RSP),
            Some(EventPriority::High)
        );
        assert_eq!(
            EventPriority::from_response_stream_id(STREAM_NORMAL_RSP),
            Some(EventPriority::Normal)
        );
        assert_eq!(
            EventPriority::from_response_stream_id(STREAM_LEGACY_RSP),
            Some(EventPriority::Normal)
        );
        assert_eq!(EventPriority::from_response_stream_id(9999), None);
    }

    #[test]
    fn test_default_priority() {
        assert_eq!(EventPriority::default(), EventPriority::Normal);
    }

    #[test]
    fn test_display() {
        assert_eq!(EventPriority::Critical.to_string(), "critical");
        assert_eq!(EventPriority::High.to_string(), "high");
        assert_eq!(EventPriority::Normal.to_string(), "normal");
    }

    #[test]
    fn test_stream_id_no_collisions() {
        // Ensure no stream ID collisions
        let all_req_streams = [
            STREAM_CRITICAL_REQ,
            STREAM_HIGH_REQ,
            STREAM_NORMAL_REQ,
            STREAM_LEGACY_REQ,
        ];
        let all_rsp_streams = [
            STREAM_CRITICAL_RSP,
            STREAM_HIGH_RSP,
            STREAM_NORMAL_RSP,
            STREAM_LEGACY_RSP,
        ];

        // Check uniqueness
        let mut seen = std::collections::HashSet::new();
        for &id in all_req_streams.iter().chain(all_rsp_streams.iter()) {
            assert!(seen.insert(id), "Duplicate stream ID: {id}");
        }
    }
}
