//! Integration test: Aeron IPC round-trip.
//!
//! Verifies that the embedded C Media Driver + safe Rust wrappers can
//! publish and receive messages end-to-end using `aeron:ipc`.
//!
//! # Running
//!
//! Requires the C library (`git submodule update --init --recursive` and a
//! cmake / g++ toolchain):
//!
//! ```bash
//! cargo test --features aeron -p blazil-transport -- --ignored --nocapture
//! ```
//!
//! These tests are `#[ignore]`d by default so that `cargo test --workspace`
//! (no features, no submodule) continues to pass in CI.

#[cfg(feature = "aeron")]
mod aeron_ipc_tests {
    use std::time::Duration;

    use blazil_transport::aeron::{
        AeronContext, AeronPublication, AeronSubscription, EmbeddedAeronDriver,
    };

    const TEST_AERON_DIR: &str = "/tmp/aeron-blazil-test";
    const TEST_CHANNEL: &str = "aeron:ipc";
    const REG_TIMEOUT: Duration = Duration::from_secs(5);

    /// Start a new embedded driver for each test, in a clean directory.
    fn start_driver() -> EmbeddedAeronDriver {
        let driver = EmbeddedAeronDriver::new(Some(TEST_AERON_DIR));
        driver.start().expect("EmbeddedAeronDriver::start");
        driver
    }

    // ── Test 1: single message round-trip ─────────────────────────────────────

    /// Send one message and receive it back on the same IPC endpoint.
    #[test]
    #[ignore = "requires aeron feature + C library (git submodule)"]
    fn test_ipc_single_message() {
        let driver = start_driver();

        let ctx = AeronContext::new(TEST_AERON_DIR).expect("AeronContext::new");

        let pub_ = AeronPublication::new(&ctx, TEST_CHANNEL, 1001, REG_TIMEOUT)
            .expect("AeronPublication::new");

        let sub = AeronSubscription::new(&ctx, TEST_CHANNEL, 1001, REG_TIMEOUT)
            .expect("AeronSubscription::new");

        // Wait for the publisher image to appear on the subscription.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !sub.is_connected() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(sub.is_connected(), "subscription should see a connected publisher");

        let payload = b"hello-aeron-ipc";
        pub_.offer(payload).expect("offer should succeed");

        let mut received: Vec<Vec<u8>> = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while received.is_empty() && std::time::Instant::now() < deadline {
            sub.poll_fragments(&mut received, 10);
            if received.is_empty() {
                std::hint::spin_loop();
            }
        }

        assert_eq!(received.len(), 1, "expected exactly one fragment");
        assert_eq!(received[0], payload, "payload mismatch");

        // Explicit ordered drop: pub, sub, ctx, driver.
        drop(pub_);
        drop(sub);
        drop(ctx);
        drop(driver);
    }

    // ── Test 2: 1 000 message throughput ─────────────────────────────────────

    /// Publish 1 000 messages and confirm all are received in order.
    #[test]
    #[ignore = "requires aeron feature + C library (git submodule)"]
    fn test_ipc_1000_messages() {
        let driver = start_driver();
        let ctx = AeronContext::new(TEST_AERON_DIR).expect("AeronContext::new");

        let pub_ = AeronPublication::new(&ctx, TEST_CHANNEL, 2001, REG_TIMEOUT)
            .expect("AeronPublication");
        let sub = AeronSubscription::new(&ctx, TEST_CHANNEL, 2001, REG_TIMEOUT)
            .expect("AeronSubscription");

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !sub.is_connected() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(sub.is_connected());

        const N: u64 = 1_000;

        for i in 0..N {
            let payload = i.to_le_bytes();
            pub_.offer(&payload).expect("offer");
        }

        let mut received: Vec<Vec<u8>> = Vec::with_capacity(N as usize);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while (received.len() as u64) < N && std::time::Instant::now() < deadline {
            sub.poll_fragments(&mut received, 100);
            std::hint::spin_loop();
        }

        assert_eq!(received.len() as u64, N, "should receive all {N} messages");

        // Verify each message (order is preserved with IPC).
        for (i, msg) in received.iter().enumerate() {
            let expected = (i as u64).to_le_bytes();
            assert_eq!(msg.as_slice(), expected, "message {i} content mismatch");
        }

        drop(pub_);
        drop(sub);
        drop(ctx);
        drop(driver);
    }

    // ── Test 3: public API round-trip through AeronTransportServer ───────────

