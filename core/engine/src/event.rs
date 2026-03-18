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
//! Target: **1 CPU cache line (64 bytes)**.
//! Layout after optimisation:
//!   6 × u64 (sequence, tx_id, debit, credit, amount_units, timestamp) = 48 B
//!   1 × u32 (ledger_id)                                                =  4 B
//!   1 × u16 (code)                                                     =  2 B
//!   1 × u8  (flags bitfield)                                           =  1 B
//!   padding                                                            =  1 B
//!   ─────────────────────────────────────────────────────────────────────────
//!   Total                                                              = 56 B  (< 64 B ✓)

use blazil_common::error::BlazerError;
use blazil_common::ids::{AccountId, LedgerId, TransactionId, TransferId};
use blazil_common::timestamp::Timestamp;

// ── EventFlags ────────────────────────────────────────────────────────────────

/// Control flags for pipeline routing of a [`TransactionEvent`].
///
/// Packed into a single `u8` bitfield so the full event fits in one cache
/// line.  All flags default to `0` (standard, unconstrained processing).
///
/// # Examples
///
/// ```rust
/// use blazil_engine::event::EventFlags;
///
/// let flags = EventFlags::default();
/// assert!(!flags.requires_risk_check());
/// assert!(!flags.is_pending());
/// assert!(!flags.skip_publish());
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct EventFlags(u8);

impl EventFlags {
    const RISK_CHECK: u8 = 0b0000_0001;
    const PENDING: u8 = 0b0000_0010;
    const SKIP_PUBLISH: u8 = 0b0000_0100;

    /// Creates EventFlags from raw u8 value (for deserialization).
    #[inline]
    pub fn from_raw(byte: u8) -> Self {
        Self(byte)
    }

    /// Returns raw u8 value (for serialization).
    #[inline]
    pub fn to_raw(&self) -> u8 {
        self.0
    }

    /// When `true`, the [`crate::handlers::risk::RiskHandler`] applies limits.
    #[inline]
    pub fn requires_risk_check(&self) -> bool {
        self.0 & Self::RISK_CHECK != 0
    }
    /// Set the risk-check flag.
    #[inline]
    pub fn set_requires_risk_check(&mut self, v: bool) {
        if v {
            self.0 |= Self::RISK_CHECK;
        } else {
            self.0 &= !Self::RISK_CHECK;
        }
    }

    /// When `true`, this is a two-phase (pending) transfer.
    #[inline]
    pub fn is_pending(&self) -> bool {
        self.0 & Self::PENDING != 0
    }
    /// Set the pending flag.
    #[inline]
    pub fn set_is_pending(&mut self, v: bool) {
        if v {
            self.0 |= Self::PENDING;
        } else {
            self.0 &= !Self::PENDING;
        }
    }

    /// When `true`, the [`crate::handlers::publish::PublishHandler`] skips this event.
    #[inline]
    pub fn skip_publish(&self) -> bool {
        self.0 & Self::SKIP_PUBLISH != 0
    }
    /// Set the skip-publish flag.
    #[inline]
    pub fn set_skip_publish(&mut self, v: bool) {
        if v {
            self.0 |= Self::SKIP_PUBLISH;
        } else {
            self.0 &= !Self::SKIP_PUBLISH;
        }
    }
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
/// Fit within 1 CPU cache line (64 bytes) to minimise cache misses as the
/// event travels through handlers. Current size: **56 bytes**.
///
/// # Examples
///
/// ```rust
/// use blazil_engine::event::TransactionEvent;
/// use blazil_common::ids::{AccountId, LedgerId, TransactionId};
///
/// let event = TransactionEvent::new(
///     TransactionId::new(),
///     AccountId::new(),
///     AccountId::new(),
///     100_00_u64,   // $100.00 in cents (minor units)
///     LedgerId::USD,
///     1,
/// );
/// // sequence starts at -1 (unassigned)
/// assert_eq!(event.sequence, -1);
/// ```
#[derive(Debug, Clone)]
pub struct TransactionEvent {
    /// Monotonic sequence number assigned by the ring buffer.
    pub sequence: i64,

    /// The unique identifier for this transaction.
    pub transaction_id: TransactionId,

    /// Source account (money leaves here).
    pub debit_account_id: AccountId,

    /// Destination account (money arrives here).
    pub credit_account_id: AccountId,

    /// Amount in minor units (e.g. cents for USD, satoshis for BTC).
    /// Validated to be > 0 by the validation handler.
    pub amount_units: u64,

    /// When this event entered the pipeline.
    pub ingestion_timestamp: Timestamp,

    /// The ledger this transaction belongs to.
    pub ledger_id: LedgerId,

    /// Application-level transaction type code.
    pub code: u16,

    /// Flags for pipeline control (bitfield).
    pub flags: EventFlags,
}

impl TransactionEvent {
    /// Creates a new event with default flags.
    ///
    /// `sequence` is set to `-1` (unassigned) and will be overwritten by the
    /// ring buffer when the event is published.
    ///
    /// `amount_units` must be in the minor unit of the ledger's currency
    /// (e.g. cents for USD, satoshis for BTC).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::event::TransactionEvent;
    /// use blazil_common::ids::{AccountId, LedgerId, TransactionId};
    ///
    /// let event = TransactionEvent::new(
    ///     TransactionId::new(), AccountId::new(), AccountId::new(),
    ///     50_00_u64, LedgerId::USD, 1,
    /// );
    /// assert_eq!(event.sequence, -1);
    /// ```
    pub fn new(
        transaction_id: TransactionId,
        debit_account_id: AccountId,
        credit_account_id: AccountId,
        amount_units: u64,
        ledger_id: LedgerId,
        code: u16,
    ) -> Self {
        Self {
            sequence: -1,
            transaction_id,
            debit_account_id,
            credit_account_id,
            amount_units,
            ingestion_timestamp: Timestamp::now(),
            ledger_id,
            code,
            flags: EventFlags::default(),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event() -> TransactionEvent {
        TransactionEvent::new(
            TransactionId::new(),
            AccountId::new(),
            AccountId::new(),
            10_000_u64, // 100.00 USD in cents
            LedgerId::USD,
            1,
        )
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
    fn default_flags_are_all_unset() {
        let flags = EventFlags::default();
        assert!(!flags.requires_risk_check());
        assert!(!flags.is_pending());
        assert!(!flags.skip_publish());
    }

    #[test]
    fn flags_setters_roundtrip() {
        let mut flags = EventFlags::default();
        flags.set_requires_risk_check(true);
        assert!(flags.requires_risk_check());
        assert!(!flags.is_pending());
        flags.set_is_pending(true);
        assert!(flags.is_pending());
        flags.set_requires_risk_check(false);
        assert!(!flags.requires_risk_check());
        assert!(flags.is_pending());
    }

    #[test]
    fn event_size_fits_one_cache_line() {
        use std::mem::size_of;
        assert!(
            size_of::<TransactionEvent>() <= 64,
            "TransactionEvent must fit in one cache line (64 bytes), got {}",
            size_of::<TransactionEvent>()
        );
    }
}
