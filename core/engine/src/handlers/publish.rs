//! Publish / egress handler.
//!
//! [`PublishHandler`] is the **last** stage in the pipeline. It inspects the
//! final [`TransactionResult`] from the results map and records it for downstream
//! consumption (metrics, response callbacks, audit log, etc.).
//!
//! # Interaction with the batch LedgerHandler
//!
//! [`super::ledger::LedgerHandler`] defers result writes for all events in a
//! batch except the last. When `PublishHandler` processes those earlier events
//! their results may not yet be in the map. This handler buffers those sequence
//! numbers and flushes them at `end_of_batch` — by then `LedgerHandler` has
//! written all results to the map.
//!
//! # Responsibilities
//!
//! 1. If `event.flags.skip_publish` is set, do nothing.
//! 2. On `Committed` — increment the committed counter and emit a `debug!`
//!    trace. (Actual fan-out to subscribers is a later milestone.)
//! 3. On `Rejected` — increment the rejected counter and emit a `debug!`
//!    trace.
//! 4. On missing result — defer (see batching note above). If seen at `end_of_batch`,
//!    log an error (pipeline bug; LedgerHandler must always write before us).

use std::sync::Arc;

use dashmap::DashMap;
use tracing::{debug, error};

use crate::event::{TransactionEvent, TransactionResult};
use crate::handler::EventHandler;

// ── PublishHandler ─────────────────────────────────────────────────────────────

/// Final pipeline stage: records and publishes committed / rejected events.
pub struct PublishHandler {
    published_count: u64,
    rejected_count: u64,
    results: Arc<DashMap<i64, TransactionResult>>,
    /// Sequence numbers for events whose results weren't in the map yet when
    /// `PublishHandler` first processed them. `LedgerHandler` writes results
    /// to the map; we flush at `end_of_batch`.
    deferred: Vec<i64>,
}

impl PublishHandler {
    /// Creates a new `PublishHandler`.
    pub fn new(results: Arc<DashMap<i64, TransactionResult>>) -> Self {
        Self {
            published_count: 0,
            rejected_count: 0,
            results,
            deferred: Vec::new(),
        }
    }

    /// Total number of committed transactions successfully published.
    pub fn published_count(&self) -> u64 {
        self.published_count
    }

    /// Total number of rejected transactions recorded.
    pub fn rejected_count(&self) -> u64 {
        self.rejected_count
    }
}

impl Default for PublishHandler {
    fn default() -> Self {
        Self::new(Arc::new(DashMap::new()))
    }
}

impl EventHandler for PublishHandler {
    fn on_event(&mut self, event: &mut TransactionEvent, sequence: i64, end_of_batch: bool) {
        if event.flags.skip_publish() {
            if end_of_batch {
                self.flush_deferred();
            }
            return;
        }

        match self.results.get(&sequence) {
            None => {
                // LedgerHandler deferred this event's result to a future batch
                // flush. Store the sequence number; we'll process it at end_of_batch
                // once LedgerHandler has written the result to the map.
                self.deferred.push(sequence);
                // Do NOT flush deferred here — results aren't ready until
                // LedgerHandler fires its flush (which happens before us for
                // the same event at end_of_batch).
                return;
            }
            Some(result_ref) => {
                let result = result_ref.value();
                match result {
                    TransactionResult::Committed {
                        transfer_id,
                        timestamp,
                    } => {
                        self.published_count += 1;
                        debug!(
                            sequence,
                            transaction_id = %event.transaction_id,
                            %transfer_id,
                            timestamp_ns = timestamp.as_nanos(),
                            "PublishHandler: committed"
                        );
                    }
                    TransactionResult::Rejected { reason } => {
                        self.rejected_count += 1;
                        debug!(
                            sequence,
                            transaction_id = %event.transaction_id,
                            error = %reason,
                            "PublishHandler: rejected"
                        );
                    }
                }
            }
        }

        if end_of_batch {
            self.flush_deferred();
        }
    }
}

