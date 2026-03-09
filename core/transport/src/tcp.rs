//! TCP-based transport server.
//!
//! [`TcpTransportServer`] implements [`TransportServer`]
//! using Tokio's async TCP stack. It accepts connections in a loop and
//! spawns one task per connection, capped at `max_connections`.
//!
//! # Connection cap
//!
//! When `active_connections >= max_connections`, the server accepts the
//! socket (to consume it from the OS queue) but immediately sends a
//! capacity-error response and closes the connection. This prevents the
//! OS accept queue from filling up while still giving the client a
//! meaningful error.
//!
//! # Shutdown
//!
//! Call [`TcpTransportServer::shutdown`] from any async task. The server
//! sets a flag: the accept loop sees it and exits. Each active connection
//! task finishes its current request then exits. The shutdown waits up to
//! 5 seconds for `active_connections` to reach 0.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::net::TcpListener;
use tracing::{error, info, warn};

use blazil_common::error::{BlazerError, BlazerResult};
use blazil_common::timestamp::Timestamp;
use blazil_engine::pipeline::Pipeline;
use blazil_engine::ring_buffer::RingBuffer;

use crate::connection::handle_connection;
use crate::protocol::{serialize_response, Frame, TransactionResponse};
use crate::server::TransportServer;

/// Default maximum number of simultaneous client connections.
pub const DEFAULT_MAX_CONNECTIONS: u64 = 10_000;

// ── TcpTransportServer ────────────────────────────────────────────────────────

/// A TCP-based Blazil transport server.
///
/// Bind to `"127.0.0.1:0"` in tests so the OS assigns a free port; call
/// [`local_addr`][TcpTransportServer::local_addr] after [`serve`][TcpTransportServer::serve]
/// starts to discover the assigned address.
///
/// # Examples
///
/// ```rust,no_run
/// use std::sync::Arc;
/// use blazil_transport::tcp::TcpTransportServer;
/// use blazil_transport::server::TransportServer;
/// use blazil_engine::pipeline::Pipeline;
/// use blazil_engine::ring_buffer::RingBuffer;
///
/// # async fn example(pipeline: Arc<Pipeline>, ring_buffer: Arc<RingBuffer>) {
/// let server = Arc::new(TcpTransportServer::new(
///     "127.0.0.1:0",
///     pipeline,
///     ring_buffer,
///     100,
/// ));
/// let s = Arc::clone(&server);
/// tokio::spawn(async move { s.serve().await });
/// server.shutdown().await;
/// # }
/// ```
pub struct TcpTransportServer {
    addr: String,
    pipeline: Arc<Pipeline>,
    ring_buffer: Arc<RingBuffer>,
    shutdown: Arc<AtomicBool>,
    active_connections: Arc<AtomicU64>,
    max_connections: u64,
    /// The OS-assigned address string, populated once serve() binds.
    bound_addr: tokio::sync::RwLock<String>,
}

impl TcpTransportServer {
    /// Creates a new `TcpTransportServer`.
    ///
    /// The server does **not** bind until [`serve`][TcpTransportServer::serve] is called.
    ///
    /// # Arguments
    ///
    /// - `addr` — bind address (e.g. `"127.0.0.1:0"` or `"0.0.0.0:7878"`).
    /// - `pipeline` — shared engine pipeline.
    /// - `ring_buffer` — shared ring buffer for result polling.
    /// - `max_connections` — reject connections above this threshold.
    pub fn new(
        addr: &str,
        pipeline: Arc<Pipeline>,
        ring_buffer: Arc<RingBuffer>,
        max_connections: u64,
    ) -> Self {
        Self {
            addr: addr.to_owned(),
            pipeline,
            ring_buffer,
            shutdown: Arc::new(AtomicBool::new(false)),
            active_connections: Arc::new(AtomicU64::new(0)),
            max_connections,
            bound_addr: tokio::sync::RwLock::new(addr.to_owned()),
        }
    }

