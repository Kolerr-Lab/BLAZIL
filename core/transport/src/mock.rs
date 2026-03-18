//! Mock transport client for integration tests.
//!
//! [`MockTransportClient`] opens real TCP connections to a running
//! [`crate::tcp::TcpTransportServer`] and sends [`TransactionRequest`]s
//! using the same wire protocol as production clients.
//!
//! Using a real TCP client in tests validates the full pipeline end-to-end
//! without any mocking of the network layer itself.
//!
//! # Examples
//!
//! ```rust,no_run
//! use blazil_transport::mock::MockTransportClient;
//! use blazil_transport::protocol::TransactionRequest;
//!
//! # async fn example() {
//! let client = MockTransportClient::new("127.0.0.1:7878");
//! let request = TransactionRequest {
//!     request_id: "test-001".into(),
//!     debit_account_id: "550e8400-e29b-41d4-a716-446655440001".into(),
//!     credit_account_id: "550e8400-e29b-41d4-a716-446655440002".into(),
//!     amount: "10.00".into(),
//!     currency: "USD".into(),
//!     ledger_id: 1,
//!     code: 1,
//! };
//! let resp = client.send_transaction(request).await.unwrap();
//! assert!(resp.committed);
//! # }
//! ```

use tokio::net::TcpStream;

use blazil_common::error::{BlazerError, BlazerResult};

use crate::protocol::{
    deserialize_response, serialize_request, Frame, TransactionRequest, TransactionResponse,
};

// ── MockTransportClient ───────────────────────────────────────────────────────

/// A simple TCP client used in integration tests.
///
/// Opens a fresh TCP connection for each [`send_transaction`][MockTransportClient::send_transaction]
/// call, or reuses one connection for a [`send_batch`][MockTransportClient::send_batch].
pub struct MockTransportClient {
    addr: String,
}

impl MockTransportClient {
    /// Creates a new client targeting `addr` (e.g. `"127.0.0.1:7878"`).
    ///
    /// No connection is opened at construction time.
    pub fn new(addr: &str) -> Self {
        Self {
            addr: addr.to_owned(),
        }
    }

    /// Sends a single transaction request and waits for the response.
    ///
    /// Opens a fresh TCP connection, sends the framed request, reads the
    /// framed response, and closes the connection.
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::Transport`] if the connection or frame
    /// exchange fails.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use blazil_transport::mock::MockTransportClient;
    /// use blazil_transport::protocol::TransactionRequest;
    ///
    /// # async fn example() {
    /// let client = MockTransportClient::new("127.0.0.1:7878");
    /// let req = TransactionRequest {
    ///     request_id: "req-001".into(),
    ///     debit_account_id: "00000000-0000-0000-0000-000000000001".into(),
    ///     credit_account_id: "00000000-0000-0000-0000-000000000002".into(),
    ///     amount: "50.00".into(),
    ///     currency: "USD".into(),
    ///     ledger_id: 1,
    ///     code: 1,
    /// };
    /// let resp = client.send_transaction(req).await.unwrap();
    /// # }
    /// ```
    pub async fn send_transaction(
        &self,
        request: TransactionRequest,
    ) -> BlazerResult<TransactionResponse> {
        let mut stream = TcpStream::connect(&self.addr)
            .await
            .map_err(|e| BlazerError::Transport(format!("connect to {} failed: {e}", self.addr)))?;

        let payload = serialize_request(&request)?;
        Frame::write_frame(&mut stream, &payload).await?;

        let frame = Frame::read_frame(&mut stream).await?;
        deserialize_response(&frame.payload)
    }

    /// Sends multiple requests over a **single** connection and collects all responses.
    ///
    /// The requests are sent in order; responses are read in order. Using
    /// one connection amortises the TCP handshake cost across the batch.
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::Transport`] if the connection or any frame
    /// exchange fails.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use blazil_transport::mock::MockTransportClient;
    /// use blazil_transport::protocol::TransactionRequest;
    ///
    /// # async fn example(reqs: Vec<TransactionRequest>) {
    /// let client = MockTransportClient::new("127.0.0.1:7878");
    /// let responses = client.send_batch(reqs).await.unwrap();
    /// assert!(responses.iter().all(|r| r.committed));
    /// # }
    /// ```
    pub async fn send_batch(
        &self,
        requests: Vec<TransactionRequest>,
    ) -> BlazerResult<Vec<TransactionResponse>> {
        let mut stream = TcpStream::connect(&self.addr)
            .await
            .map_err(|e| BlazerError::Transport(format!("connect to {} failed: {e}", self.addr)))?;

        let mut responses = Vec::with_capacity(requests.len());

        for request in requests {
            let payload = serialize_request(&request)?;
            Frame::write_frame(&mut stream, &payload).await?;

            let frame = Frame::read_frame(&mut stream).await?;
            let resp = deserialize_response(&frame.payload)?;
            responses.push(resp);
        }

        Ok(responses)
    }
}

