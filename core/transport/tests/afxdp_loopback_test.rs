//! AF_XDP transport integration and unit tests.
//!
//! # Unit tests (run on all platforms, no special hardware required)
//!
//! - `test_blzl_frame_encoding`: verifies BLZL magic + msgpack frame structure.
//! - `test_blzl_frame_loopback`: AfXdpClient roundtrip over localhost UDP
//!   (simulated server — no XDP driver needed).
//!
//! # Integration tests (Linux only, kernel + hardware required)
//!
//! - `test_afxdp_roundtrip_loopback`: full AF_XDP roundtrip via a veth pair.
//!   Requires `CAP_NET_ADMIN`, a `veth0`/`veth1` pair, and the compiled BPF
//!   object (`OUT_DIR/blazil_xdp.bpf.o`).
//!   Skipped by default (`#[ignore]`); run explicitly with:
//!   ```bash
//!   sudo -E cargo test --features af-xdp -p blazil-transport \
//!       -- test_afxdp_roundtrip_loopback --nocapture --ignored
//!   ```

// ── Cross-platform unit tests ─────────────────────────────────────────────────

#[cfg(feature = "af-xdp")]
mod blzl_unit {
    use blazil_transport::protocol::{
        deserialize_request, encode_blzl_frame, TransactionRequest, BLZL_MAGIC, BLZL_UDP_PORT,
    };

    fn sample_req() -> TransactionRequest {
        TransactionRequest {
            request_id: "test-blzl-001".into(),
            debit_account_id: "550e8400-e29b-41d4-a716-446655440001".into(),
            credit_account_id: "550e8400-e29b-41d4-a716-446655440002".into(),
            amount: "42.00".into(),
            currency: "USD".into(),
            ledger_id: 1,
            code: 1,
            flags: 0,
            pending_transfer_id: "".into(),
        }
    }

    #[test]
    fn test_blzl_frame_encoding_magic() {
        let frame = encode_blzl_frame(&sample_req()).unwrap();
        assert_eq!(&frame[..4], &BLZL_MAGIC, "frame must start with BLZL magic");
    }

    #[test]
    fn test_blzl_frame_encoding_payload_roundtrip() {
        let req = sample_req();
        let frame = encode_blzl_frame(&req).unwrap();
        let payload = &frame[BLZL_MAGIC.len()..];
        let decoded = deserialize_request(payload).unwrap();
        assert_eq!(decoded.request_id, req.request_id);
        assert_eq!(decoded.amount, req.amount);
        assert_eq!(decoded.currency, req.currency);
        assert_eq!(decoded.ledger_id, req.ledger_id);
        assert_eq!(decoded.code, req.code);
    }

    #[test]
    fn test_blzl_magic_value() {
        // ASCII "BLZL" big-endian, matches ebpf/blazil_xdp.bpf.c BLAZIL_MAGIC constant.
        assert_eq!(BLZL_MAGIC, [0x42u8, 0x4C, 0x5A, 0x4C]);
        assert_eq!(BLZL_UDP_PORT, 7878);
    }

    #[test]
    fn test_blzl_frame_minimum_size() {
        let frame = encode_blzl_frame(&sample_req()).unwrap();
        // Must have at least magic + 1 byte msgpack payload.
        assert!(
            frame.len() > BLZL_MAGIC.len(),
            "frame too short: {} bytes",
            frame.len()
        );
    }
}

// ── Linux-only client loopback test (AfXdpClient re-exported only on Linux) ──

#[cfg(all(target_os = "linux", feature = "af-xdp"))]
mod client_loopback {
    use std::net::UdpSocket;
    use std::time::Duration;

    use blazil_transport::afxdp::client::AfXdpClient;
    use blazil_transport::protocol::{
        deserialize_request, encode_blzl_frame, serialize_response, TransactionRequest,
        TransactionResponse, BLZL_MAGIC,
    };

    /// Verifies the full client send → mock server receive → response flow
    /// using two localhost UDP sockets.  No AF_XDP kernel driver required.
    #[test]
    fn test_blzl_frame_loopback() {
        // ── Simulated server ─────────────────────────────────────────────────
        let server_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        server_sock
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let server_addr = server_sock.local_addr().unwrap();

        let req = TransactionRequest {
            request_id: "loopback-001".into(),
            debit_account_id: "debit-001".into(),
            credit_account_id: "credit-001".into(),
            amount: "10.00".into(),
            currency: "USD".into(),
            ledger_id: 1,
            code: 1,
            flags: 0,
            pending_transfer_id: "".into(),
        };
        let req_clone = req.clone();

        let server_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 65_535];
            let (n, src) = server_sock.recv_from(&mut buf).unwrap();

            // Verify BLZL magic.
            assert_eq!(
                &buf[..4],
                &BLZL_MAGIC,
                "server: expected BLZL magic at offset 0"
            );

