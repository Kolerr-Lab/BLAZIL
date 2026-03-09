//! `TransactionEvent` — the ring buffer slot.
//!
//! Every slot in the [`crate::ring_buffer::RingBuffer`] holds one
//! `TransactionEvent`. Slots are allocated **once** at startup via
//! [`RingBuffer::new`][crate::ring_buffer::RingBuffer::new] and reused for
//! every transaction thereafter. There is **zero** heap allocation on the hot
//! path.
//!
//! # Size budget
//!
//! The target is to fit within 2 CPU cache lines (128 bytes). Staying within
//! this budget minimises cache misses as the event travels through the
//! pipeline handlers.

use blazil_common::amount::Amount;
use blazil_common::error::BlazerError;
use blazil_common::ids::{AccountId, LedgerId, TransactionId, TransferId};
use blazil_common::timestamp::Timestamp;

// ── EventFlags ────────────────────────────────────────────────────────────────

/// Control flags for pipeline routing of a [`TransactionEvent`].
///
/// All flags default to `false` (standard, unconstrained processing).
///
/// # Examples
///
/// ```rust
/// use blazil_engine::event::EventFlags;
///
/// let flags = EventFlags::default();
/// assert!(!flags.requires_risk_check);
/// assert!(!flags.is_pending);
/// assert!(!flags.skip_publish);
/// ```
#[derive(Debug, Clone, Default)]
pub struct EventFlags {
    /// When `true`, the [`crate::handlers::risk::RiskHandler`] applies limits.
    pub requires_risk_check: bool,
    /// When `true`, this is a two-phase (pending) transfer.
    pub is_pending: bool,
    /// When `true`, the [`crate::handlers::publish::PublishHandler`] skips this event.
    pub skip_publish: bool,
}

// ── TransactionResult ─────────────────────────────────────────────────────────

/// The outcome of a processed transaction.
///
/// Set by the [`crate::handlers::ledger::LedgerHandler`] once the transaction
/// has been committed to or rejected from TigerBeetle.
///
/// # Examples
///
/// ```rust
/// use blazil_engine::event::TransactionResult;
/// use blazil_common::ids::TransferId;
/// use blazil_common::timestamp::Timestamp;
/// use blazil_common::error::BlazerError;
///
/// let committed = TransactionResult::Committed {
///     transfer_id: TransferId::new(),
///     timestamp: Timestamp::now(),
/// };
/// assert!(matches!(committed, TransactionResult::Committed { .. }));
/// ```
#[derive(Debug, Clone)]
pub enum TransactionResult {
    /// The transaction was committed successfully.
    Committed {
        /// The TigerBeetle transfer ID assigned by the ledger.
        transfer_id: TransferId,
        /// The timestamp at which the ledger committed the transfer.
        timestamp: Timestamp,
    },
    /// The transaction was rejected before or during ledger commit.
    Rejected {
        /// The reason for rejection.
        reason: BlazerError,
    },
}

// ── TransactionEvent ──────────────────────────────────────────────────────────

/// The unit of work flowing through the Blazil engine pipeline.
///
/// Allocated **once** per ring buffer slot at startup. Reused for every
/// transaction — zero heap allocation on the hot path.
///
/// # Size target
///
/// Fit within 2 CPU cache lines (128 bytes) to minimise cache misses as the
/// event travels through handlers.
///
/// # Examples
///
/// ```rust
/// use blazil_engine::event::TransactionEvent;
/// use blazil_common::ids::{AccountId, LedgerId, TransactionId};
/// use blazil_common::amount::Amount;
/// use blazil_common::currency::parse_currency;
/// use rust_decimal::Decimal;
///
/// let usd = parse_currency("USD").unwrap();
/// let amount = Amount::new(Decimal::new(100_00, 2), usd).unwrap();
/// let event = TransactionEvent::new(
///     TransactionId::new(),
///     AccountId::new(),
///     AccountId::new(),
///     amount,
///     LedgerId::USD,
///     1,
/// );
/// assert!(event.result.is_none());
/// assert!(!event.is_committed());
/// assert!(!event.is_rejected());
/// ```
#[derive(Debug)]
pub struct TransactionEvent {
    /// Monotonic sequence number assigned by the ring buffer.
    pub sequence: i64,

    /// The unique identifier for this transaction.
    pub transaction_id: TransactionId,

    /// Source account (money leaves here).
    pub debit_account_id: AccountId,

    /// Destination account (money arrives here).
    pub credit_account_id: AccountId,

    /// Amount to transfer.
    pub amount: Amount,

    /// The ledger this transaction belongs to.
    pub ledger_id: LedgerId,

    /// Application-level transaction type code.
    pub code: u16,

    /// When this event entered the pipeline.
    pub ingestion_timestamp: Timestamp,

    /// Processing result — `None` until [`crate::handlers::ledger::LedgerHandler`] completes.
    pub result: Option<TransactionResult>,

    /// Flags for pipeline control.
    pub flags: EventFlags,
}

