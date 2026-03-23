//! Safe Rust wrappers for the Aeron C Media Driver.
//!
//! This module provides an embedded Aeron transport layer that compiles only
//! under `#[cfg(feature = "aeron")]`.
//!
//! # Component overview
//!
//! | File | Responsibility |
//! |------|----------------|
//! | [`driver`] | [`EmbeddedAeronDriver`] — in-process C Media Driver |
//! | [`context`] | [`AeronContext`] — client-side `aeron_context_t` + `aeron_t` |
//! | [`publication`] | [`AeronPublication`] — safe `offer(data)` wrapper |
//! | [`subscription`] | [`AeronSubscription`] — safe `poll(handler)` wrapper |
//! | [`transport`] | [`AeronTransportServer`] — `TransportServer` impl |
//!
//! # Drop ordering (critical for safety)
//!
//! Resources must be dropped in this order to avoid use-after-free:
//! 1. All [`AeronPublication`]s and [`AeronSubscription`]s
//! 2. [`AeronContext`]
//! 3. [`EmbeddedAeronDriver`]
//!
//! The [`AeronTransportServer::serve`] loop enforces this ordering.

pub mod context;
pub mod driver;
pub mod publication;
pub mod subscription;
pub mod transport;

// Convenient re-exports for external callers (e.g., integration tests, bench).
pub use context::AeronContext;
pub use driver::EmbeddedAeronDriver;
pub use publication::AeronPublication;
pub use subscription::AeronSubscription;
pub use transport::{AeronTransportServer, DEFAULT_AERON_CHANNEL, REQ_STREAM_ID, RSP_STREAM_ID};
