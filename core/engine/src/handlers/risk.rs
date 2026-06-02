//! Risk check handler.
//!
//! [`RiskHandler`] is the **second** stage in the pipeline. It applies
//! configurable transaction limits and optional ML-based fraud scoring.
//!
//! # Rules (applied in order)
//!
//! 1. Skip if result already exists in results map (already rejected upstream).
//! 2. Skip if `event.flags.requires_risk_check()` returns `false`.
//! 3. If `amount_units > max_amount_units` → reject.
//! 4. If `fraud_scorer.score() >= fraud_threshold` → reject.
//!
//! # Fraud scoring
//!
//! [`FraudScorer`] is a synchronous, non-blocking trait called on the ring
//! buffer hot path. The default [`NoopFraudScorer`] passes every transaction.
//! A production scorer backs the result with a `DashMap<AccountId, f32>`
//! updated asynchronously by a background task polling the inference service
//! over Aeron IPC (stream 2001/2002). Use [`RiskHandler::with_fraud_scorer`]
//! to plug in a custom scorer without changing any existing call site.

use std::sync::Arc;

use blazil_common::{error::BlazerError, ids::AccountId};
use dashmap::DashMap;
use tracing::warn;

use crate::event::{TransactionEvent, TransactionResult};
use crate::handler::EventHandler;

// ── FraudScorer ────────────────────────────────────────────────────────────────────────────

/// Synchronous fraud probability provider.
///
/// Implementations **must not block** — `score` is called on the ring buffer
/// hot path and must return in nanoseconds. Typically backed by a pre-computed
/// `DashMap<AccountId, f32>` that a background task updates asynchronously by
/// querying the inference service over Aeron IPC (stream 2001/2002).
///
/// Return value is a fraud probability in `[0.0, 1.0]`:
/// - `0.0` — no fraud signal (pass through)
/// - `1.0` — maximum fraud signal (reject)
/// - Return `0.0` when no cached score is available for this account.
pub trait FraudScorer: Send + Sync + 'static {
    /// Returns a fraud probability for `(debit_account_id, amount_units)`.
    fn score(&self, debit_account_id: &AccountId, amount_units: u64) -> f32;
}

// ── NoopFraudScorer ──────────────────────────────────────────────────────────────

/// No-op scorer — always returns `0.0` (every transaction passes fraud check).
///
/// Default when [`RiskHandler::new`] is used. Replace with a production scorer
/// (e.g. backed by the inference service) via [`RiskHandler::with_fraud_scorer`]
/// without changing any existing call site.
#[derive(Debug, Clone, Default)]
pub struct NoopFraudScorer;

impl FraudScorer for NoopFraudScorer {
    #[inline(always)]
    fn score(&self, _debit_account_id: &AccountId, _amount_units: u64) -> f32 {
        0.0
    }
}

// ── RiskHandler ───────────────────────────────────────────────────────────────

/// Risk check handler: amount limits + optional fraud scoring.
///
/// Use [`RiskHandler::new`] for basic amount-limit enforcement — all existing
/// call sites need no changes. Use [`RiskHandler::with_fraud_scorer`] to plug
/// in a [`FraudScorer`] for ML-backed fraud detection in production.
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
/// assert!(!results.contains_key(&0)); // within limit → approved
/// ```
#[derive(Clone)]
pub struct RiskHandler {
    max_amount_units: u64,
    /// Reject when fraud score ≥ threshold. Range `[0.0, 1.0]`.
    /// Default `1.0` with `NoopFraudScorer` — effectively disabled.
    /// Set to e.g. `0.85` in production to catch high-confidence fraud.
    fraud_threshold: f32,
    fraud_scorer: Arc<dyn FraudScorer>,
    results: Arc<DashMap<i64, TransactionResult>>,
}

impl RiskHandler {
    /// Creates a `RiskHandler` using the default [`NoopFraudScorer`].
    ///
    /// All existing call sites continue to work unchanged. Transactions are
    /// rejected only when `amount_units > max_amount_units`.
    pub fn new(max_amount_units: u64, results: Arc<DashMap<i64, TransactionResult>>) -> Self {
        Self {
            max_amount_units,
            fraud_threshold: 1.0,
            fraud_scorer: Arc::new(NoopFraudScorer),
            results,
        }
    }