    /// Returns the current count of active connections.
    pub fn active_connections(&self) -> u64 {
        self.active_connections.load(Ordering::Acquire)
    }
}

#[async_trait]
impl TransportServer for TcpTransportServer {
    async fn serve(&self) -> BlazerResult<()> {
        let listener = TcpListener::bind(&self.addr)
            .await
            .map_err(|e| BlazerError::Transport(format!("bind failed on {}: {e}", self.addr)))?;

        // Record the OS-assigned address (important when port was 0).
        let actual_addr = listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| self.addr.clone());

        *self.bound_addr.write().await = actual_addr.clone();

        info!(addr = %actual_addr, "Blazil transport listening");

        loop {
            // ── Shutdown check ─────────────────────────────────────────────
            if self.shutdown.load(Ordering::Acquire) {
                break;
            }

            // ── Accept next connection (with shutdown interruptibility) ────
            let accept_result = tokio::select! {
                res = listener.accept() => res,
                _ = shutdown_signal(Arc::clone(&self.shutdown)) => break,
            };

            let (stream, peer_addr) = match accept_result {
                Ok(pair) => pair,
                Err(e) => {
                    error!(error = %e, "accept() failed");
                    continue;
                }
            };

            // ── Connection cap ────────────────────────────────────────────
            let current = self.active_connections.load(Ordering::Acquire);
            if current >= self.max_connections {
                warn!(
                    peer = %peer_addr,
                    active = current,
                    max = self.max_connections,
                    "server at capacity — rejecting connection"
                );
                // Consume the socket and send a capacity error, then close.
                let resp = TransactionResponse {
                    request_id: String::new(),
                    committed: false,
                    transfer_id: None,
                    error: Some("server at capacity".into()),
                    timestamp_ns: Timestamp::now().as_nanos(),
                };
                let mut stream = stream;
                if let Ok(bytes) = serialize_response(&resp) {
                    let _ = Frame::write_frame(&mut stream, &bytes).await;
                }
                continue;
            }

            // ── Spawn connection task ─────────────────────────────────────
            self.active_connections.fetch_add(1, Ordering::Release);
            let pipeline = Arc::clone(&self.pipeline);
            let ring_buffer = Arc::clone(&self.ring_buffer);
            let active = Arc::clone(&self.active_connections);

            tokio::spawn(async move {
                if let Err(e) = handle_connection(stream, pipeline, ring_buffer, active).await {
                    warn!(peer = %peer_addr, error = %e, "connection handler error");
                }
            });
        }

        info!("Transport accept loop exited");
        Ok(())
    }

    async fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);

        // Wait up to 5 seconds for all connections to drain.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while self.active_connections.load(Ordering::Acquire) > 0 {
            if std::time::Instant::now() >= deadline {
                warn!(
                    remaining = self.active_connections.load(Ordering::Acquire),
                    "shutdown timeout — some connections may not have drained"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        info!("Transport server shut down cleanly");
    }

    fn local_addr(&self) -> &str {
        // This is safe only after `serve()` has had a chance to update `bound_addr`.
        // For test use, callers should wait briefly or use `local_addr_async`.
        // We return the configured addr as a safe fallback.
        &self.addr
    }
}

impl TcpTransportServer {
    /// Returns the actual bound address, including the OS-assigned port.
    ///
    /// Must be called after `serve()` has started and bound the listener.
    pub async fn local_addr_async(&self) -> String {
        self.bound_addr.read().await.clone()
    }
}

// ── Shutdown signal ───────────────────────────────────────────────────────────

/// Resolves once the shutdown flag is set.
///
/// Used in `select!` inside the accept loop so the loop is not blocked
/// waiting for a connection when shutdown is requested.
async fn shutdown_signal(shutdown: Arc<AtomicBool>) {
    loop {
        if shutdown.load(Ordering::Acquire) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
