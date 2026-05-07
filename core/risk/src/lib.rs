//! # Blazil Risk Engine
//!
//! Pre-trade risk checks and position tracking for financial compliance and safety.
//!
//! ## Overview
//!
//! The risk module provides:
//! - **Position tracking**: Track long/short positions across instruments
//! - **Limit enforcement**: Validate orders against position/notional limits
//! - **Risk engine trait**: Extensible interface for custom risk logic
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────┐
//! │  OrderRequest   │  ← Order details (account, instrument, qty, price)
//! └────────┬────────┘
//!          │
//!          ▼
//! ┌─────────────────┐
//! │  RiskEngine     │  ← check_order() validates limits
//! │  (trait)        │    update_position() tracks execution
//! └────────┬────────┘
//!          │
//!          ├─→ RiskLimit (max position, notional, order size)
//!          └─→ Position  (quantity, notional, avg price)
//! ```
//!
//! ## Usage
//!
//! ```rust
//! use blazil_risk::engine::{InMemoryRiskEngine, RiskEngine, OrderRequest};
//! use blazil_risk::limit::RiskLimit;
//! use rust_decimal::Decimal;
//!
//! # tokio_test::block_on(async {
//! let engine = InMemoryRiskEngine::new();
//!
//! // Register account with retail limits
//! engine.register_account("alice".to_string(), RiskLimit::retail()).await;
//!
//! // Check order before execution
//! let order = OrderRequest::new(
//!     "alice".to_string(),
//!     "AAPL".to_string(),
//!     Decimal::new(50, 0),   // 50 shares
//!     Decimal::new(150, 0),  // $150/share
//! );
//!
//! match engine.check_order(&order).await {
//!     Ok(_) => {
//!         println!("Order approved");
//!         engine.update_position(&order).await;
//!     }
//!     Err(e) => println!("Order rejected: {}", e),
//! }
//!
//! // Query positions
//! let pos = engine.get_position("alice", "AAPL").await;
//! let total_notional = engine.get_total_notional("alice").await;
//! # })
//! ```
//!
//! ## Production Considerations
//!
//! - **Atomicity**: `check_order` + `update_position` should be atomic to prevent TOCTOU
//! - **Persistence**: `InMemoryRiskEngine` is non-persistent; use Redis/DB for production
//! - **Real-time prices**: Current implementation uses order price; consider mark-to-market
//! - **Concurrency**: `DashMap` provides lock-free concurrent access

pub mod engine;
pub mod limit;
pub mod position;

pub use engine::{InMemoryRiskEngine, OrderRequest, RiskEngine, RiskError};
pub use limit::RiskLimit;
pub use position::Position;
