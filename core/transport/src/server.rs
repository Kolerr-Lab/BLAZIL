//! Abstract transport server interface.
//!
//! [`TransportServer`] is the trait every network backend implements.
//! Today: [`crate::tcp::TcpTransportServer`].
//! Future (Prompt #8): `AeronTransportServer`.
//!
//! Implementations are injected at the application root, so all business
//! logic in `connection.rs` depends only on this trait.

use async_trait::async_trait;
use blazil_common::error::BlazerResult;

// ── TransportServer ───────────────────────────────────────────────────────────

/// Abstract interface for a Blazil network ingestion server.
///
/// Implementors receive raw client connections, deserialize
/// [`crate::protocol::TransactionRequest`]s, feed them into the engine
/// pipeline, and return [`crate::protocol::TransactionResponse`]s.
///
/// # Contract
///
/// - [`serve`][TransportServer::serve] runs until [`shutdown`][TransportServer::shutdown]
///   is called from another task.
/// - [`shutdown`][TransportServer::shutdown] must not drop in-flight requests.
///   It sets a flag and waits for active connections to drain.
/// - [`local_addr`][TransportServer::local_addr] returns the bound address
///   (including the OS-assigned port when bound to `0`).
///
/// # Examples
///
/// ```rust,no_run
/// use std::sync::Arc;
/// use blazil_transport::server::TransportServer;
/// use blazil_transport::tcp::TcpTransportServer;
/// use blazil_engine::pipeline::{Pipeline, PipelineBuilder};
/// use blazil_engine::event::TransactionResult;
/// use dashmap::DashMap;
///
/// # async fn example(pipeline: Arc<Pipeline>, results: Arc<DashMap<i64, TransactionResult>>) {
/// let server = TcpTransportServer::new("127.0.0.1:0", pipeline, results, 1000);
/// let server = Arc::new(server);
/// let s = Arc::clone(&server);
/// tokio::spawn(async move { s.serve().await });
/// // ... later ...
/// server.shutdown().await;
/// # }
/// ```
#[async_trait]
pub trait TransportServer: Send + Sync {
    /// Start listening and processing connections.
    ///
    /// This future runs until [`shutdown`][TransportServer::shutdown] is
    /// called. Implementations must accept connections in a loop and spawn
    /// a task per connection.
    ///
    /// # Errors
    ///
    /// Returns [`blazil_common::error::BlazerError::Transport`] if the server
    /// cannot bind to its address.
    async fn serve(&self) -> BlazerResult<()>;

    /// Gracefully stop the server.
    ///
    /// Sets a shutdown flag and waits up to 5 seconds for all active
    /// connections to finish processing their current request.
    async fn shutdown(&self);

    /// Returns the address string this server is bound to.
    ///
    /// When the server was created with port `0`, this returns the
    /// OS-assigned address (e.g. `"127.0.0.1:54321"`).
    fn local_addr(&self) -> &str;
}
