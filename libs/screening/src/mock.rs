// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Rule-based mock screener for testing and staging environments.
//!
//! Applies deterministic, synchronous rules without any external I/O,
//! making it suitable for unit tests, integration tests, and local
//! development where a live provider is unavailable.
//!
//! Default thresholds are aligned with FinCEN reporting requirements:
//! - Flag ≥ $10,000 (Currency Transaction Report threshold)
//! - Reject ≥ $50,000 (extended suspicious activity threshold)

use async_trait::async_trait;
use tracing::debug;

use crate::{RiskLevel, ScreeningMode, ScreeningResult, TransactionEvent, TransactionScreener};

/// FinCEN Currency Transaction Report threshold: $10,000.00 in cents.
const DEFAULT_FLAG_THRESHOLD: u64 = 1_000_000;

/// Extended suspicious activity threshold: $50,000.00 in cents.
const DEFAULT_REJECT_THRESHOLD: u64 = 5_000_000;

/// Rule-based mock screener for testing and staging environments.
///
/// Evaluation order (first match wins):
/// 1. Sender ID on blocklist → `Reject { sar_required: true }`
/// 2. Amount ≥ reject threshold → `Reject { sar_required: true }`
/// 3. Amount ≥ flag threshold → `Flag { severity: High }`
/// 4. Otherwise → `Clear`
pub struct MockScreener {
    flag_threshold: u64,
    reject_threshold: u64,
    blocklist: Vec<String>,
}

impl MockScreener {
    /// Creates a mock screener with default FinCEN-aligned thresholds and an
    /// empty blocklist.
    pub fn new() -> Self {
        Self {
            flag_threshold: DEFAULT_FLAG_THRESHOLD,
            reject_threshold: DEFAULT_REJECT_THRESHOLD,
            blocklist: Vec::new(),
        }
    }

    /// Creates a mock screener with custom flag and reject thresholds (in
    /// minor units). Useful for tests that need deterministic outcomes at
    /// amounts that differ from the production defaults.
    pub fn with_thresholds(flag_threshold: u64, reject_threshold: u64) -> Self {
        Self {
            flag_threshold,
            reject_threshold,
            blocklist: Vec::new(),
        }
    }

    /// Adds sender IDs to the deny-list. Any transaction from a listed sender
    /// is rejected regardless of amount.
    pub fn with_blocklist(mut self, ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.blocklist.extend(ids.into_iter().map(Into::into));
        self
    }
}

impl Default for MockScreener {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TransactionScreener for MockScreener {
    async fn screen(&self, tx: &TransactionEvent, mode: ScreeningMode) -> ScreeningResult {
        debug!(
            tx_id  = %tx.transaction_id,
            amount = tx.amount,
            sender = %tx.sender_id,
            ?mode,
            "mock screener evaluating transaction"
        );

        // Rule 1: blocklist (highest priority)
        if self.blocklist.iter().any(|id| id == &tx.sender_id) {
            let sender = &tx.sender_id;
            return ScreeningResult::Reject {
                reason: format!("sender {sender} is on the compliance deny-list"),
                sar_required: true,
            };
        }

        // Rule 2: hard reject above threshold
        if tx.amount >= self.reject_threshold {
            let amount = tx.amount;
            let threshold = self.reject_threshold;
            return ScreeningResult::Reject {
                reason: format!("amount {amount} exceeds rejection threshold {threshold}"),
                sar_required: true,
            };
        }

        // Rule 3: flag for review above flag threshold
        if tx.amount >= self.flag_threshold {
            let amount = tx.amount;
            let threshold = self.flag_threshold;
            return ScreeningResult::Flag {
                reason: format!("amount {amount} exceeds flag threshold {threshold}"),
                severity: RiskLevel::High,
            };
        }

        // Rule 4: no issues detected
        ScreeningResult::Clear
    }

    fn provider_name(&self) -> &'static str {
        "mock"
    }
}