    /// Smoke-test `AeronTransportServer::serve` by spinning it up, sending
    /// 10 MessagePack requests, and asserting committed responses return.
    ///
    /// This test exercises the full E2E path:
    /// client pub → server sub → Pipeline → server pub → client sub
    #[tokio::test]
    #[ignore = "requires aeron feature + C library (git submodule)"]
    async fn test_transport_server_round_trip() {
        use std::sync::Arc;

        use blazil_common::ids::{AccountId, LedgerId};
        use blazil_common::currency::parse_currency;
        use blazil_engine::handlers::ledger::LedgerHandler;
        use blazil_engine::handlers::publish::PublishHandler;
        use blazil_engine::handlers::risk::RiskHandler;
        use blazil_engine::handlers::validation::ValidationHandler;
        use blazil_engine::pipeline::PipelineBuilder;
        use blazil_ledger::account::{Account, AccountFlags};
        use blazil_ledger::client::LedgerClient;
        use blazil_ledger::mock::InMemoryLedgerClient;
        use blazil_transport::aeron_transport::{AeronTransportServer, REQ_STREAM_ID, RSP_STREAM_ID};
        use blazil_transport::protocol::{serialize_request, deserialize_response, TransactionRequest};
        use blazil_transport::server::TransportServer;

        const TEST_DIR: &str = "/tmp/aeron-blazil-e2e-test";

        // ── pipeline ──────────────────────────────────────────────────────────
        let ledger_client = Arc::new(InMemoryLedgerClient::new_unbounded());
        let usd = parse_currency("USD").expect("USD");
        let ledger_rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("ledger rt"),
        );

        let debit_id = ledger_client
            .create_account(Account::new(
                AccountId::new(),
                LedgerId::USD,
                usd,
                1,
                AccountFlags::default(),
            ))
            .await
            .expect("debit");

        let credit_id = ledger_client
            .create_account(Account::new(
                AccountId::new(),
                LedgerId::USD,
                usd,
                1,
                AccountFlags::default(),
            ))
            .await
            .expect("credit");

        let builder = PipelineBuilder::new().with_capacity(1024);
        let results = builder.results();
        let (pipeline, runners) = builder
            .add_handler(ValidationHandler::new(Arc::clone(&results)))
            .add_handler(RiskHandler::new(100_000_000_000, Arc::clone(&results)))
            .add_handler(LedgerHandler::new(
                ledger_client,
                ledger_rt,
                Arc::clone(&results),
            ))
            .add_handler(PublishHandler::new(Arc::clone(&results)))
            .build()
            .expect("pipeline");

        let pipeline = Arc::new(pipeline);
        let _run_handles: Vec<_> = runners.into_iter().map(|r| r.run()).collect();

        // ── server ─────────────────────────────────────────────────────────────
        let channel = "aeron:udp?endpoint=127.0.0.1:41234".to_string();
        let server = Arc::new(AeronTransportServer::new(
            &channel,
            TEST_DIR,
            Arc::clone(&pipeline),
        ));
        let server_handle = {
            let s = Arc::clone(&server);
            tokio::task::spawn(async move { s.serve().await })
        };

        // Give server time to start the embedded driver and register streams.
        tokio::time::sleep(Duration::from_millis(500)).await;

        // ── client ─────────────────────────────────────────────────────────────
        // The client must connect to the SAME embedded driver IPC dir.
        // Use blocking code on a dedicated thread to avoid blocking the async executor.
        let (tx, rx) = std::sync::mpsc::channel::<Vec<Vec<u8>>>();
        let channel_c = channel.clone();

        tokio::task::spawn_blocking(move || {
            let ctx = AeronContext::new(TEST_DIR).expect("client ctx");

            // Client publishes requests on stream 1001 (server's receive stream).
            let client_pub = AeronPublication::new(&ctx, &channel_c, REQ_STREAM_ID, REG_TIMEOUT)
                .expect("client pub");

            // Client subscribes to stream 1002 (server's response stream).
            let client_sub = AeronSubscription::new(&ctx, &channel_c, RSP_STREAM_ID, REG_TIMEOUT)
                .expect("client sub");

            let conn_deadline = std::time::Instant::now() + Duration::from_secs(3);
            while !client_pub.is_connected() && std::time::Instant::now() < conn_deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(client_pub.is_connected(), "client pub should see server sub");

            // Send 10 requests.
            for i in 0u16..10 {
                let req = TransactionRequest {
                    request_id: format!("req-{i:04}"),
                    debit_account_id: debit_id.to_string(),
                    credit_account_id: credit_id.to_string(),
                    amount: "1.00".to_owned(),
                    currency: "USD".to_owned(),
                    ledger_id: 0,
                    code: i + 1,
                };
                let bytes = serialize_request(&req).expect("serialize");
                client_pub.offer(&bytes).expect("offer");
            }

            // Collect 10 responses.
            let mut responses: Vec<Vec<u8>> = Vec::new();
            let recv_deadline = std::time::Instant::now() + Duration::from_secs(5);
            while responses.len() < 10 && std::time::Instant::now() < recv_deadline {
                client_sub.poll_fragments(&mut responses, 10);
                std::hint::spin_loop();
            }

            drop(client_pub);
            drop(client_sub);
            drop(ctx);

            tx.send(responses).expect("send responses");
        });

        let responses = rx.recv_timeout(Duration::from_secs(10)).expect("recv_timeout");
        assert_eq!(responses.len(), 10, "expected 10 responses");

        for bytes in &responses {
            let resp = deserialize_response(bytes).expect("deserialize response");
            assert!(resp.committed, "response not committed: {:?}", resp.error);
        }

        server.shutdown().await;
        let _ = server_handle.await;
    }
}
