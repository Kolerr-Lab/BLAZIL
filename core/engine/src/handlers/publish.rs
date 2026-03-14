//! Publish / egress handler.
//!
//! [`PublishHandler`] is the **last** stage in the pipeline. It inspects the
//! final [`TransactionResult`] and records it for downstream consumption
//! (metrics, response callbacks, audit log, etc.).
//!
//! # Interaction with the batch LedgerHandler
//!
//! [`super::ledger::LedgerHandler`] defers result writes for all events in a
//! batch except the last. When `PublishHandler` processes those earlier events
//! their `result` field will be `None`. This handler buffers those events as
//! raw pointers and flushes them at `end_of_batch` — by then `LedgerHandler`
//! has written all results back to the ring buffer.
//!
//! # Safety invariant
//!
//! Deferred pointers are stored for ring buffer slots from **previous**
//! `on_event` calls. Those slots remain valid until `gating_sequence` advances
//! after the full batch loop completes. All accesses are on the single
//! dedicated runner thread.
//!
//! # Responsibilities
//!
//! 1. If `event.flags.skip_publish` is set, do nothing.
//! 2. On `Committed` — increment the committed counter and emit a `debug!`
//!    trace. (Actual fan-out to subscribers is a later milestone.)
//! 3. On `Rejected` — increment the rejected counter and emit a `debug!`
//!    trace.
//! 4. On `None` — defer (see batching note above). If seen at `end_of_batch`,
//!    log an error (pipeline bug; LedgerHandler must always write before us).

use tracing::{debug, error};

use crate::event::{TransactionEvent, TransactionResult};
use crate::handler::EventHandler;

// ── PublishHandler ─────────────────────────────────────────────────────────────

/// Final pipeline stage: records and publishes committed / rejected events.
pub struct PublishHandler {
    published_count: u64,
    rejected_count: u64,
    /// Ring buffer slot pointers for events whose `result` was `None` when
    /// `PublishHandler` first processed them. `LedgerHandler` writes results
    /// back to these slots; we flush them at `end_of_batch`.
    deferred: Vec<*mut TransactionEvent>,
}

// SAFETY: `PublishHandler` runs exclusively on the dedicated Disruptor runner
// thread.  Raw pointers in `deferred` are never sent to or accessed from any
// other thread.
unsafe impl Send for PublishHandler {}

