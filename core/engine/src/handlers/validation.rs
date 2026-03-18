//! Structural validation handler.
//!
//! [`ValidationHandler`] is the **first** stage in the pipeline. It
//! validates transaction fields before any money moves. Rejections here
//! are cheap — they prevent unnecessary ledger I/O.
//!
//! # Rules (applied in order)
//!
//! 1. `transaction_id` must not be the zero sentinel.
//! 2. `debit_account_id` ≠ `credit_account_id` (no self-transfers).
//! 3. `amount_units` must be > 0.
//! 4. `debit_account_id` must not be zero.
//! 5. `credit_account_id` must not be zero.
//!
//! If all rules pass, no result is written and the event continues downstream.
//! On failure, result is written to the results map and logged at `WARN`.

use std::sync::Arc;

use blazil_common::error::BlazerError;
use dashmap::DashMap;
use tracing::warn;

use crate::event::{TransactionEvent, TransactionResult};
use crate::handler::EventHandler;

// ── ValidationHandler ─────────────────────────────────────────────────────────

/// Validates transaction business rules.
///
/// First handler in the pipeline. Rejects events with structural violations
/// before they reach the ledger.
///
/// # Examples
///
/// ```rust
/// use std::sync::Arc;
/// use dashmap::DashMap;
/// use blazil_engine::handlers::validation::ValidationHandler;
/// use blazil_engine::handler::EventHandler;
/// use blazil_engine::event::TransactionEvent;
/// use blazil_common::ids::{AccountId, LedgerId, TransactionId};
///
/// let mut event = TransactionEvent::new(
///     TransactionId::new(), AccountId::new(), AccountId::new(),
///     100_00_u64, LedgerId::USD, 1,
/// );
///
/// let results = Arc::new(DashMap::new());
/// let mut handler = ValidationHandler::new(Arc::clone(&results));
/// handler.on_event(&mut event, 0, true);
/// assert!(!results.contains_key(&0)); // valid event produces no result
/// ```
#[derive(Clone)]
pub struct ValidationHandler {
    results: Arc<DashMap<i64, TransactionResult>>,
}

impl ValidationHandler {
    /// Creates a new `ValidationHandler`.
    pub fn new(results: Arc<DashMap<i64, TransactionResult>>) -> Self {
        Self { results }
    }
}
impl EventHandler for ValidationHandler {
    fn on_event(&mut self, event: &mut TransactionEvent, sequence: i64, _end_of_batch: bool) {
        // Skip events already rejected upstream (belt-and-suspenders).
        if self.results.contains_key(&sequence) {
            return;
        }

        if let Err(reason) = validate(event) {
            warn!(
                sequence,
                transaction_id = %event.transaction_id,
                reason = %reason,
                "ValidationHandler: rejecting event"
            );
            self.results
                .insert(sequence, TransactionResult::Rejected { reason });
        }
    }

    fn clone_handler(&self) -> Box<dyn EventHandler> {
        Box::new(self.clone())
    }
}