impl PublishHandler {
    /// Processes all deferred events whose results were written by
    /// `LedgerHandler` during this batch's flush.
    fn flush_deferred(&mut self) {
        for seq in self.deferred.drain(..) {
            match self.results.get(&seq) {
                Some(result_ref) => {
                    let result = result_ref.value();
                    match result {
                        TransactionResult::Committed {
                            transfer_id,
                            timestamp,
                        } => {
                            self.published_count += 1;
                            debug!(
                                sequence = seq,
                                %transfer_id,
                                timestamp_ns = timestamp.as_nanos(),
                                "PublishHandler: committed (deferred batch)"
                            );
                        }
                        TransactionResult::Rejected { reason } => {
                            self.rejected_count += 1;
                            debug!(
                                sequence = seq,
                                error = %reason,
                                "PublishHandler: rejected (deferred batch)"
                            );
                        }
                    }
                }
                None => {
                    // LedgerHandler must always write a result before we flush.
                    // If we see None here it is a pipeline bug.
                    error!(
                        sequence = seq,
                        "PublishHandler: pipeline bug — deferred event still has no result at flush"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use blazil_common::ids::{AccountId, LedgerId, TransactionId, TransferId};
    use blazil_common::timestamp::Timestamp;
    use dashmap::DashMap;

    use super::*;
    use crate::event::TransactionEvent;

    fn make_event() -> TransactionEvent {
        TransactionEvent::new(
            TransactionId::new(),
            AccountId::new(),
            AccountId::new(),
            10_000_u64, // $100.00 in cents
            LedgerId::USD,
            1,
        )
    }

    #[test]
    fn skip_publish_flag_is_respected() {
        let results: Arc<DashMap<i64, TransactionResult>> = Arc::new(DashMap::new());
        results.insert(
            0,
            TransactionResult::Committed {
                transfer_id: TransferId::new(),
                timestamp: Timestamp::now(),
            },
        );
        let mut h = PublishHandler::new(Arc::clone(&results));
        let mut event = make_event();
        event.flags.set_skip_publish(true);
        h.on_event(&mut event, 0, false);
        assert_eq!(h.published_count(), 0);
        assert_eq!(h.rejected_count(), 0);
    }

    #[test]
    fn committed_event_increments_published_count() {
        let results: Arc<DashMap<i64, TransactionResult>> = Arc::new(DashMap::new());
        results.insert(
            1,
            TransactionResult::Committed {
                transfer_id: TransferId::new(),
                timestamp: Timestamp::now(),
            },
        );
        let mut h = PublishHandler::new(Arc::clone(&results));
        let mut event = make_event();
        h.on_event(&mut event, 1, true);
        assert_eq!(h.published_count(), 1);
        assert_eq!(h.rejected_count(), 0);
    }

    #[test]
    fn rejected_event_increments_rejected_count() {
        use blazil_common::error::BlazerError;
        let results: Arc<DashMap<i64, TransactionResult>> = Arc::new(DashMap::new());
        results.insert(
            2,
            TransactionResult::Rejected {
                reason: BlazerError::ValidationError("test".into()),
            },
        );
        let mut h = PublishHandler::new(Arc::clone(&results));
        let mut event = make_event();
        h.on_event(&mut event, 2, false);
        assert_eq!(h.published_count(), 0);
        assert_eq!(h.rejected_count(), 1);
    }

    #[test]
    fn multiple_events_accumulate_counts() {
        use blazil_common::error::BlazerError;
        let results: Arc<DashMap<i64, TransactionResult>> = Arc::new(DashMap::new());
        for i in 0..5_i64 {
            results.insert(
                i,
                TransactionResult::Committed {
                    transfer_id: TransferId::new(),
                    timestamp: Timestamp::now(),
                },
            );
        }
        for i in 5..8_i64 {
            results.insert(
                i,
                TransactionResult::Rejected {
                    reason: BlazerError::ValidationError("x".into()),
                },
            );
        }

        let mut h = PublishHandler::new(Arc::clone(&results));
        for i in 0..5_i64 {
            let mut e = make_event();
            h.on_event(&mut e, i, false);
        }
        for i in 5..8_i64 {
            let mut e = make_event();
            h.on_event(&mut e, i, false);
        }

        assert_eq!(h.published_count(), 5);
        assert_eq!(h.rejected_count(), 3);
    }
}
