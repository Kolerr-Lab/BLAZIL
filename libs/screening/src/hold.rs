// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Transaction hold / release lifecycle.
//!
//! Holds represent compliance-enforced blocks on transactions pending manual
//! review. They **must** be persisted to a durable, auditable store so that a
//! process restart cannot silently unblock a held transaction.
//!
//! The `InMemoryHoldStore` is provided exclusively for testing. Production
//! deployments must use a TigerBeetle-backed implementation that records each
//! hold and release as a ledger transfer with appropriate flags.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::ScreeningError;

/// The persistent state of a held transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldRecord {
    /// Identifier of the transaction under hold.
    pub transaction_id: String,

    /// Human-readable reason for placing the hold.
    pub reason: String,

    /// If `true`, a compliance analyst must explicitly release this hold.
    /// If `false`, the hold may be released automatically after re-screening.
    pub review_required: bool,

    /// UTC timestamp when the hold was placed.
    pub held_at: DateTime<Utc>,

    /// UTC timestamp when the hold was released, if it has been released.
    pub released_at: Option<DateTime<Utc>>,
}

impl HoldRecord {
    /// Returns `true` if this hold has been released.
    pub fn is_released(&self) -> bool {
        self.released_at.is_some()
    }
}

/// Persistent store for transaction hold state.
///
/// # Implementation requirements
///
/// - `hold()` must be atomic and idempotency-safe: a second call for the same
///   transaction ID must return an error rather than silently overwriting.
/// - `release()` must record the release timestamp durably before returning.
/// - All writes must be linearisable: concurrent calls must not produce
///   inconsistent state.
///
/// The `InMemoryHoldStore` satisfies these constraints for a single process
/// but does not survive restarts. Production implementations must back this
/// store against TigerBeetle or an equivalent ACID-compliant datastore.
#[async_trait]
pub trait HoldStore: Send + Sync {
    /// Places a hold on a transaction.
    ///
    /// # Errors
    ///
    /// Returns `ScreeningError::HoldStore` if a hold already exists for
    /// this transaction ID (duplicate hold prevention).
    async fn hold(&self, record: HoldRecord) -> Result<(), ScreeningError>;

    /// Releases a previously held transaction.
    ///
    /// # Errors
    ///
    /// Returns `ScreeningError::HoldStore` if:
    /// - No hold exists for `transaction_id`.
    /// - The hold has already been released (double-release prevention).
    async fn release(&self, transaction_id: &str) -> Result<HoldRecord, ScreeningError>;

    /// Returns the hold record for a transaction, if one exists.
    async fn get(&self, transaction_id: &str) -> Option<HoldRecord>;

    /// Returns all holds that have not yet been released.
    async fn active_holds(&self) -> Vec<HoldRecord>;
}

/// In-memory `HoldStore` backed by `DashMap`.
///
/// **FOR TESTING ONLY.** State is lost on process restart. Do not use in
/// production — a restarted process would silently drop all active holds,
/// allowing blocked transactions to proceed without compliance review.
#[derive(Debug, Default, Clone)]
pub struct InMemoryHoldStore {
    records: Arc<DashMap<String, HoldRecord>>,
}

impl InMemoryHoldStore {
    /// Creates a new empty in-memory hold store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl HoldStore for InMemoryHoldStore {
    async fn hold(&self, record: HoldRecord) -> Result<(), ScreeningError> {
        use dashmap::mapref::entry::Entry;

        match self.records.entry(record.transaction_id.clone()) {
            Entry::Occupied(_) => {
                let tx_id = &record.transaction_id;
                Err(ScreeningError::HoldStore(format!(
                    "hold already exists for transaction {tx_id}"
                )))
            }
            Entry::Vacant(slot) => {
                slot.insert(record);
                Ok(())
            }
        }
    }

    async fn release(&self, transaction_id: &str) -> Result<HoldRecord, ScreeningError> {
        match self.records.get_mut(transaction_id) {
            None => Err(ScreeningError::HoldStore(format!(
                "no active hold found for transaction {transaction_id}"
            ))),
            Some(mut entry) => {
                if entry.is_released() {
                    return Err(ScreeningError::HoldStore(format!(
                        "transaction {transaction_id} hold already released"
                    )));
                }
                entry.released_at = Some(Utc::now());
                Ok(entry.value().clone())
            }
        }
    }

    async fn get(&self, transaction_id: &str) -> Option<HoldRecord> {
        self.records.get(transaction_id).map(|r| r.value().clone())
    }

    async fn active_holds(&self) -> Vec<HoldRecord> {
        self.records
            .iter()
            .filter(|r| !r.value().is_released())
            .map(|r| r.value().clone())
            .collect()
    }
}
