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
//! | [`aeron_transport`] | [`aeron_transport::AeronTransportServer`] (feature = "aeron") |
//! | [`io_uring_transport`] | [`io_uring_transport::IoUringTransportServer`] (feature = "io-uring", Linux only) |
//! | [`connection`] | Per-connection request handler |
//! | [`backpressure`] | Ring buffer fill ratio guard |
//! | [`mock`] | [`mock::MockTransportClient`] for integration tests |

pub mod backpressure;
pub mod connection;
pub mod metrics_server;
pub mod mock;
pub mod protocol;
pub mod rate_limit;
pub mod server;
pub mod tcp;

#[cfg(feature = "aeron")]
pub mod aeron_transport;

#[cfg(all(target_os = "linux", feature = "io-uring"))]
pub mod io_uring_transport;

pub use protocol::{TransactionRequest, TransactionResponse};
pub use server::TransportServer;
pub use tcp::TcpTransportServer;

#[cfg(feature = "aeron")]
pub use aeron_transport::AeronTransportServer;

#[cfg(all(target_os = "linux", feature = "io-uring"))]
pub use io_uring_transport::IoUringTransportServer;