    /// Creates a `RiskHandler` with a custom fraud scorer and threshold.
    ///
    /// `fraud_threshold` is clamped to `[0.0, 1.0]`. Transactions whose
    /// fraud score meets or exceeds `fraud_threshold` are rejected.
    /// Typical production value: `0.85`.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use std::sync::Arc;
    /// # use dashmap::DashMap;
    /// # use blazil_engine::handlers::risk::{RiskHandler, FraudScorer, NoopFraudScorer};
    /// # use blazil_engine::event::TransactionResult;
    /// let results = Arc::new(DashMap::<i64, TransactionResult>::new());
    /// let scorer = Arc::new(NoopFraudScorer);
    /// let handler = RiskHandler::with_fraud_scorer(
    ///     100_000_000, // $1,000,000.00 max
    ///     results,
    ///     scorer,
    ///     0.85,        // reject if fraud score ≥ 85%
    /// );
    /// ```
    pub fn with_fraud_scorer(
        max_amount_units: u64,
        results: Arc<DashMap<i64, TransactionResult>>,
        fraud_scorer: Arc<dyn FraudScorer>,
        fraud_threshold: f32,
    ) -> Self {
        Self {
            max_amount_units,
            fraud_threshold: fraud_threshold.clamp(0.0, 1.0),
            fraud_scorer,
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
            return;
        }

        // Rule 4: reject if fraud score meets or exceeds the threshold.
        // The scorer is synchronous and non-blocking (ring-buffer hot path).
        // NoopFraudScorer always returns 0.0, so this rule is a no-op by default.
        let fraud_score = self
            .fraud_scorer
            .score(&event.debit_account_id, event.amount_units);
        if fraud_score >= self.fraud_threshold {
            warn!(
                sequence,
                transaction_id = %event.transaction_id,
                fraud_score,
                threshold = self.fraud_threshold,
                "RiskHandler: transaction rejected by fraud scorer"
            );
            self.results.insert(
                sequence,
                TransactionResult::Rejected {
                    reason: BlazerError::ValidationError(format!(
                        "fraud score {fraud_score:.3} meets or exceeds threshold {:.3}",
                        self.fraud_threshold
                    )),
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

    #[test]
    fn fraud_scorer_above_threshold_is_rejected() {
        struct AlwaysFraudScorer;
        impl FraudScorer for AlwaysFraudScorer {
            fn score(&self, _: &AccountId, _: u64) -> f32 {
                0.99
            }
        }

        let results: Arc<DashMap<i64, TransactionResult>> = Arc::new(DashMap::new());
        let mut handler = RiskHandler::with_fraud_scorer(
            MAX_CENTS,
            Arc::clone(&results),
            Arc::new(AlwaysFraudScorer),
            0.85,
        );
        let mut event = make_event_with_units(100); // amount is fine
        handler.on_event(&mut event, 0, true);
        assert!(
            matches!(
                results.get(&0).as_deref(),
                Some(TransactionResult::Rejected { .. })
            ),
            "fraud score 0.99 >= threshold 0.85 must be rejected"
        );
    }

    #[test]
    fn fraud_scorer_below_threshold_passes() {
        struct LowFraudScorer;
        impl FraudScorer for LowFraudScorer {
            fn score(&self, _: &AccountId, _: u64) -> f32 {
                0.50
            }
        }

        let results: Arc<DashMap<i64, TransactionResult>> = Arc::new(DashMap::new());
        let mut handler = RiskHandler::with_fraud_scorer(
            MAX_CENTS,
            Arc::clone(&results),
            Arc::new(LowFraudScorer),
            0.85,
        );
        let mut event = make_event_with_units(100);
        handler.on_event(&mut event, 0, true);
        assert!(
            !results.contains_key(&0),
            "fraud score 0.50 < threshold 0.85 must pass"
        );
    }

    #[test]
    fn noop_scorer_never_rejects_by_fraud() {
        let results: Arc<DashMap<i64, TransactionResult>> = Arc::new(DashMap::new());
        // threshold=0.5: would reject if scorer returns >= 0.5.
        // NoopFraudScorer always returns 0.0 → must pass.
        let mut handler = RiskHandler::with_fraud_scorer(
            MAX_CENTS,
            Arc::clone(&results),
            Arc::new(NoopFraudScorer),
            0.5,
        );
        let mut event = make_event_with_units(100);
        handler.on_event(&mut event, 0, true);
        assert!(
            !results.contains_key(&0),
            "NoopFraudScorer (score=0.0) must never trigger fraud rejection"
        );
    }
}
