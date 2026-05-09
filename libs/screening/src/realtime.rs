// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Real-time screening router with a hard 50 ms deadline.

use std::sync::Arc;
use std::time::Duration;
use tracing::{instrument, warn};

use crate::{ScreeningMode, ScreeningResult, TransactionEvent, TransactionScreener};

/// Maximum wall-clock time permitted for a real-time screening decision.
///
/// This is set to 50 ms to preserve sub-100 ms end-to-end transaction
/// latency. On timeout the router falls back to `Clear` (fail-open) and
/// returns `timed_out = true` so the caller can enqueue the transaction
/// for deferred batch re-screening.
///
/// Fail-open is a deliberate risk trade-off: blocking a legitimate
/// transaction due to provider latency causes more business harm than
/// deferring a compliance check by a few seconds. All timed-out
/// transactions must be re-screened in the batch path.
const REALTIME_DEADLINE: Duration = Duration::from_millis(50);

/// Routes real-time screening calls with a hard `REALTIME_DEADLINE`.
///
/// # Timeout behaviour
///
/// Returns `(ScreeningResult::Clear, true)` on timeout. The `true` flag
/// signals the caller to enqueue the transaction for batch re-screening.
/// A `warn!`-level log is emitted so operators can detect provider latency
/// regressions.
pub struct RealTimeRouter {
    screener: Arc<dyn TransactionScreener>,
}

impl RealTimeRouter {
    /// Wraps a screener in a real-time router.
    pub fn new(screener: Arc<dyn TransactionScreener>) -> Self {
        Self { screener }
    }

    /// Screens a transaction with a hard 50 ms deadline.
    ///
    /// Returns `(result, timed_out)`.
    ///
    /// When `timed_out` is `true`, the caller **must** enqueue the transaction
    /// for asynchronous batch re-screening via `BatchSender::submit`.
    #[instrument(
        skip(self, tx),
        fields(
            tx_id   = %tx.transaction_id,
            amount  = tx.amount,
            provider = self.screener.provider_name(),
        )
    )]
    pub async fn screen(&self, tx: &TransactionEvent) -> (ScreeningResult, bool) {
        match tokio::time::timeout(
            REALTIME_DEADLINE,
            self.screener.screen(tx, ScreeningMode::RealTime),
        )
        .await
        {
            Ok(result) => (result, false),
            Err(_elapsed) => {
                warn!(
                    tx_id    = %tx.transaction_id,
                    provider = self.screener.provider_name(),
                    deadline_ms = REALTIME_DEADLINE.as_millis(),
                    "screening deadline exceeded — failing open (Clear), \
                     transaction must be re-screened in batch path"
                );
                (ScreeningResult::Clear, true)
            }
        }
    }
}
