//! Risk check handler.
//!
//! [`RiskHandler`] is the **second** stage in the pipeline. It applies
//! configurable transaction limits. In future prompts, this will be
//! extended with ML-based fraud scoring.
//!
//! # Rules (applied in order)
//!
//! 1. Skip if result already exists in results map (already rejected upstream).
//! 2. Skip if `event.flags.requires_risk_check()` returns `false`.
//! 3. If `amount_units > max_amount_units` → reject with
//!    `BlazerError::ValidationError("transaction exceeds maximum amount limit")`.

use std::sync::Arc;

use blazil_common::error::BlazerError;
use dashmap::DashMap;
use tracing::warn;

use crate::event::{TransactionEvent, TransactionResult};
use crate::handler::EventHandler;

// ── RiskHandler ───────────────────────────────────────────────────────────────

/// Basic risk checks.
///
/// Placeholder for ML-based scoring (Prompt #7+).
/// Currently enforces a single configurable maximum transaction amount in
/// minor units (e.g. cents for USD).
///
/// # Examples
///
/// ```rust
/// use std::sync::Arc;
/// use dashmap::DashMap;
/// use blazil_engine::handlers::risk::RiskHandler;
/// use blazil_engine::handler::EventHandler;
/// use blazil_engine::event::TransactionEvent;
/// use blazil_common::ids::{AccountId, LedgerId, TransactionId};
///
/// let results = Arc::new(DashMap::new());
/// let max_cents = 100_000_000_u64; // $1,000,000.00
/// let mut handler = RiskHandler::new(max_cents, Arc::clone(&results));
///
/// let mut event = TransactionEvent::new(
///     TransactionId::new(), AccountId::new(), AccountId::new(),
///     10_00_u64, LedgerId::USD, 1,  // $10.00
/// );
/// event.flags.set_requires_risk_check(true);
/// handler.on_event(&mut event, 0, true);
/// assert!(!results.contains_key(&0)); // within limit
/// ```
#[derive(Clone)]
pub struct RiskHandler {
    max_amount_units: u64,
    results: Arc<DashMap<i64, TransactionResult>>,
}

impl RiskHandler {
    /// Creates a new `RiskHandler` with the given maximum amount in minor units.
    ///
    /// Transactions whose `amount_units` exceeds `max_amount_units` are
    /// rejected when `flags.requires_risk_check()` is `true`.
    pub fn new(max_amount_units: u64, results: Arc<DashMap<i64, TransactionResult>>) -> Self {
        Self {
            max_amount_units,
            results,
        }
    }
}

impl EventHandler for RiskHandler {
    fn on_event(&mut self, event: &mut TransactionEvent, sequence: i64, _end_of_batch: bool) {
        // Rule 1: skip if already rejected.
        if self.results.contains_key(&sequence) {
            return;
        }

        // Rule 2: skip if risk check is not required for this event.
        if !event.flags.requires_risk_check() {
            return;
        }

        // Rule 3: reject if amount_units exceeds the configured limit.
        if event.amount_units > self.max_amount_units {
            warn!(
                sequence,
                transaction_id = %event.transaction_id,
                amount_units = event.amount_units,
                max_units = self.max_amount_units,
                "RiskHandler: transaction exceeds maximum amount limit"
            );
            self.results.insert(
                sequence,
                TransactionResult::Rejected {
                    reason: BlazerError::ValidationError(
                        "transaction exceeds maximum amount limit".into(),
                    ),
                },
            );
        }
    }

    fn clone_handler(&self) -> Box<dyn EventHandler> {
        Box::new(self.clone())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use blazil_common::ids::{AccountId, LedgerId, TransactionId};
    use dashmap::DashMap;

    use super::*;

    /// Max = $1,000,000.00 = 100_000_000 cents.
    const MAX_CENTS: u64 = 100_000_000;

    fn make_handler(results: Arc<DashMap<i64, TransactionResult>>) -> RiskHandler {
        RiskHandler::new(MAX_CENTS, results)
    }

    fn make_event_with_units(amount_units: u64) -> TransactionEvent {
        let mut event = TransactionEvent::new(
            TransactionId::new(),
            AccountId::new(),
            AccountId::new(),
            amount_units,
            LedgerId::USD,
            1,
        );
        event.flags.set_requires_risk_check(true);
        event
    }

    #[test]
    fn amount_below_limit_result_remains_none() {
        let results: Arc<DashMap<i64, TransactionResult>> = Arc::new(DashMap::new());
        let mut handler = make_handler(Arc::clone(&results));
        let mut event = make_event_with_units(10_000); // $100.00
        handler.on_event(&mut event, 0, true);
        assert!(
            !results.contains_key(&0),
            "below-limit amount must not produce a result"
        );
    }

    #[test]
    fn amount_above_limit_is_rejected() {
        let results: Arc<DashMap<i64, TransactionResult>> = Arc::new(DashMap::new());
        let mut handler = make_handler(Arc::clone(&results));
        // $1,000,000.01 → should be rejected
        let mut event = make_event_with_units(MAX_CENTS + 1);
        handler.on_event(&mut event, 0, true);
        assert!(
            matches!(
                results.get(&0).as_deref(),
                Some(TransactionResult::Rejected { .. })
            ),
            "over-limit amount must be rejected"
        );
    }

    #[test]
    fn already_rejected_event_is_unchanged() {
        use blazil_common::error::BlazerError;

        let results: Arc<DashMap<i64, TransactionResult>> = Arc::new(DashMap::new());
        results.insert(
            0,
            TransactionResult::Rejected {
                reason: BlazerError::ValidationError("upstream".into()),
            },
        );
        let original = match results.get(&0).as_deref() {
            Some(TransactionResult::Rejected { reason }) => reason.to_string(),
            _ => panic!("expected pre-set rejection"),
        };

        let mut handler = make_handler(Arc::clone(&results));
        let mut event = make_event_with_units(MAX_CENTS + 1);
        handler.on_event(&mut event, 0, true);

        let current = match results.get(&0).as_deref() {
            Some(TransactionResult::Rejected { reason }) => reason.to_string(),
            _ => panic!("expected rejection after handler"),
        };
        assert_eq!(
            original, current,
            "pre-existing rejection must not be overwritten"
        );
    }

    #[test]
    fn risk_check_skipped_when_flag_is_false() {
        let results: Arc<DashMap<i64, TransactionResult>> = Arc::new(DashMap::new());
        let mut handler = make_handler(Arc::clone(&results));
        let mut event = TransactionEvent::new(
            TransactionId::new(),
            AccountId::new(),
            AccountId::new(),
            MAX_CENTS + 1_000_000, // way over the limit
            LedgerId::USD,
            1,
        );
        // requires_risk_check defaults to false → skip
        handler.on_event(&mut event, 0, true);
        assert!(
            !results.contains_key(&0),
            "risk check skipped when flag is false"
        );
    }
}