impl PublishHandler {
    /// Creates a new `PublishHandler`.
    pub fn new() -> Self {
        Self {
            published_count: 0,
            rejected_count: 0,
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
        Self::new()
    }
}

impl EventHandler for PublishHandler {
    fn on_event(&mut self, event: &mut TransactionEvent, sequence: i64, end_of_batch: bool) {
        if event.flags.skip_publish {
            if end_of_batch {
                self.flush_deferred();
            }
            return;
        }

        match &event.result {
            None => {
                // LedgerHandler deferred this event's result to a future batch
                // flush.  Store a raw pointer; we'll process it at end_of_batch
                // once LedgerHandler has written the result back.
                //
                // SAFETY: same invariant as LedgerHandler — previous-call slot,
                // producer cannot reclaim until gating_sequence advances.
                self.deferred.push(event as *mut _);
                // Do NOT flush deferred here — results aren't ready until
                // LedgerHandler fires its flush (which happens before us for
                // the same event at end_of_batch).
                return;
            }
            Some(TransactionResult::Committed {
                transfer_id,
                timestamp,
            }) => {
                self.published_count += 1;
                debug!(
                    sequence,
                    transaction_id = %event.transaction_id,
                    %transfer_id,
                    timestamp_ns = timestamp.as_nanos(),
                    "PublishHandler: committed"
                );
            }
            Some(TransactionResult::Rejected { reason }) => {
                self.rejected_count += 1;
                debug!(
                    sequence,
                    transaction_id = %event.transaction_id,
                    error = %reason,
                    "PublishHandler: rejected"
                );
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
        for ptr in self.deferred.drain(..) {
            // SAFETY: ptr is a ring buffer slot from a previous on_event call.
            // LedgerHandler has written its result before this flush runs
            // (LedgerHandler precedes PublishHandler in the handler chain for
            // the same event).
            let event = unsafe { &*ptr };
            match &event.result {
                Some(TransactionResult::Committed {
                    transfer_id,
                    timestamp,
                }) => {
                    self.published_count += 1;
                    debug!(
                        transaction_id = %event.transaction_id,
                        %transfer_id,
                        timestamp_ns = timestamp.as_nanos(),
                        "PublishHandler: committed (deferred batch)"
                    );
                }
                Some(TransactionResult::Rejected { reason }) => {
                    self.rejected_count += 1;
                    debug!(
                        transaction_id = %event.transaction_id,
                        error = %reason,
                        "PublishHandler: rejected (deferred batch)"
                    );
                }
                None => {
                    // LedgerHandler must always write a result before we flush.
                    // If we see None here it is a pipeline bug.
                    error!(
                        transaction_id = %event.transaction_id,
                        "PublishHandler: pipeline bug — deferred event still has no result at flush"
                    );
                }
            }
        }
    }
}

// ── tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use blazil_common::amount::Amount;
    use blazil_common::currency::parse_currency;
    use blazil_common::ids::{AccountId, LedgerId, TransactionId};
    use blazil_common::timestamp::Timestamp;
    use rust_decimal::Decimal;

    use super::*;
    use crate::event::TransactionEvent;

    fn make_event() -> TransactionEvent {
        let usd = parse_currency("USD").unwrap();
        let amount = Amount::new(Decimal::new(10_000, 2), usd).unwrap();
        TransactionEvent::new(
            TransactionId::new(),
            AccountId::new(),
            AccountId::new(),
            amount,
            LedgerId::USD,
            1,
        )
    }

    #[test]
    fn skip_publish_flag_is_respected() {
        let mut h = PublishHandler::new();
        let mut event = make_event();
        event.flags.skip_publish = true;
        event.result = Some(TransactionResult::Committed {
            transfer_id: blazil_common::ids::TransferId::new(),
            timestamp: Timestamp::now(),
        });
        h.on_event(&mut event, 0, false);
        assert_eq!(h.published_count(), 0);
        assert_eq!(h.rejected_count(), 0);
    }

    #[test]
    fn committed_event_increments_published_count() {
        let mut h = PublishHandler::new();
        let mut event = make_event();
        event.result = Some(TransactionResult::Committed {
            transfer_id: blazil_common::ids::TransferId::new(),
            timestamp: Timestamp::now(),
        });
        h.on_event(&mut event, 1, true);
        assert_eq!(h.published_count(), 1);
        assert_eq!(h.rejected_count(), 0);
    }

    #[test]
    fn rejected_event_increments_rejected_count() {
        use blazil_common::error::BlazerError;
        let mut h = PublishHandler::new();
        let mut event = make_event();
        event.result = Some(TransactionResult::Rejected {
            reason: BlazerError::ValidationError("test".into()),
        });
        h.on_event(&mut event, 2, false);
        assert_eq!(h.published_count(), 0);
        assert_eq!(h.rejected_count(), 1);
    }

    #[test]
    fn multiple_events_accumulate_counts() {
        use blazil_common::error::BlazerError;
        let mut h = PublishHandler::new();

        for i in 0..5_i64 {
            let mut e = make_event();
            e.result = Some(TransactionResult::Committed {
                transfer_id: blazil_common::ids::TransferId::new(),
                timestamp: Timestamp::now(),
            });
            h.on_event(&mut e, i, false);
        }
        for i in 5..8_i64 {
            let mut e = make_event();
            e.result = Some(TransactionResult::Rejected {
                reason: BlazerError::ValidationError("x".into()),
            });
            h.on_event(&mut e, i, false);
        }

        assert_eq!(h.published_count(), 5);
        assert_eq!(h.rejected_count(), 3);
    }
}
