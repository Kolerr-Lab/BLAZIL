//! Structural validation handler.
//!
//! [`ValidationHandler`] is the **first** stage in the pipeline. It
//! validates transaction fields before any money moves. Rejections here
//! are cheap — they prevent unnecessary ledger I/O.
//!
//! # Rules (applied in order)
//!
//! 1. `transaction_id` must not be the nil UUID.
//! 2. `debit_account_id` ≠ `credit_account_id` (no self-transfers).
//! 3. `amount.value()` must be strictly positive.
//! 4. `amount.value().scale()` must be ≤ 8.
//! 5. `debit_account_id` must not be the nil UUID.
//! 6. `credit_account_id` must not be the nil UUID.
//!
//! If all rules pass, `event.result` remains `None` and the event
//! continues downstream. On failure, `event.result` is set to
//! [`TransactionResult::Rejected`] and logged at `WARN`.

use blazil_common::error::BlazerError;
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
/// use blazil_engine::handlers::validation::ValidationHandler;
/// use blazil_engine::handler::EventHandler;
/// use blazil_engine::event::TransactionEvent;
/// use blazil_common::ids::{AccountId, LedgerId, TransactionId};
/// use blazil_common::amount::Amount;
/// use blazil_common::currency::parse_currency;
/// use rust_decimal::Decimal;
///
/// let usd = parse_currency("USD").unwrap();
/// let amount = Amount::new(Decimal::new(100_00, 2), usd).unwrap();
/// let mut event = TransactionEvent::new(
///     TransactionId::new(), AccountId::new(), AccountId::new(),
///     amount, LedgerId::USD, 1,
/// );
///
/// let mut handler = ValidationHandler;
/// handler.on_event(&mut event, 0, true);
/// assert!(event.result.is_none()); // valid event passes through
/// ```
pub struct ValidationHandler;

impl EventHandler for ValidationHandler {
    fn on_event(
        &mut self,
        event: &mut TransactionEvent,
        sequence: i64,
        _end_of_batch: bool,
    ) {
        // Skip events already rejected upstream (belt-and-suspenders).
        if event.result.is_some() {
            return;
        }

        if let Err(reason) = validate(event) {
            warn!(
                sequence,
                transaction_id = %event.transaction_id,
                reason = %reason,
                "ValidationHandler: rejecting event"
            );
            event.result = Some(TransactionResult::Rejected { reason });
        }
    }
}

/// Runs all validation rules against `event`. Returns `Ok(())` if all pass.
fn validate(event: &TransactionEvent) -> Result<(), BlazerError> {
    use rust_decimal::Decimal;

    // Rule 1: transaction_id must not be nil.
    if event.transaction_id.as_uuid().is_nil() {
        return Err(BlazerError::ValidationError(
            "transaction_id must not be the nil UUID".into(),
        ));
    }

    // Rule 5: debit_account_id must not be nil.
    if event.debit_account_id.as_uuid().is_nil() {
        return Err(BlazerError::ValidationError(
            "debit_account_id must not be the nil UUID".into(),
        ));
    }

    // Rule 6: credit_account_id must not be nil.
    if event.credit_account_id.as_uuid().is_nil() {
        return Err(BlazerError::ValidationError(
            "credit_account_id must not be the nil UUID".into(),
        ));
    }

    // Rule 2: no self-transfers.
    if event.debit_account_id == event.credit_account_id {
        return Err(BlazerError::ValidationError(
            "debit_account_id must not equal credit_account_id".into(),
        ));
    }

    // Rule 3: amount must be positive.
    if event.amount.value() <= Decimal::ZERO {
        return Err(BlazerError::ValidationError(
            "amount must be greater than zero".into(),
        ));
    }

    // Rule 4: scale must be <= 8.
    if event.amount.value().scale() > 8 {
        return Err(BlazerError::InvalidAmountScale(
            event.amount.value().scale(),
        ));
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use blazil_common::amount::Amount;
    use blazil_common::currency::parse_currency;
    use blazil_common::ids::{AccountId, LedgerId, TransactionId};
    use rust_decimal::Decimal;

    fn make_valid_event() -> TransactionEvent {
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

    fn run(event: &mut TransactionEvent) {
        ValidationHandler.on_event(event, 0, true);
    }

    #[test]
    fn valid_event_result_remains_none() {
        let mut event = make_valid_event();
        run(&mut event);
        assert!(event.result.is_none());
    }

    #[test]
    fn nil_transaction_id_is_rejected() {
        let mut event = make_valid_event();
        event.transaction_id = TransactionId::from_bytes([0u8; 16]);
        run(&mut event);
        assert!(event.is_rejected());
    }

    #[test]
    fn nil_debit_account_id_is_rejected() {
        let mut event = make_valid_event();
        event.debit_account_id = AccountId::from_bytes([0u8; 16]);
        run(&mut event);
        assert!(event.is_rejected());
    }

    #[test]
    fn nil_credit_account_id_is_rejected() {
        let mut event = make_valid_event();
        event.credit_account_id = AccountId::from_bytes([0u8; 16]);
        run(&mut event);
        assert!(event.is_rejected());
    }

    #[test]
    fn self_transfer_is_rejected() {
        let mut event = make_valid_event();
        event.credit_account_id = event.debit_account_id;
        run(&mut event);
        assert!(event.is_rejected());
    }

    #[test]
    fn zero_amount_is_rejected() {
        let mut event = make_valid_event();
        let usd = parse_currency("USD").unwrap();
        event.amount = Amount::zero(usd);
        run(&mut event);
        assert!(event.is_rejected());
    }

    #[test]
    fn already_rejected_event_is_not_double_rejected() {
        let mut event = make_valid_event();
        // Pre-set a rejection
        event.result = Some(TransactionResult::Rejected {
            reason: BlazerError::ValidationError("upstream".into()),
        });
        let original_msg = match &event.result {
            Some(TransactionResult::Rejected { reason }) => reason.to_string(),
            _ => panic!(),
        };
        run(&mut event);
        // Result must be unchanged
        let current_msg = match &event.result {
            Some(TransactionResult::Rejected { reason }) => reason.to_string(),
            _ => panic!(),
        };
        assert_eq!(original_msg, current_msg);
    }
}
