//! Risk check handler.
//!
//! [`RiskHandler`] is the **second** stage in the pipeline. It applies
//! configurable transaction limits. In future prompts, this will be
//! extended with ML-based fraud scoring.
//!
//! # Rules (applied in order)
//!
//! 1. Skip if `event.result` is already `Some` (already rejected upstream).
//! 2. Skip if `event.flags.requires_risk_check == false`.
//! 3. If `amount > max_transaction_amount` → reject with
//!    `BlazerError::ValidationError("transaction exceeds maximum amount limit")`.

use blazil_common::amount::Amount;
use blazil_common::error::BlazerError;
use tracing::warn;

use crate::event::{TransactionEvent, TransactionResult};
use crate::handler::EventHandler;

// ── RiskHandler ───────────────────────────────────────────────────────────────

/// Basic risk checks.
///
/// Placeholder for ML-based scoring (Prompt #7+).
/// Currently enforces a single configurable maximum transaction amount.
///
/// # Examples
///
/// ```rust
/// use blazil_engine::handlers::risk::RiskHandler;
/// use blazil_engine::handler::EventHandler;
/// use blazil_engine::event::TransactionEvent;
/// use blazil_common::ids::{AccountId, LedgerId, TransactionId};
/// use blazil_common::amount::Amount;
/// use blazil_common::currency::parse_currency;
/// use rust_decimal::Decimal;
///
/// let usd = parse_currency("USD").unwrap();
/// let max = Amount::new(Decimal::new(1_000_000_00, 2), usd.clone()).unwrap();
/// let mut handler = RiskHandler::new(max);
///
/// let small = Amount::new(Decimal::new(10_00, 2), usd).unwrap();
/// let mut event = TransactionEvent::new(
///     TransactionId::new(), AccountId::new(), AccountId::new(),
///     small, LedgerId::USD, 1,
/// );
/// event.flags.requires_risk_check = true;
/// handler.on_event(&mut event, 0, true);
/// assert!(event.result.is_none()); // within limit
/// ```
pub struct RiskHandler {
    max_transaction_amount: Amount,
}

impl RiskHandler {
    /// Creates a new `RiskHandler` with the given maximum transaction amount.
    ///
    /// Transactions whose `amount` exceeds `max_transaction_amount` are
    /// rejected when `flags.requires_risk_check` is `true`.
    pub fn new(max_transaction_amount: Amount) -> Self {
        Self { max_transaction_amount }
    }
}

impl EventHandler for RiskHandler {
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

        // Rule 2: skip if risk check is not required for this event.
        if !event.flags.requires_risk_check {
            return;
        }

        // Rule 3: reject if amount exceeds the configured limit.
        // We compare values (ignoring currency — both should be in the same
        // currency denomination as the limit; enforcement is at the caller).
        if event.amount.value() > self.max_transaction_amount.value() {
            warn!(
                sequence,
                transaction_id = %event.transaction_id,
                amount = %event.amount.value(),
                max = %self.max_transaction_amount.value(),
                "RiskHandler: transaction exceeds maximum amount limit"
            );
            event.result = Some(TransactionResult::Rejected {
                reason: BlazerError::ValidationError(
                    "transaction exceeds maximum amount limit".into(),
                ),
            });
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use blazil_common::amount::Amount;
    use blazil_common::currency::parse_currency;
    use blazil_common::ids::{AccountId, LedgerId, TransactionId};
    use rust_decimal::Decimal;

    fn make_handler() -> RiskHandler {
        let usd = parse_currency("USD").unwrap();
        let max = Amount::new(Decimal::new(1_000_000_00, 2), usd).unwrap();
        RiskHandler::new(max)
    }

    fn make_event_with_amount(cents: i64) -> TransactionEvent {
        let usd = parse_currency("USD").unwrap();
        let amount = Amount::new(Decimal::new(cents, 2), usd).unwrap();
        let mut event = TransactionEvent::new(
            TransactionId::new(),
            AccountId::new(),
            AccountId::new(),
            amount,
            LedgerId::USD,
            1,
        );
        event.flags.requires_risk_check = true;
        event
    }

    #[test]
    fn amount_below_limit_result_remains_none() {
        let mut handler = make_handler();
        let mut event = make_event_with_amount(100_00); // $100
        handler.on_event(&mut event, 0, true);
        assert!(event.result.is_none());
    }

    #[test]
    fn amount_above_limit_is_rejected() {
        let mut handler = make_handler();
        // Max is $1,000,000 → $1,000,001 should be rejected
        let mut event = make_event_with_amount(1_000_001_00);
        handler.on_event(&mut event, 0, true);
        assert!(event.is_rejected());
    }

    #[test]
    fn already_rejected_event_is_unchanged() {
        let mut handler = make_handler();
        let mut event = make_event_with_amount(1_000_001_00);
        event.result = Some(TransactionResult::Rejected {
            reason: BlazerError::ValidationError("upstream".into()),
        });
        let original = match &event.result {
            Some(TransactionResult::Rejected { reason }) => reason.to_string(),
            _ => panic!(),
        };
        handler.on_event(&mut event, 0, true);
        let current = match &event.result {
            Some(TransactionResult::Rejected { reason }) => reason.to_string(),
            _ => panic!(),
        };
        assert_eq!(original, current);
    }

    #[test]
    fn risk_check_skipped_when_flag_is_false() {
        let mut handler = make_handler();
        let usd = parse_currency("USD").unwrap();
        let amount = Amount::new(Decimal::new(999_999_999_00, 2), usd).unwrap();
        let mut event = TransactionEvent::new(
            TransactionId::new(),
            AccountId::new(),
            AccountId::new(),
            amount,
            LedgerId::USD,
            1,
        );
        // requires_risk_check defaults to false
        handler.on_event(&mut event, 0, true);
        assert!(event.result.is_none()); // skipped
    }
}
