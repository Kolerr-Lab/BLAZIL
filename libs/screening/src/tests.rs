// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

// This file is already `mod tests` (declared in lib.rs with `#[cfg(test)]`).
// No inner module wrapper needed.

use std::sync::Arc;

use chrono::Utc;
use tokio::time::{sleep, Duration};

use crate::{
    batch::BatchWorker,
    hold::{HoldRecord, HoldStore, InMemoryHoldStore},
    mock::MockScreener,
    realtime::RealTimeRouter,
    sar::{SarReport, SuspiciousActivityType},
    RiskLevel, ScreeningError, ScreeningMode, ScreeningResult, TransactionEvent,
    TransactionScreener,
};

// ── Test-only helpers on SarReport ────────────────────────────────────────────

impl SarReport {
    fn is_empty_narrative(&self) -> bool {
        self.narrative.is_empty()
    }
}

// ── Shared test helpers ────────────────────────────────────────────────────────

fn usd(id: &str, cents: u64) -> TransactionEvent {
    TransactionEvent::new(id, cents, "USD", "sender_alice", "receiver_bob")
}

fn make_hold(tx_id: &str) -> HoldRecord {
    HoldRecord {
        transaction_id: tx_id.to_string(),
        reason: "structuring detected".to_string(),
        review_required: true,
        held_at: Utc::now(),
        released_at: None,
    }
}

// ── TransactionEvent ───────────────────────────────────────────────────────────

#[test]
fn test_transaction_event_fields() {
    // 500_000 cents = $5,000.00 SGD
    let tx = TransactionEvent::new("tx_001", 500_000, "SGD", "alice", "bob");
    assert_eq!(tx.transaction_id, "tx_001");
    assert_eq!(tx.amount, 500_000);
    assert_eq!(tx.currency, "SGD");
    assert_eq!(tx.sender_id, "alice");
    assert_eq!(tx.receiver_id, "bob");
    assert!(tx.metadata.is_empty());
}

#[test]
fn test_transaction_event_with_metadata() {
    let tx = TransactionEvent::new("tx_002", 100, "USD", "alice", "bob")
        .with_metadata("ip", "10.0.0.1")
        .with_metadata("device", "mobile");
    assert_eq!(tx.metadata.get("ip").map(String::as_str), Some("10.0.0.1"));
    assert_eq!(
        tx.metadata.get("device").map(String::as_str),
        Some("mobile")
    );
}

// ── ScreeningResult helpers ────────────────────────────────────────────────────

#[test]
fn test_screening_result_clear_helpers() {
    let r = ScreeningResult::Clear;
    assert!(r.is_clear());
    assert!(!r.is_blocked());
    assert!(!r.requires_sar());
}

#[test]
fn test_screening_result_flag_helpers() {
    let r = ScreeningResult::Flag {
        reason: "test".into(),
        severity: RiskLevel::Low,
    };
    assert!(!r.is_clear());
    assert!(!r.is_blocked());
    assert!(!r.requires_sar());
}

#[test]
fn test_screening_result_hold_helpers() {
    let r = ScreeningResult::Hold {
        reason: "test".into(),
        review_required: true,
    };
    assert!(!r.is_clear());
    assert!(r.is_blocked());
    assert!(!r.requires_sar());
}

#[test]
fn test_screening_result_reject_sar_required() {
    let r = ScreeningResult::Reject {
        reason: "test".into(),
        sar_required: true,
    };
    assert!(!r.is_clear());
    assert!(r.is_blocked());
    assert!(r.requires_sar());
}

#[test]
fn test_screening_result_reject_no_sar() {
    let r = ScreeningResult::Reject {
        reason: "test".into(),
        sar_required: false,
    };
    assert!(r.is_blocked());
    assert!(!r.requires_sar());
}

// ── RiskLevel ─────────────────────────────────────────────────────────────────

#[test]
fn test_risk_level_ordering() {
    assert!(RiskLevel::Low < RiskLevel::Medium);
    assert!(RiskLevel::Medium < RiskLevel::High);
    assert!(RiskLevel::High < RiskLevel::Critical);
}

// ── MockScreener ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_mock_clear_below_flag_threshold() {
    let screener = MockScreener::new();
    let tx = usd("tx_010", 999_999); // 1 cent below $10,000 flag threshold
    assert_eq!(
        screener.screen(&tx, ScreeningMode::RealTime).await,
        ScreeningResult::Clear
    );
}

