//! Publish / egress handler.
//!
//! [`PublishHandler`] is the **last** stage in the pipeline. It inspects the
//! final [`TransactionResult`] and records it for downstream consumption
//! (metrics, response callbacks, audit log, etc.).
//!
//! # Responsibilities
//!
//! 1. If `event.flags.skip_publish` is set, do nothing.
//! 2. On `Committed` — increment the committed counter and emit a `debug!`
//!    trace. (Actual fan-out to subscribers is a later milestone.)
//! 3. On `Rejected` — increment the rejected counter and emit a `debug!`
//!    trace.
//! 4. On `None` — this should never happen; log an `error!` describing the
//!    pipeline bug.

use tracing::{debug, error};

use crate::event::{TransactionEvent, TransactionResult};
use crate::handler::EventHandler;

// ── PublishHandler ─────────────────────────────────────────────────────────────

/// Final pipeline stage: records and publishes committed / rejected events.
pub struct PublishHandler {
    published_count: u64,
    rejected_count: u64,
}

impl PublishHandler {
    /// Creates a new `PublishHandler`.
    pub fn new() -> Self {
        Self {
            published_count: 0,
            rejected_count: 0,
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
    fn on_event(
        &mut self,
        event: &mut TransactionEvent,
        sequence: i64,
        _end_of_batch: bool,
    ) {
        if event.flags.skip_publish {
            return;
        }

        match &event.result {
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
            None => {
                // This must not happen in a correctly-ordered pipeline; flag
                // loudly so we catch it during integration testing.
                error!(
                    sequence,
                    transaction_id = %event.transaction_id,
                    "PublishHandler: pipeline bug — event has no result at publish stage"
                );
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
        let amount = Amount::new(Decimal::new(100_00, 2), usd).unwrap();
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
