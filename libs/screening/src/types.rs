// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Core domain types for compliance screening.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The mode in which a transaction is screened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreeningMode {
    /// Inline in the transaction pipeline.
    ///
    /// The screener must respond within 50 ms. If it does not, the
    /// `RealTimeRouter` falls back to `Clear` and signals the caller to
    /// enqueue the transaction for deferred batch re-screening.
    RealTime,

    /// Asynchronous, post-commit.
    ///
    /// Jobs are submitted to the `BatchWorker` over an mpsc channel and
    /// processed outside the critical transaction path.
    Batch,
}

/// Risk severity attached to flagged transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

/// The outcome of a compliance screening check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScreeningResult {
    /// No suspicious activity detected; transaction may proceed.
    Clear,

    /// Suspicious indicators present; transaction may proceed but is flagged
    /// for asynchronous review.
    Flag { reason: String, severity: RiskLevel },

    /// Transaction is blocked pending manual compliance review.
    Hold {
        reason: String,
        /// If `true`, a compliance analyst must explicitly release the hold.
        review_required: bool,
    },

    /// Transaction is rejected outright.
    Reject {
        reason: String,
        /// If `true`, a Suspicious Activity Report must be filed with FinCEN.
        sar_required: bool,
    },
}

impl ScreeningResult {
    /// Returns `true` if the transaction cleared without any restrictions.
    pub fn is_clear(&self) -> bool {
        matches!(self, Self::Clear)
    }

    /// Returns `true` if the transaction is blocked (held or rejected).
    pub fn is_blocked(&self) -> bool {
        matches!(self, Self::Hold { .. } | Self::Reject { .. })
    }

    /// Returns `true` if a Suspicious Activity Report must be filed.
    pub fn requires_sar(&self) -> bool {
        matches!(
            self,
            Self::Reject {
                sar_required: true,
                ..
            }
        )
    }
}

/// A transaction event submitted to the compliance screening pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionEvent {
    /// Globally unique transaction identifier.
    pub transaction_id: String,

    /// Amount in minor units (e.g. cents for USD, pence for GBP).
    pub amount: u64,

    /// ISO 4217 currency code (e.g. `"USD"`, `"SGD"`).
    pub currency: String,

    /// Originating party identifier (account ID, customer ID, etc.).
    pub sender_id: String,

    /// Receiving party identifier.
    pub receiver_id: String,

    /// Arbitrary key-value metadata for provider-specific enrichment
    /// (e.g. IP address, device fingerprint, merchant category code).
    pub metadata: HashMap<String, String>,

    /// UTC timestamp when this transaction was initiated.
    pub timestamp: DateTime<Utc>,
}

impl TransactionEvent {
    /// Creates a new transaction event with an empty metadata map and the
    /// current UTC timestamp.
    pub fn new(
        transaction_id: impl Into<String>,
        amount: u64,
        currency: impl Into<String>,
        sender_id: impl Into<String>,
        receiver_id: impl Into<String>,
    ) -> Self {
        Self {
            transaction_id: transaction_id.into(),
            amount,
            currency: currency.into(),
            sender_id: sender_id.into(),
            receiver_id: receiver_id.into(),
            metadata: HashMap::new(),
            timestamp: Utc::now(),
        }
    }

    /// Attaches a metadata entry and returns `self` for chaining.
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}