            // Decode request payload.
            let decoded = deserialize_request(&buf[4..n]).unwrap();
            assert_eq!(decoded.request_id, req_clone.request_id);

            // Send response.
            let resp = TransactionResponse {
                request_id: decoded.request_id,
                committed: true,
                transfer_id: Some("t-loopback-001".into()),
                error: None,
                timestamp_ns: 1_000_000,
            };
            let resp_bytes = serialize_response(&resp).unwrap();
            server_sock.send_to(&resp_bytes, src).unwrap();
        });

        // ── Client ──────────────────────────────────────────────────────────
        let client = AfXdpClient::connect(server_addr).unwrap();
        let (resp, rtt) = client
            .roundtrip(&req, Duration::from_secs(3))
            .expect("loopback roundtrip must succeed");

        assert!(resp.committed, "response must be committed");
        assert_eq!(resp.request_id, req.request_id);
        assert!(rtt < Duration::from_secs(1), "RTT must be < 1s on loopback");

        server_thread.join().unwrap();
    }
}

// ── Linux-only AF_XDP hardware integration test ───────────────────────────────

#[cfg(all(target_os = "linux", feature = "af-xdp"))]
mod afxdp_linux {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::time::Duration;

    use blazil_transport::afxdp::client::AfXdpClient;
    use blazil_transport::protocol::TransactionRequest;

    /// Full AF_XDP roundtrip over a veth loopback pair.
    ///
    /// # Prerequisites
    ///
    /// 1. `CAP_NET_ADMIN` (run with `sudo -E`).
    /// 2. A veth pair set up:
    ///    ```bash
    ///    ip link add veth0 type veth peer name veth1
    ///    ip link set veth0 up
    ///    ip link set veth1 up
    ///    ip addr add 169.254.100.1/24 dev veth0
    ///    ip addr add 169.254.100.2/24 dev veth1
    ///    ```
    /// 3. `ulimit -l unlimited`
    ///
    /// # Running
    ///
    /// ```bash
    /// sudo -E cargo test --features af-xdp -p blazil-transport \
    ///     -- test_afxdp_roundtrip_loopback --nocapture --ignored
    /// ```
    #[test]
    #[ignore = "requires CAP_NET_ADMIN, veth pair (veth0/veth1), and ulimit -l unlimited"]
    fn test_afxdp_roundtrip_loopback() {
        use std::sync::Arc;

        use blazil_engine::pipeline::PipelineBuilder;
        use blazil_engine::result_ring::ResultRing;
        use blazil_transport::afxdp::AfXdpConfig;
        use blazil_transport::server::TransportServer;
        use blazil_transport::AfXdpTransportServer;
        use dashmap::DashMap;

        // ── Build minimal pipeline (no TB ledger needed for this test) ────────
        let results = Arc::new(DashMap::new());
        let result_ring = Arc::new(ResultRing::new(1024));
        let pipeline = blazil_engine::pipeline::PipelineBuilder::new(1024)
            .with_result_ring(Arc::clone(&result_ring))
            .build();
        let pipeline = Arc::new(pipeline);

        // ── Start server on veth0 (queue 0 only) ──────────────────────────────
        let cfg = AfXdpConfig {
            if_name: "veth0".into(),
            queue_ids: vec![0],
            port: 7878,
            zero_copy: false, // veth does not support XDP_ZEROCOPY
        };

        let server =
            AfXdpTransportServer::new(cfg, vec![Arc::clone(&pipeline)], Arc::clone(&results));

        let stop = Arc::new(AtomicBool::new(false));
        let stop_server = Arc::clone(&stop);
        let server_thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let _ = server.serve().await;
            });
        });

        // Give the server time to attach the XDP program and open sockets.
        std::thread::sleep(Duration::from_millis(500));

        // ── Connect client to server address ──────────────────────────────────
        let client =
            AfXdpClient::connect("169.254.100.1:7878".parse().unwrap()).expect("client connect");

        let req = TransactionRequest {
            request_id: "veth-loopback-001".into(),
            debit_account_id: "550e8400-e29b-41d4-a716-446655440001".into(),
            credit_account_id: "550e8400-e29b-41d4-a716-446655440002".into(),
            amount: "1.00".into(),
            currency: "USD".into(),
            ledger_id: 1,
            code: 1,
            flags: 0,
            pending_transfer_id: "".into(),
        };

        let (resp, rtt) = client
            .roundtrip(&req, Duration::from_millis(500))
            .expect("veth AF_XDP roundtrip");

        println!("AF_XDP roundtrip RTT: {:?}", rtt);
        assert!(
            resp.committed || resp.error.is_some(),
            "response must be committed or have an error: {resp:?}"
        );

        // Shutdown server.
        stop.store(true, Ordering::Relaxed);
        server_thread.join().unwrap();
    }
}
