//! Integration tests for audit logging

use super::*;
use std::time::Duration;

#[tokio::test]
async fn test_full_transaction_lifecycle() {
    let log = AuditLog::new();

    // Transaction created
    log.record(
        AuditEvent::new(
            "tx_lifecycle".to_string(),
            "user_alice".to_string(),
            AuditAction::TransactionCreated,
        )
        .with_result("success")
        .with_latency(Duration::from_micros(100)),
    )
    .await;

    // Transaction validated
    log.record(
        AuditEvent::new(
            "tx_lifecycle".to_string(),
            "validator_service".to_string(),
            AuditAction::TransactionValidated,
        )
        .with_result("success")
        .with_latency(Duration::from_micros(50)),
    )
    .await;

    // Compliance screening
    log.record(
        AuditEvent::new(
            "tx_lifecycle".to_string(),
            "compliance_engine".to_string(),
            AuditAction::ComplianceScreeningStarted,
        )
        .with_result("success"),
    )
    .await;

    log.record(
        AuditEvent::new(
            "tx_lifecycle".to_string(),
            "compliance_engine".to_string(),
            AuditAction::ComplianceScreeningCompleted,
        )
        .with_result("success")
        .with_latency(Duration::from_millis(30)),
    )
    .await;

    // Ledger operations
    log.record(
        AuditEvent::new(
            "tx_lifecycle".to_string(),
            "ledger_client".to_string(),
            AuditAction::LedgerSubmitted,
        )
        .with_result("success")
        .with_latency(Duration::from_micros(500)),
    )
    .await;

    log.record(
        AuditEvent::new(
            "tx_lifecycle".to_string(),
            "tigerbeetle".to_string(),
            AuditAction::LedgerCommitted,
        )
        .with_result("success")
        .with_latency(Duration::from_millis(5)),
    )
    .await;

    // Transaction completed
    log.record(
        AuditEvent::new(
            "tx_lifecycle".to_string(),
            "engine".to_string(),
            AuditAction::TransactionCompleted,
        )
        .with_result("success")
        .with_latency(Duration::from_millis(35)),
    )
    .await;

    // Verify all events recorded
    let tx_events = log.get_by_transaction("tx_lifecycle").await;
    assert_eq!(tx_events.len(), 7);

    // Verify chain integrity
    assert!(log.verify_integrity().await);
}

#[tokio::test]
async fn test_compliance_hold_and_release() {
    let log = AuditLog::new();

    // Transaction created
    log.record(
        AuditEvent::new(
            "tx_suspicious".to_string(),
            "user_bob".to_string(),
            AuditAction::TransactionCreated,
        )
        .with_result("success"),
    )
    .await;

    // Compliance screening detects issue
    log.record(
        AuditEvent::new(
            "tx_suspicious".to_string(),
            "compliance_engine".to_string(),
            AuditAction::ComplianceScreeningCompleted,
        )
        .with_result("success")
        .with_metadata(serde_json::json!({
            "risk_score": 85,
            "reason": "High velocity transaction pattern"
        })),
    )
    .await;

    // Transaction held
    log.record(
        AuditEvent::new(
            "tx_suspicious".to_string(),
            "compliance_officer".to_string(),
            AuditAction::TransactionHeld,
        )
        .with_result("success")
        .with_metadata(serde_json::json!({
            "review_required": true,
            "assigned_to": "officer_123"
        })),
    )
    .await;

    // After review, transaction released
    log.record(
        AuditEvent::new(
            "tx_suspicious".to_string(),
            "compliance_officer".to_string(),
            AuditAction::TransactionReleased,
        )
        .with_result("success")
        .with_metadata(serde_json::json!({
            "reviewed_by": "officer_123",
            "decision": "cleared"
        })),
    )
    .await;

    let events = log.get_by_transaction("tx_suspicious").await;
    assert_eq!(events.len(), 4);

    // Find held and released events
    let held = events
        .iter()
        .find(|e| matches!(e.event.action, AuditAction::TransactionHeld));
    let released = events
        .iter()
        .find(|e| matches!(e.event.action, AuditAction::TransactionReleased));

    assert!(held.is_some());
    assert!(released.is_some());
}

#[tokio::test]
async fn test_sar_generation() {
    let log = AuditLog::new();

    log.record(
        AuditEvent::new(
            "tx_money_laundering".to_string(),
            "compliance_engine".to_string(),
            AuditAction::SarGenerated,
        )
        .with_result("success")
        .with_metadata(serde_json::json!({
            "sar_id": "SAR-2026-05-09-001",
            "risk_level": "critical",
            "indicators": ["structuring", "high_velocity", "cross_border"],
            "filing_required": true
        })),
    )
    .await;

    let events = log.get_by_transaction("tx_money_laundering").await;
    assert_eq!(events.len(), 1);

    let sar_event = &events[0];
    assert!(matches!(sar_event.event.action, AuditAction::SarGenerated));
    assert!(sar_event.event.metadata.is_some());
}

#[tokio::test]
async fn test_access_control_audit() {
    let log = AuditLog::new();

    // Successful authentication
    log.record(
        AuditEvent::new(
            "api_request_1".to_string(),
            "api_key_xyz".to_string(),
            AuditAction::ApiAuthentication,
        )
        .with_result("success")
        .with_metadata(serde_json::json!({
            "ip_address": "192.168.1.100",
            "endpoint": "/api/transactions"
        })),
    )
    .await;

    // Failed authentication
    log.record(
        AuditEvent::new(
            "api_request_2".to_string(),
            "invalid_key".to_string(),
            AuditAction::ApiAuthentication,
        )
        .with_error("Invalid API key".to_string())
        .with_metadata(serde_json::json!({
            "ip_address": "10.0.0.50",
            "endpoint": "/api/transactions"
        })),
    )
    .await;

    // Access control check
    log.record(
        AuditEvent::new(
            "api_request_1".to_string(),
            "api_key_xyz".to_string(),
            AuditAction::AccessControlCheck,
        )
        .with_result("success")
        .with_metadata(serde_json::json!({
            "resource": "/api/transactions",
            "permission": "write",
            "granted": true
        })),
    )
    .await;

    assert_eq!(log.len().await, 3);

    let auth_events = log.get_by_actor("api_key_xyz").await;
    assert_eq!(auth_events.len(), 2);
}

#[tokio::test]
async fn test_export_formats() {
    let log = AuditLog::new();

    log.record(
        AuditEvent::new(
            "tx_export_test".to_string(),
            "user_test".to_string(),
            AuditAction::TransactionCreated,
        )
        .with_result("success")
        .with_latency(Duration::from_micros(1500)),
    )
    .await;

    // Test JSON export
    let json = LogExporter::export(&log, ExportFormat::Json, None, None).await;
    assert!(json.contains("tx_export_test"));
    assert!(json.contains("1500000"));
    assert!(json.contains("transaction_id"));

    // Test CEF export
    let cef = LogExporter::export(&log, ExportFormat::Cef, None, None).await;
    assert!(cef.contains("CEF:0|Blazil|"));
    assert!(cef.contains("txId=tx_export_test"));
    assert!(cef.contains("latency=1500000"));
}