#[tokio::test]
async fn test_mock_flag_at_threshold() {
    let screener = MockScreener::new();
    let tx = usd("tx_011", 1_000_000); // exactly $10,000.00
    assert!(matches!(
        screener.screen(&tx, ScreeningMode::RealTime).await,
        ScreeningResult::Flag {
            severity: RiskLevel::High,
            ..
        }
    ));
}

#[tokio::test]
async fn test_mock_flag_between_thresholds() {
    let screener = MockScreener::new();
    let tx = usd("tx_012", 2_500_000); // $25,000 — above flag, below reject
    assert!(matches!(
        screener.screen(&tx, ScreeningMode::RealTime).await,
        ScreeningResult::Flag { .. }
    ));
}

#[tokio::test]
async fn test_mock_reject_at_threshold() {
    let screener = MockScreener::new();
    let tx = usd("tx_013", 5_000_000); // exactly $50,000.00
    assert!(matches!(
        screener.screen(&tx, ScreeningMode::RealTime).await,
        ScreeningResult::Reject {
            sar_required: true,
            ..
        }
    ));
}

#[tokio::test]
async fn test_mock_reject_above_threshold() {
    let screener = MockScreener::new();
    let tx = usd("tx_014", 10_000_000); // $100,000.00
    assert!(matches!(
        screener.screen(&tx, ScreeningMode::RealTime).await,
        ScreeningResult::Reject {
            sar_required: true,
            ..
        }
    ));
}

#[tokio::test]
async fn test_mock_blocklist_reject_overrides_amount() {
    let screener = MockScreener::new().with_blocklist(["bad_actor"]);
    let tx = TransactionEvent::new("tx_015", 100, "USD", "bad_actor", "receiver");
    assert!(matches!(
        screener.screen(&tx, ScreeningMode::RealTime).await,
        ScreeningResult::Reject {
            sar_required: true,
            ..
        }
    ));
}

#[tokio::test]
async fn test_mock_blocklist_does_not_affect_other_senders() {
    let screener = MockScreener::new().with_blocklist(["bad_actor"]);
    let tx = TransactionEvent::new("tx_016", 100, "USD", "good_actor", "receiver");
    assert_eq!(
        screener.screen(&tx, ScreeningMode::RealTime).await,
        ScreeningResult::Clear
    );
}

#[tokio::test]
async fn test_mock_custom_thresholds() {
    // 100 cents = $1.00 flag / 500 cents = $5.00 reject
    let screener = MockScreener::with_thresholds(100, 500);
    assert!(matches!(
        screener
            .screen(&usd("tx_017", 200), ScreeningMode::Batch)
            .await,
        ScreeningResult::Flag { .. }
    ));
    assert!(matches!(
        screener
            .screen(&usd("tx_018", 500), ScreeningMode::Batch)
            .await,
        ScreeningResult::Reject { .. }
    ));
    assert_eq!(
        screener
            .screen(&usd("tx_019", 99), ScreeningMode::Batch)
            .await,
        ScreeningResult::Clear
    );
}

#[test]
fn test_mock_provider_name() {
    assert_eq!(MockScreener::new().provider_name(), "mock");
}

#[test]
fn test_mock_default() {
    let _ = MockScreener::default();
}

// ── RealTimeRouter ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_realtime_router_clear_no_timeout() {
    let router = RealTimeRouter::new(Arc::new(MockScreener::new()));
    let (result, timed_out) = router.screen(&usd("tx_020", 100)).await;
    assert_eq!(result, ScreeningResult::Clear);
    assert!(!timed_out);
}

#[tokio::test]
async fn test_realtime_router_flag_no_timeout() {
    let router = RealTimeRouter::new(Arc::new(MockScreener::new()));
    let (result, timed_out) = router.screen(&usd("tx_021", 1_500_000)).await; // $15,000
    assert!(matches!(result, ScreeningResult::Flag { .. }));
    assert!(!timed_out);
}

#[tokio::test]
async fn test_realtime_router_timeout_falls_back_to_clear() {
    struct AlwaysSlowScreener;

    #[async_trait::async_trait]
    impl TransactionScreener for AlwaysSlowScreener {
        async fn screen(&self, _tx: &TransactionEvent, _mode: ScreeningMode) -> ScreeningResult {
            sleep(Duration::from_millis(200)).await;
            ScreeningResult::Reject {
                reason: "slow".into(),
                sar_required: false,
            }
        }

        fn provider_name(&self) -> &'static str {
            "always_slow"
        }
    }

    let router = RealTimeRouter::new(Arc::new(AlwaysSlowScreener));
    let (result, timed_out) = router.screen(&usd("tx_022", 100)).await;

    assert_eq!(
        result,
        ScreeningResult::Clear,
        "timeout must fall back to Clear (fail-open)"
    );
    assert!(
        timed_out,
        "must report timeout so caller can enqueue for batch"
    );
}

