//! `EventHandler` trait — the contract every pipeline stage implements.
//!
//! Each handler runs on its own pinned thread and processes one
//! [`TransactionEvent`] at a time. Handlers are called in strict pipeline
//! order by the [`crate::pipeline::PipelineRunner`].
//!
//! # Implementation contract
//!
//! - `on_event` must be **non-blocking**. Never sleep, never lock a shared
//!   mutex, never perform unbounded I/O on the hot path.
//! - `on_event` should complete in < 1 microsecond on modern hardware.
//! - Use `end_of_batch` to flush batched state when the current batch ends
//!   (e.g. flushing a write buffer once per batch rather than per event).

use crate::event::TransactionEvent;

// ── EventHandler ──────────────────────────────────────────────────────────────

/// A pipeline stage that processes one [`TransactionEvent`].
///
/// Implementors receive events in strict sequence order, one at a time.
/// The pipeline runner calls handlers on a single dedicated thread, so
/// `&mut self` access is exclusive and no locking is required within a
/// handler.
///
/// # Examples
///
/// ```rust
/// use blazil_engine::handler::EventHandler;
/// use blazil_engine::event::TransactionEvent;
///
/// struct NoopHandler;
///
/// impl EventHandler for NoopHandler {
///     fn on_event(
///         &mut self,
///         _event: &mut TransactionEvent,
///         _sequence: i64,
///         _end_of_batch: bool,
///     ) {}
///
///     fn clone_handler(&self) -> Box<dyn EventHandler> {
///         Box::new(NoopHandler)
///     }
/// }
/// ```
pub trait EventHandler: Send + 'static {
    /// Processes one event from the ring buffer.
    ///
    /// Called in strict sequence order by the pipeline runner.
    ///
    /// - `event`:        mutable reference to the ring buffer slot.
    /// - `sequence`:     the monotonic sequence number of this event.
    /// - `end_of_batch`: `true` when this is the last event in the current
    ///   batch (i.e. no newer event is available yet). Use this hint to flush
    ///   any accumulated batch state.
    fn on_event(&mut self, event: &mut TransactionEvent, sequence: i64, end_of_batch: bool);

    /// Called once when the pipeline starts, before any events are processed.
    ///
    /// Override to perform one-time initialisation (e.g. opening a file,
    /// establishing a connection).
    fn on_start(&mut self) {}

    /// Called once when the pipeline shuts down gracefully.
    ///
    /// Override to release resources (e.g. flushing buffers, closing sockets).
    fn on_shutdown(&mut self) {}

    /// Clones this handler for use in a parallel worker thread.
    ///
    /// Shared state (Arc) is cloned, but worker-local state (counters, buffers)
    /// should be reset to initial values.
    fn clone_handler(&self) -> Box<dyn EventHandler>;
}
