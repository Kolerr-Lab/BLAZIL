//! Audit Event Types — Transaction Lifecycle Events

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

/// Audit action performed in the system
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    /// Transaction created in the engine
    TransactionCreated,
    /// Transaction validated (pre-flight checks)
    TransactionValidated,
    /// Compliance screening initiated
    ComplianceScreeningStarted,
    /// Compliance screening completed
    ComplianceScreeningCompleted,
    /// Transaction submitted to ledger
    LedgerSubmitted,
    /// Ledger commit confirmed
    LedgerCommitted,
    /// Transaction completed successfully
    TransactionCompleted,
    /// Transaction rejected
    TransactionRejected,
    /// Transaction held for review
    TransactionHeld,
    /// Transaction released from hold
    TransactionReleased,
    /// SAR (Suspicious Activity Report) generated
    SarGenerated,
    /// Access control check performed
    AccessControlCheck,
    /// API authentication performed
    ApiAuthentication,
    /// Configuration changed
    ConfigurationChanged,
}

/// Result of the audited action
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditResult {
    Success,
    Failure,
    Pending,
}

/// Structured audit event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unique event ID
    pub event_id: Uuid,
    /// Timestamp when event occurred
    pub timestamp: DateTime<Utc>,
    /// Transaction ID (if applicable)
    pub transaction_id: String,
    /// Actor performing the action (user ID, service name, API key)
    pub actor: String,
    /// Action performed
    pub action: AuditAction,
    /// Result of the action
    pub result: AuditResult,
    /// Latency of the operation (nanoseconds)
    pub latency_ns: Option<u64>,
    /// Additional metadata (JSON-serializable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// Error message (if result is Failure)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl AuditEvent {
    /// Create a new audit event
    pub fn new(transaction_id: String, actor: String, action: AuditAction) -> Self {
        Self {
            event_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            transaction_id,
            actor,
            action,
            result: AuditResult::Pending,
            latency_ns: None,
            metadata: None,
            error: None,
        }
    }

    /// Set result to success
    pub fn with_result(mut self, result: &str) -> Self {
        self.result = match result {
            "success" => AuditResult::Success,
            "failure" => AuditResult::Failure,
            _ => AuditResult::Pending,
        };
        self
    }

    /// Set latency
    pub fn with_latency(mut self, duration: Duration) -> Self {
        self.latency_ns = Some(duration.as_nanos() as u64);
        self
    }

    /// Set metadata
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Set error message
    pub fn with_error(mut self, error: String) -> Self {
        self.error = Some(error);
        self.result = AuditResult::Failure;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_event_creation() {
        let event = AuditEvent::new(
            "tx_12345".to_string(),
            "user_alice".to_string(),
            AuditAction::TransactionCreated,
        );

        assert_eq!(event.transaction_id, "tx_12345");
        assert_eq!(event.actor, "user_alice");
        assert_eq!(event.action, AuditAction::TransactionCreated);
        assert_eq!(event.result, AuditResult::Pending);
    }

    #[test]
    fn test_audit_event_with_result() {
        let event = AuditEvent::new(
            "tx_12345".to_string(),
            "user_alice".to_string(),
            AuditAction::TransactionCreated,
        )
        .with_result("success");

        assert_eq!(event.result, AuditResult::Success);
    }

    #[test]
    fn test_audit_event_with_latency() {
        let event = AuditEvent::new(
            "tx_12345".to_string(),
            "user_alice".to_string(),
            AuditAction::TransactionCreated,
        )
        .with_latency(Duration::from_millis(10));

        assert!(event.latency_ns.is_some());
        assert_eq!(event.latency_ns.unwrap(), 10_000_000);
    }

    #[test]
    fn test_audit_event_serialization() {
        let event = AuditEvent::new(
            "tx_12345".to_string(),
            "user_alice".to_string(),
            AuditAction::TransactionCreated,
        )
        .with_result("success");

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"transaction_id\":\"tx_12345\""));
        assert!(json.contains("\"actor\":\"user_alice\""));
    }
}