// ── BatchWorker ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_batch_worker_single_job() {
    let screener = Arc::new(MockScreener::new());
    let (worker, sender) = BatchWorker::new(screener, 64);
    tokio::spawn(worker.run());

    // 5_000_000 cents = $50,000 — at reject threshold
    let rx = sender
        .submit(usd("tx_030", 5_000_000))
        .await
        .expect("submit must succeed");
    let result = rx.await.expect("result channel must not close prematurely");

    assert!(matches!(
        result,
        ScreeningResult::Reject {
            sar_required: true,
            ..
        }
    ));
}

#[tokio::test]
async fn test_batch_worker_concurrent_jobs() {
    let screener = Arc::new(MockScreener::new());
    let (worker, sender) = BatchWorker::new(screener, 256);
    tokio::spawn(worker.run());

    let mut receivers = Vec::with_capacity(50);
    for i in 0u64..50 {
        let tx_id = format!("tx_batch_{i}");
        // i * 100_000 cents = i * $1,000
        let rx = sender
            .submit(usd(&tx_id, i * 100_000))
            .await
            .expect("submit must succeed");
        receivers.push(rx);
    }

    for rx in receivers {
        rx.await.expect("result channel must not close prematurely");
    }
}

#[tokio::test]
async fn test_batch_sender_closed_channel_returns_error() {
    let screener = Arc::new(MockScreener::new());
    let (worker, sender) = BatchWorker::new(screener, 1);
    drop(worker); // Close receiver side immediately.

    let result = sender.submit(usd("tx_040", 100)).await;
    assert!(
        matches!(result, Err(ScreeningError::BatchChannelClosed)),
        "must return BatchChannelClosed when worker has exited"
    );
}

// ── InMemoryHoldStore ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_hold_store_hold_and_release() {
    let store = InMemoryHoldStore::new();
    store
        .hold(make_hold("tx_050"))
        .await
        .expect("hold must succeed");

    let active = store.active_holds().await;
    assert_eq!(active.len(), 1);
    assert!(!active[0].is_released());

    let released = store.release("tx_050").await.expect("release must succeed");
    assert!(released.is_released());

    assert!(
        store.active_holds().await.is_empty(),
        "released hold must not appear in active list"
    );
}

#[tokio::test]
async fn test_hold_store_get_existing() {
    let store = InMemoryHoldStore::new();
    store.hold(make_hold("tx_051")).await.unwrap();
    assert!(store.get("tx_051").await.is_some());
}

#[tokio::test]
async fn test_hold_store_get_nonexistent() {
    let store = InMemoryHoldStore::new();
    assert!(store.get("does_not_exist").await.is_none());
}

#[tokio::test]
async fn test_hold_store_duplicate_hold_rejected() {
    let store = InMemoryHoldStore::new();
    store.hold(make_hold("tx_052")).await.unwrap();
    let err = store.hold(make_hold("tx_052")).await;
    assert!(
        err.is_err(),
        "second hold on same transaction must be rejected"
    );
}

#[tokio::test]
async fn test_hold_store_release_nonexistent_rejected() {
    let store = InMemoryHoldStore::new();
    assert!(
        store.release("never_held").await.is_err(),
        "release of unheld transaction must fail"
    );
}

#[tokio::test]
async fn test_hold_store_double_release_rejected() {
    let store = InMemoryHoldStore::new();
    store.hold(make_hold("tx_053")).await.unwrap();
    store.release("tx_053").await.unwrap();
    assert!(
        store.release("tx_053").await.is_err(),
        "second release must fail"
    );
}

#[tokio::test]
async fn test_hold_store_multiple_active_holds() {
    let store = InMemoryHoldStore::new();
    for i in 0..5u32 {
        let tx_id = format!("tx_multi_{i}");
        store.hold(make_hold(&tx_id)).await.unwrap();
    }
    assert_eq!(store.active_holds().await.len(), 5);

    store.release("tx_multi_2").await.unwrap();
    assert_eq!(
        store.active_holds().await.len(),
        4,
        "released hold must not appear in active list"
    );
}