/// Runs all validation rules against `event`. Returns `Ok(())` if all pass.
fn validate(event: &TransactionEvent) -> Result<(), BlazerError> {
    // Rule 1: transaction_id must not be zero.
    if event.transaction_id.is_zero() {
        return Err(BlazerError::ValidationError(
            "transaction_id must not be zero".into(),
        ));
    }

    // Rule 4: debit_account_id must not be zero.
    if event.debit_account_id.is_zero() {
        return Err(BlazerError::ValidationError(
            "debit_account_id must not be zero".into(),
        ));
    }

    // Rule 5: credit_account_id must not be zero.
    if event.credit_account_id.is_zero() {
        return Err(BlazerError::ValidationError(
            "credit_account_id must not be zero".into(),
        ));
    }

    // Rule 2: no self-transfers.
    if event.debit_account_id == event.credit_account_id {
        return Err(BlazerError::ValidationError(
            "debit_account_id must not equal credit_account_id".into(),
        ));
    }

    // Rule 3: amount must be positive.
    if event.amount_units == 0 {
        return Err(BlazerError::ValidationError(
            "amount_units must be greater than zero".into(),
        ));
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use blazil_common::ids::{AccountId, LedgerId, TransactionId};
    use dashmap::DashMap;

    use super::*;

    fn make_valid_event() -> TransactionEvent {
        TransactionEvent::new(
            TransactionId::new(),
            AccountId::new(),
            AccountId::new(),
            10_000_u64, // 100.00 USD in cents
            LedgerId::USD,
            1,
        )
    }

    /// Run validation with a fresh DashMap and return the map so tests can
    /// inspect what (if anything) was written.
    fn run(event: &mut TransactionEvent) -> Arc<DashMap<i64, TransactionResult>> {
        let results = Arc::new(DashMap::new());
        ValidationHandler::new(Arc::clone(&results)).on_event(event, 0, true);
        results
    }

    #[test]
    fn valid_event_result_remains_none() {
        let mut event = make_valid_event();
        let results = run(&mut event);
        assert!(
            !results.contains_key(&0),
            "valid event should not produce a result"
        );
    }

    #[test]
    fn zero_transaction_id_is_rejected() {
        let mut event = make_valid_event();
        event.transaction_id = TransactionId::from_u64(0);
        let results = run(&mut event);
        assert!(
            matches!(
                results.get(&0).as_deref(),
                Some(TransactionResult::Rejected { .. })
            ),
            "zero transaction_id must be rejected"
        );
    }

    #[test]
    fn zero_debit_account_id_is_rejected() {
        let mut event = make_valid_event();
        event.debit_account_id = AccountId::from_u64(0);
        let results = run(&mut event);
        assert!(
            matches!(
                results.get(&0).as_deref(),
                Some(TransactionResult::Rejected { .. })
            ),
            "zero debit_account_id must be rejected"
        );
    }

    #[test]
    fn zero_credit_account_id_is_rejected() {
        let mut event = make_valid_event();
        event.credit_account_id = AccountId::from_u64(0);
        let results = run(&mut event);
        assert!(
            matches!(
                results.get(&0).as_deref(),
                Some(TransactionResult::Rejected { .. })
            ),
            "zero credit_account_id must be rejected"
        );
    }

    #[test]
    fn self_transfer_is_rejected() {
        let mut event = make_valid_event();
        event.credit_account_id = event.debit_account_id;
        let results = run(&mut event);
        assert!(
            matches!(
                results.get(&0).as_deref(),
                Some(TransactionResult::Rejected { .. })
            ),
            "self-transfer must be rejected"
        );
    }

    #[test]
    fn zero_amount_is_rejected() {
        let mut event = make_valid_event();
        event.amount_units = 0;
        let results = run(&mut event);
        assert!(
            matches!(
                results.get(&0).as_deref(),
                Some(TransactionResult::Rejected { .. })
            ),
            "zero amount_units must be rejected"
        );
    }

    #[test]
    fn already_rejected_event_is_not_double_rejected() {
        use blazil_common::error::BlazerError;

        let mut event = make_valid_event();
        // Pre-set a rejection in the results map before running validation.
        let results: Arc<DashMap<i64, TransactionResult>> = Arc::new(DashMap::new());
        results.insert(
            0,
            TransactionResult::Rejected {
                reason: BlazerError::ValidationError("upstream".into()),
            },
        );
        let original_msg = match results.get(&0).as_deref() {
            Some(TransactionResult::Rejected { reason }) => reason.to_string(),
            _ => panic!("expected pre-set rejection"),
        };

        ValidationHandler::new(Arc::clone(&results)).on_event(&mut event, 0, true);

        // Result must be unchanged — ValidationHandler must not overwrite.
        let current_msg = match results.get(&0).as_deref() {
            Some(TransactionResult::Rejected { reason }) => reason.to_string(),
            _ => panic!("expected rejection after re-run"),
        };
        assert_eq!(
            original_msg, current_msg,
            "pre-existing rejection must not be overwritten"
        );
    }
}
