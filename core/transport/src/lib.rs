//! # Blazil Transport Layer
//!
//! The network ingestion front door for Blazil. Every transaction from
//! the outside world enters here.
//!
//! ## Architecture
//!
//! ```text
//! TCP client
//!    │  [4-byte len | MessagePack payload]
//!    ▼
//! TcpTransportServer (tcp.rs)
//!    │  accept loop → spawn task per connection
//!    ▼
//! handle_connection (connection.rs)
//!    │  read_frame → deserialize → build TransactionEvent
//!    ▼
//! Pipeline::publish_event (blazil-engine)
//!    │  wait for TransactionResult
//!    ▼
//! TransactionResponse → write_frame → TCP client
//! ```
//!
//! ## Modules
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`protocol`] | Wire frames and MessagePack messages |
//! | [`server`] | Abstract [`server::TransportServer`] trait |
//! | [`tcp`] | [`tcp::TcpTransportServer`] implementation |
//! | [`connection`] | Per-connection request handler |
//! | [`backpressure`] | Ring buffer fill ratio guard |
//! | [`mock`] | [`mock::MockTransportClient`] for integration tests |

pub mod backpressure;
pub mod connection;
pub mod mock;
pub mod protocol;
pub mod server;
pub mod tcp;

pub use protocol::{TransactionRequest, TransactionResponse};
pub use server::TransportServer;
pub use tcp::TcpTransportServer;