// ── HoldRecord ────────────────────────────────────────────────────────────────

#[test]
fn test_hold_record_is_released() {
    let mut record = make_hold("tx_060");
    assert!(!record.is_released());
    record.released_at = Some(Utc::now());
    assert!(record.is_released());
}

// ── SarReport ─────────────────────────────────────────────────────────────────

#[test]
fn test_sar_xml_valid_structure() {
    // 7_500_000 cents = $75,000.00
    let tx = usd("tx_sar_001", 7_500_000);
    let report = SarReport::from_transaction(
        &tx,
        "Blazil Financial Inc.",
        SuspiciousActivityType::Structuring,
        "Multiple rapid transfers detected below the reporting threshold.",
    );

    let xml = String::from_utf8(report.to_xml().expect("XML generation must not fail"))
        .expect("XML must be valid UTF-8");

    assert!(xml.contains(r#"<?xml version="1.0" encoding="UTF-8"?>"#));
    assert!(xml.contains(r#"xmlns="urn:fincen:sar:2.0""#));
    assert!(xml.contains("tx_sar_001"));
    assert!(xml.contains("Blazil Financial Inc."));
    assert!(xml.contains("sender_alice"));
    assert!(xml.contains(">7500000<")); // amount in minor units
    assert!(xml.contains(r#"code="B""#)); // Structuring
    assert!(xml.contains("threshold"));
}

#[test]
fn test_sar_xml_escapes_element_text() {
    // 100_000 cents = $1,000.00
    let mut tx = usd("tx_sar_002", 100_000);
    tx.sender_id = "alice & <bob>".to_string();
    let report = SarReport::from_transaction(
        &tx,
        "Institution & Co.",
        SuspiciousActivityType::Fraud,
        "Narrative text",
    );

    let xml = String::from_utf8(report.to_xml().unwrap()).unwrap();

    assert!(
        xml.contains("alice &amp; &lt;bob&gt;"),
        "sender_id must be XML-escaped"
    );
    assert!(
        xml.contains("Institution &amp; Co."),
        "filing_institution must be XML-escaped"
    );
    assert!(
        !xml.contains("alice & <bob>"),
        "unescaped special chars must not appear in element text"
    );
}

#[test]
fn test_sar_xml_cdata_escape_for_narrative() {
    let tx = usd("tx_sar_003", 100_000);
    let report = SarReport::from_transaction(
        &tx,
        "Institution",
        SuspiciousActivityType::Other,
        "Contains ]]> which must be handled.",
    );

    let xml = String::from_utf8(report.to_xml().unwrap()).unwrap();
    assert!(
        !xml.contains("<![CDATA[Contains ]]>"),
        "raw ]]> must be escaped inside CDATA"
    );
}

#[test]
fn test_sar_fincen_activity_codes() {
    let cases = [
        (SuspiciousActivityType::MoneyLaundering, "A"),
        (SuspiciousActivityType::Structuring, "B"),
        (SuspiciousActivityType::Fraud, "C"),
        (SuspiciousActivityType::TerroristFinancing, "D"),
        (SuspiciousActivityType::Other, "Z"),
    ];

    for (sat, expected_code) in cases {
        let tx = usd("tx_code", 100_000);
        let report = SarReport::from_transaction(&tx, "Inst", sat, "narrative");
        let xml = String::from_utf8(report.to_xml().unwrap()).unwrap();
        assert!(
            xml.contains(&format!(r#"code="{expected_code}""#)),
            "expected FinCEN code {expected_code}"
        );
    }
}

#[test]
fn test_sar_from_transaction_populates_fields() {
    // 2_000_000 cents = $20,000.00
    let mut tx = usd("tx_sar_004", 2_000_000);
    tx.currency = "SGD".to_string();
    tx.sender_id = "kyc_subject_99".to_string();

    let report = SarReport::from_transaction(
        &tx,
        "Filing Inst",
        SuspiciousActivityType::MoneyLaundering,
        "Narrative",
    );

    assert_eq!(report.transaction_id, "tx_sar_004");
    assert_eq!(report.amount, 2_000_000);
    assert_eq!(report.currency, "SGD");
    assert_eq!(report.subject_id, "kyc_subject_99");
    assert_eq!(report.filing_institution, "Filing Inst");
    assert!(!report.is_empty_narrative());
}
