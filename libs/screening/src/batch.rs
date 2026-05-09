// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Asynchronous batch screening worker.

use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tracing::{info, instrument};

use crate::{
    ScreeningError, ScreeningMode, ScreeningResult, TransactionEvent, TransactionScreener,
};

/// A pending batch screening job.
pub struct BatchJob {
    /// Transaction to screen.
    pub tx: TransactionEvent,
    /// Channel to deliver the screening result to the submitter.
    ///
    /// If the submitter is no longer interested, dropping the receiver
    /// is safe — the worker logs nothing and moves on.
    pub result_tx: oneshot::Sender<ScreeningResult>,
}

/// Asynchronous batch screening worker.
///
/// Dequeues `BatchJob`s from an mpsc channel and processes them through the
/// configured `TransactionScreener`. Designed to run on a single dedicated
/// Tokio task; parallelism is achieved by running multiple workers backed
/// by a shared `Arc<dyn TransactionScreener>`.
///
/// # Lifecycle
///
/// The worker runs until all `BatchSender` handles are dropped (i.e. the
/// channel sender side is closed), then exits cleanly.
///
/// ```ignore
/// let (worker, sender) = BatchWorker::new(screener, 1024);
/// tokio::spawn(worker.run());
/// ```
pub struct BatchWorker {
    screener: Arc<dyn TransactionScreener>,
    receiver: mpsc::Receiver<BatchJob>,
}

impl BatchWorker {
    /// Creates a new worker and returns the associated [`BatchSender`].
    ///
    /// `channel_capacity` is the mpsc buffer depth. Size this to absorb
    /// real-time timeout spikes without blocking the transaction pipeline.
    /// A capacity of 1024 handles ~20 s of burst at 50 timeouts/s.
    pub fn new(
        screener: Arc<dyn TransactionScreener>,
        channel_capacity: usize,
    ) -> (Self, BatchSender) {
        let (tx, rx) = mpsc::channel(channel_capacity);
        let worker = Self {
            screener,
            receiver: rx,
        };
        let sender = BatchSender { inner: tx };
        (worker, sender)
    }

    /// Runs the worker event loop.
    ///
    /// Returns when the channel is closed (all `BatchSender` handles dropped).
    /// Spawn on a dedicated task:
    ///
    /// ```ignore
    /// tokio::spawn(worker.run());
    /// ```
    pub async fn run(mut self) {
        info!(
            provider = self.screener.provider_name(),
            "batch screening worker started"
        );

        while let Some(job) = self.receiver.recv().await {
            let result = self.screener.screen(&job.tx, ScreeningMode::Batch).await;
            // Receiver may have been dropped (caller gave up waiting).
            // Silently discard — this is not an error.
            let _ = job.result_tx.send(result);
        }

        info!(
            provider = self.screener.provider_name(),
            "batch screening worker stopped — channel closed"
        );
    }
}

/// Cheaply cloneable handle for submitting jobs to a [`BatchWorker`].
///
/// All clones share the same mpsc channel. Dropping all clones signals the
/// worker to shut down.
#[derive(Clone)]
pub struct BatchSender {
    inner: mpsc::Sender<BatchJob>,
}

impl BatchSender {
    /// Submits a transaction for asynchronous batch screening.
    ///
    /// Returns a `oneshot::Receiver` through which the caller can `await`
    /// the screening result. The receiver may be dropped if the result is
    /// not needed (fire-and-forget mode).
    ///
    /// # Errors
    ///
    /// Returns `ScreeningError::BatchChannelClosed` if the worker task has
    /// exited and the channel is permanently closed.
    #[instrument(skip(self, tx), fields(tx_id = %tx.transaction_id))]
    pub async fn submit(
        &self,
        tx: TransactionEvent,
    ) -> Result<oneshot::Receiver<ScreeningResult>, ScreeningError> {
        let (result_tx, result_rx) = oneshot::channel();
        self.inner
            .send(BatchJob { tx, result_tx })
            .await
            .map_err(|_| ScreeningError::BatchChannelClosed)?;
        Ok(result_rx)
    }
}