impl TransactionEvent {
    /// Creates a new event with default flags and no result.
    ///
    /// `sequence` is set to `-1` (unassigned) and will be overwritten by the
    /// ring buffer when the event is published.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::event::TransactionEvent;
    /// use blazil_common::ids::{AccountId, LedgerId, TransactionId};
    /// use blazil_common::amount::Amount;
    /// use blazil_common::currency::parse_currency;
    /// use rust_decimal::Decimal;
    ///
    /// let usd = parse_currency("USD").unwrap();
    /// let amount = Amount::new(Decimal::new(50_00, 2), usd).unwrap();
    /// let event = TransactionEvent::new(
    ///     TransactionId::new(), AccountId::new(), AccountId::new(),
    ///     amount, LedgerId::USD, 1,
    /// );
    /// assert_eq!(event.sequence, -1);
    /// assert!(event.result.is_none());
    /// ```
    pub fn new(
        transaction_id: TransactionId,
        debit_account_id: AccountId,
        credit_account_id: AccountId,
        amount: Amount,
        ledger_id: LedgerId,
        code: u16,
    ) -> Self {
        Self {
            sequence: -1,
            transaction_id,
            debit_account_id,
            credit_account_id,
            amount,
            ledger_id,
            code,
            ingestion_timestamp: Timestamp::now(),
            result: None,
            flags: EventFlags::default(),
        }
    }

    /// Returns `true` if this event has a `Committed` result.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::event::{TransactionEvent, TransactionResult};
    /// use blazil_common::ids::{AccountId, LedgerId, TransactionId, TransferId};
    /// use blazil_common::amount::Amount;
    /// use blazil_common::currency::parse_currency;
    /// use blazil_common::timestamp::Timestamp;
    /// use rust_decimal::Decimal;
    ///
    /// let usd = parse_currency("USD").unwrap();
    /// let amount = Amount::new(Decimal::new(10_00, 2), usd).unwrap();
    /// let mut event = TransactionEvent::new(
    ///     TransactionId::new(), AccountId::new(), AccountId::new(),
    ///     amount, LedgerId::USD, 1,
    /// );
    /// assert!(!event.is_committed());
    /// event.result = Some(TransactionResult::Committed {
    ///     transfer_id: TransferId::new(),
    ///     timestamp: Timestamp::now(),
    /// });
    /// assert!(event.is_committed());
    /// ```
    pub fn is_committed(&self) -> bool {
        matches!(self.result, Some(TransactionResult::Committed { .. }))
    }

    /// Returns `true` if this event has a `Rejected` result.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::event::{TransactionEvent, TransactionResult};
    /// use blazil_common::ids::{AccountId, LedgerId, TransactionId};
    /// use blazil_common::amount::Amount;
    /// use blazil_common::currency::parse_currency;
    /// use blazil_common::error::BlazerError;
    /// use rust_decimal::Decimal;
    ///
    /// let usd = parse_currency("USD").unwrap();
    /// let amount = Amount::new(Decimal::new(10_00, 2), usd).unwrap();
    /// let mut event = TransactionEvent::new(
    ///     TransactionId::new(), AccountId::new(), AccountId::new(),
    ///     amount, LedgerId::USD, 1,
    /// );
    /// assert!(!event.is_rejected());
    /// event.result = Some(TransactionResult::Rejected {
    ///     reason: BlazerError::ValidationError("test".into()),
    /// });
    /// assert!(event.is_rejected());
    /// ```
    pub fn is_rejected(&self) -> bool {
        matches!(self.result, Some(TransactionResult::Rejected { .. }))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use blazil_common::amount::Amount;
    use blazil_common::currency::parse_currency;
    use rust_decimal::Decimal;

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
    fn new_sets_result_to_none() {
        let event = make_event();
        assert!(event.result.is_none());
    }

    #[test]
    fn new_sets_sequence_to_minus_one() {
        let event = make_event();
        assert_eq!(event.sequence, -1);
    }

    #[test]
    fn new_sets_ingestion_timestamp() {
        let event = make_event();
        assert!(event.ingestion_timestamp.as_nanos() > 0);
    }

    #[test]
    fn is_committed_false_on_new_event() {
        assert!(!make_event().is_committed());
    }

    #[test]
    fn is_rejected_false_on_new_event() {
        assert!(!make_event().is_rejected());
    }

    #[test]
    fn is_committed_true_after_setting_committed_result() {
        let mut event = make_event();
        event.result = Some(TransactionResult::Committed {
            transfer_id: TransferId::new(),
            timestamp: Timestamp::now(),
        });
        assert!(event.is_committed());
        assert!(!event.is_rejected());
    }

    #[test]
    fn is_rejected_true_after_setting_rejected_result() {
        let mut event = make_event();
        event.result = Some(TransactionResult::Rejected {
            reason: BlazerError::ValidationError("test".into()),
        });
        assert!(event.is_rejected());
        assert!(!event.is_committed());
    }

    #[test]
    fn default_flags_are_all_false() {
        let flags = EventFlags::default();
        assert!(!flags.requires_risk_check);
        assert!(!flags.is_pending);
        assert!(!flags.skip_publish);
    }
}
