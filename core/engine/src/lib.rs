//! # Blazil Transaction Engine
//!
//! Ultra-high-performance transaction processing engine using a Disruptor-based
//! ring-buffer pipeline. Targets 10 M transactions per second with sub-microsecond
//! handler latencies.
//!
//! ## Pipeline stages (in order)
//!
//! 1. [`handlers::validation::ValidationHandler`] — structural field checks
//! 2. [`handlers::risk::RiskHandler`] — configurable amount limits
//! 3. [`handlers::ledger::LedgerHandler`] — TigerBeetle commit
//! 4. [`handlers::publish::PublishHandler`] — egress / metrics recording
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use blazil_engine::pipeline::PipelineBuilder;
//! use blazil_engine::handlers::validation::ValidationHandler;
//!
//! let builder = PipelineBuilder::new();
//! let results = builder.results();
//! let (pipeline, runner) = builder
//!     .add_handler(ValidationHandler::new(results))
//!     .build()
//!     .expect("valid capacity");
//!
//! let _handle = runner.run();
//! pipeline.stop();
//! ```

// ── modules ───────────────────────────────────────────────────────────────────

pub mod event;
pub mod handler;
pub mod handlers;
pub mod metrics;
pub mod pipeline;
pub mod ring_buffer;
pub mod sequence;
pub mod simd;

// ── re-exports ────────────────────────────────────────────────────────────────

pub use event::{EventFlags, TransactionEvent, TransactionResult};
pub use handler::EventHandler;
pub use metrics::EngineMetrics;
pub use pipeline::{Pipeline, PipelineBuilder, PipelineRunner};