// ── Integration tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use blazil_common::currency::parse_currency;
    use blazil_common::ids::{AccountId, LedgerId, TransactionId};
    use blazil_engine::handlers::ledger::LedgerHandler;
    use blazil_engine::handlers::publish::PublishHandler;
    use blazil_engine::handlers::risk::RiskHandler;
    use blazil_engine::handlers::validation::ValidationHandler;
    use blazil_engine::pipeline::PipelineBuilder;
    use blazil_ledger::account::{Account, AccountFlags};
    use blazil_ledger::client::LedgerClient;
    use blazil_ledger::mock::InMemoryLedgerClient;
    use tokio::net::TcpStream;

    use super::*;
    use crate::protocol::TransactionRequest;
    use crate::server::TransportServer;
    use crate::tcp::TcpTransportServer;

    // ── Test helpers ──────────────────────────────────────────────────────────

    /// Builds a full pipeline with InMemoryLedgerClient and returns
    /// (server, debit_id, credit_id) — the two pre-seeded accounts.
    async fn build_test_server(
        max_connections: u64,
    ) -> (
        Arc<TcpTransportServer>,
        String, // debit_account_id (UUID string)
        String, // credit_account_id (UUID string)
    ) {
        // We need a Runtime to pass to LedgerHandler (which calls block_on from
        // a sync pipeline thread). We do NOT call block_on here — we're already
        // inside a #[tokio::test] runtime, so we just use .await directly.
        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("handler runtime"),
        );
        let client = Arc::new(InMemoryLedgerClient::new());
        let usd = parse_currency("USD").expect("USD");

        let debit_id = {
            let acc = Account::new(
                AccountId::new(),
                LedgerId::USD,
                usd,
                1,
                AccountFlags::default(),
            );
            client.create_account(acc).await.expect("debit account")
        };

        let credit_id = {
            let usd2 = parse_currency("USD").expect("USD");
            let acc = Account::new(
                AccountId::new(),
                LedgerId::USD,
                usd2,
                1,
                AccountFlags::default(),
            );
            client.create_account(acc).await.expect("credit account")
        };

        let max_amount_units: u64 = 10_000_000_000_u64; // $1M in cents

        let builder = PipelineBuilder::new().with_capacity(1024);
        let results = builder.results();
        let (pipeline, runners) = builder
            .add_handler(ValidationHandler::new(Arc::clone(&results)))
            .add_handler(RiskHandler::new(max_amount_units, Arc::clone(&results)))
            .add_handler(LedgerHandler::new(
                Arc::clone(&client),
                Arc::clone(&rt),
                Arc::clone(&results),
            ))
            .add_handler(PublishHandler::new(Arc::clone(&results)))
            .build()
            .expect("pipeline");

        let _handles: Vec<_> = runners.into_iter().map(|r| r.run()).collect();

        let pipeline = Arc::new(pipeline);

        let server = Arc::new(TcpTransportServer::new(
            "127.0.0.1:0",
            pipeline,
            Arc::clone(&results),
            max_connections,
        ));

        (server, debit_id.to_string(), credit_id.to_string())
    }

    /// Starts the server on a background task and returns its address.
    async fn start_server(server: &Arc<TcpTransportServer>) -> String {
        let s = Arc::clone(server);
        tokio::spawn(async move {
            if let Err(e) = s.serve().await {
                eprintln!("serve error: {e}");
            }
        });

        // Give the server a moment to bind and record its address.
        tokio::time::sleep(Duration::from_millis(20)).await;
        server.local_addr_async().await
    }

    fn make_request(debit_id: &str, credit_id: &str, amount: &str) -> TransactionRequest {
        TransactionRequest {
            request_id: TransactionId::new().to_string(),
            debit_account_id: debit_id.to_owned(),
            credit_account_id: credit_id.to_owned(),
            amount: amount.to_owned(),
            currency: "USD".into(),
            ledger_id: 1,
            code: 1,
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn single_valid_transaction_is_committed() {
        let (server, debit_id, credit_id) = build_test_server(100).await;
        let addr = start_server(&server).await;

        let client = MockTransportClient::new(&addr);
        let req = make_request(&debit_id, &credit_id, "10.00");
        let resp = client
            .send_transaction(req)
            .await
            .expect("send_transaction");

        assert!(
            resp.committed,
            "expected committed=true, got: {:?}",
            resp.error
        );
        assert!(resp.transfer_id.is_some());
        assert!(resp.error.is_none());

        server.shutdown().await;
    }

    #[tokio::test]
    async fn invalid_account_id_returns_rejection() {
        let (server, _, credit_id) = build_test_server(100).await;
        let addr = start_server(&server).await;

        let client = MockTransportClient::new(&addr);
        let req = TransactionRequest {
            request_id: TransactionId::new().to_string(),
            debit_account_id: "not-a-valid-uuid".into(), // invalid
            credit_account_id: credit_id,
            amount: "10.00".into(),
            currency: "USD".into(),
            ledger_id: 1,
            code: 1,
        };
        let resp = client
            .send_transaction(req)
            .await
            .expect("send_transaction");

        assert!(!resp.committed);
        assert!(resp.error.is_some());

        server.shutdown().await;
    }

    #[tokio::test]
    async fn zero_amount_is_rejected_at_validation() {
        let (server, debit_id, credit_id) = build_test_server(100).await;
        let addr = start_server(&server).await;

        let client = MockTransportClient::new(&addr);
        let req = make_request(&debit_id, &credit_id, "0.00"); // zero amount
        let resp = client
            .send_transaction(req)
            .await
            .expect("send_transaction");

        assert!(!resp.committed);
        assert!(resp.error.is_some());

        server.shutdown().await;
    }

    #[tokio::test]
    async fn batch_of_10_all_committed() {
        let (server, debit_id, credit_id) = build_test_server(100).await;
        let addr = start_server(&server).await;

        let client = MockTransportClient::new(&addr);
        let requests: Vec<_> = (0..10)
            .map(|_| make_request(&debit_id, &credit_id, "1.00"))
            .collect();

        let responses = client.send_batch(requests).await.expect("send_batch");
        assert_eq!(responses.len(), 10);

        for (i, resp) in responses.iter().enumerate() {
            assert!(
                resp.committed,
                "request {i} not committed: {:?}",
                resp.error
            );
        }

        server.shutdown().await;
    }

    #[tokio::test]
    async fn server_at_max_connections_rejects_second_client() {
        // max_connections = 1: after the first connection is accepted, the
        // second should receive a capacity-error response.
        let (server, debit_id, credit_id) = build_test_server(1).await;
        let addr = start_server(&server).await;

        // Hold a persistent connection open so active_connections stays at 1.
        let first_stream = TcpStream::connect(&addr).await.expect("first connect");
        // Give the server a moment to accept and increment the counter.
        tokio::time::sleep(Duration::from_millis(30)).await;

        // The second client should get a capacity-error response immediately.
        let second_client = MockTransportClient::new(&addr);
        let req = make_request(&debit_id, &credit_id, "5.00");
        let resp = second_client
            .send_transaction(req)
            .await
            .expect("second send");

        assert!(!resp.committed);
        assert!(
            resp.error
                .as_deref()
                .map(|e| e.contains("capacity"))
                .unwrap_or(false),
            "expected capacity error, got: {:?}",
            resp.error
        );

        drop(first_stream);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn client_disconnect_mid_session_does_not_panic() {
        let (server, debit_id, credit_id) = build_test_server(100).await;
        let addr = start_server(&server).await;

        // Connect and immediately drop the stream (simulates abrupt disconnect).
        {
            let _stream = TcpStream::connect(&addr).await.expect("connect");
            // Stream dropped here — server should handle gracefully.
        }

        // Give the server time to notice the disconnect.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Server should still be functional.
        let client = MockTransportClient::new(&addr);
        let req = make_request(&debit_id, &credit_id, "1.00");
        let resp = client
            .send_transaction(req)
            .await
            .expect("send after disconnect");
        assert!(
            resp.committed,
            "server should still work after a disconnect: {:?}",
            resp.error
        );

        server.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_causes_serve_to_return() {
        let (server, _debit, _credit) = build_test_server(100).await;
        let s = Arc::clone(&server);

        let serve_handle = tokio::spawn(async move {
            s.serve().await.expect("serve failed");
        });

        // Give it a moment to bind.
        tokio::time::sleep(Duration::from_millis(20)).await;

        server.shutdown().await;

        // serve() should return within a reasonable timeout.
        tokio::time::timeout(Duration::from_secs(2), serve_handle)
            .await
            .expect("serve did not return after shutdown")
            .expect("serve task panicked");
    }
}
