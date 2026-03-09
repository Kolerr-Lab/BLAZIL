//! Ledger commit handler.
//!
//! [`LedgerHandler`] is the **third** stage in the pipeline. It is where
//! money actually moves: it builds a [`blazil_ledger::transfer::Transfer`]
//! from the event and calls [`LedgerClient::create_transfer`] to commit it
//! to TigerBeetle.
//!
//! # Async-in-sync
//!
//! The handler trait requires synchronous `on_event` calls (the pipeline
//! thread must not park itself on an async executor). `LedgerHandler` uses
//! `tokio::runtime::Runtime::block_on` to drive the async `LedgerClient`
//! call to completion on the calling thread. This is correct because the
//! handler thread is a **dedicated pinned thread** — blocking it for an I/O
//! round-trip is intentional. The async runtime handles the actual I/O
//! without holding any system threads for the full duration.

use std::sync::Arc;

use blazil_common::ids::TransferId;
use blazil_common::timestamp::Timestamp;
use blazil_ledger::client::LedgerClient;
use blazil_ledger::transfer::Transfer;
use tracing::{error, info};

use crate::event::{TransactionEvent, TransactionResult};
use crate::handler::EventHandler;

// ── LedgerHandler ─────────────────────────────────────────────────────────────

/// Commits transactions to TigerBeetle.
///
/// Wraps any [`LedgerClient`] implementation. In tests, use
/// [`blazil_ledger::mock::InMemoryLedgerClient`]; in production, use
/// `TigerBeetleClient` (feature-gated in `blazil-ledger`).
pub struct LedgerHandler<C: LedgerClient> {
    client: Arc<C>,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl<C: LedgerClient> LedgerHandler<C> {
    /// Creates a new `LedgerHandler`.
    ///
    /// - `client` — the [`LedgerClient`] that writes transfers to TigerBeetle.
    /// - `runtime` — a Tokio `Runtime` used to drive async calls synchronously
    ///   from the handler thread.
    pub fn new(client: Arc<C>, runtime: Arc<tokio::runtime::Runtime>) -> Self {
        Self { client, runtime }
    }
}

impl<C: LedgerClient + 'static> EventHandler for LedgerHandler<C> {
    fn on_event(
        &mut self,
        event: &mut TransactionEvent,
        sequence: i64,
        _end_of_batch: bool,
    ) {
        // Rule 1: skip if already rejected.
        if event.result.is_some() {
            return;
        }

        // Build a Transfer from the event fields.
        let transfer = match Transfer::new(
            TransferId::new(),
            event.debit_account_id,
            event.credit_account_id,
            event.amount.clone(),
            event.ledger_id,
            event.code,
        ) {
            Ok(t) => t,
            Err(e) => {
                error!(
                    sequence,
                    transaction_id = %event.transaction_id,
                    error = %e,
                    "LedgerHandler: failed to construct Transfer"
                );
                event.result = Some(TransactionResult::Rejected { reason: e });
                return;
            }
        };

        let transfer_id = *transfer.id();
        let client = Arc::clone(&self.client);

        // block_on is intentional: this is a dedicated handler thread.
        // See module-level doc for rationale.
        match self.runtime.block_on(client.create_transfer(transfer)) {
            Ok(_) => {
                let ts = Timestamp::now();
                info!(
                    sequence,
                    transaction_id = %event.transaction_id,
                    %transfer_id,
                    "LedgerHandler: committed"
                );
                event.result = Some(TransactionResult::Committed {
                    transfer_id,
                    timestamp: ts,
                });
            }
            Err(e) => {
                error!(
                    sequence,
                    transaction_id = %event.transaction_id,
                    error = %e,
                    "LedgerHandler: failed to commit transfer"
                );
                event.result = Some(TransactionResult::Rejected { reason: e });
            }
        }
    }
}
