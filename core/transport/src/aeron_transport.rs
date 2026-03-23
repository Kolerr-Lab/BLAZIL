//! Backward-compatible re-export shim for the embedded-C-driver Aeron transport.
//!
//! The actual implementation lives in [`crate::aeron::transport`].
//! This module exists so that existing `use blazil_transport::aeron_transport::…`
//! call-sites continue to compile without changes.
//!
//! Compiled only when the `aeron` crate feature is enabled.

pub use crate::aeron::transport::{
    AeronTransportServer, DEFAULT_AERON_CHANNEL, REQ_STREAM_ID, RSP_STREAM_ID,
};

// ── Historical documentation (kept for reference) ─────────────────────────────
//
// Prior to v0.2 this file contained a pure-Rust implementation using the
// `aeron-rs` crate that spawned `aeronmd` as a subprocess.
//
// v0.2 replaces that with an **embedded C Media Driver** (`blazil-aeron-sys`
// crate) that runs entirely in-process on a dedicated Rust thread — no
// external binary required.
//
// See `aeron/transport.rs` for the full implementation and architecture docs.
//
// Original file was not deleted to preserve `BLAZIL_TRANSPORT=aeron` from other places.
