# ADR 0005 — Transport Protocol Selection

**Status:** Accepted  
**Date:** 2026-07-03  
**Deciders:** Architecture Room  

---

## Context

Blazil processes financial transactions on multiple data paths that have different latency, throughput, and operability requirements:

| Path | Latency target | Throughput target | Counterparty |
|------|---------------|-------------------|--------------|
| External API | < 50 ms p99 | 50k req/s | Mobile / web clients, partners |
| Engine hot path | < 1 ms p99 | 1M TPS | Internal — engine ↔ transport |
| Cross-shard 2PC | < 5 ms p99 | 100k TPS | Internal — payments ↔ engine |
| ML inference | < 20 ms p99 | 10k req/s | Internal — inference ↔ engine |
| Benchmark harness | best-effort | max throughput | Internal — dev tooling |

Candidate transports evaluated:

| Transport | Latency | Throughput | Operability | Notes |
|-----------|---------|------------|-------------|-------|
| gRPC (HTTP/2) | ~2–5 ms | ~200k TPS | Excellent | Schema via Protobuf; easy tracing |
| TCP + MessagePack | ~0.1–0.5 ms | ~1M TPS | Moderate | Minimal framing; binary compact |
| Aeron (UDP multicast) | ~50–500 µs | ~10M TPS | Complex | Purpose-built for trading |
| AF_XDP (kernel bypass) | ~5–50 µs | ~20M TPS | Very complex | Requires eBPF; Linux-only |
| QUIC | ~1–3 ms | ~500k TPS | Moderate | Good for lossy networks |

---

## Decision

Blazil uses **three transports simultaneously**, each assigned to the path it fits best.

### 1. gRPC (HTTP/2 + Protobuf) — External API Gateway

- All external clients talk to the gRPC Gateway on port 50050.
- Protobuf schemas provide strong typing, versioning, and code generation for client SDKs.
- HTTP/2 multiplexing allows thousands of concurrent client streams on a single connection.
- Interceptors provide a natural integration point for JWT validation, rate limiting, and OTel traces.
- TLS via cert-manager terminates at the gateway; no cleartext external traffic.

### 2. TCP + MessagePack — Engine Hot Path and Cross-Shard 2PC

- The engine's transaction processor listens on TCP port 50051.
- Framing: 4-byte big-endian length prefix + MessagePack-encoded payload.
- MessagePack chosen over JSON (3–5× smaller, no parsing overhead) and Protobuf (no schema compilation step; simpler for in-process Rust ↔ Go bridging).
- Connection pool (10 pre-dialled connections) eliminates per-request TCP handshake cost.
- All internal services (payments, trading, banking, crypto) use this path for submitting transactions.
- The 2PC coordinator uses the same path with extended `flags` + `pending_transfer_id` fields appended at the end of each request struct, preserving backward compatibility.

### 3. Aeron (UDP multicast) — Ultra-Low-Latency Benchmark Path

- Available as an optional transport for high-frequency trading workloads and benchmarking.
- Aeron IPC media driver provides sub-microsecond delivery within a single host.
- Not used in production API flows — reserved for latency-sensitive algorithmic trading scenarios where the counterparty is a co-located strategy process.
- AF_XDP/eBPF kernel-bypass path is available for benchmark validation; not enabled in production due to operational complexity (requires custom kernel modules, privileged containers, and NUMA-aware memory allocation).

---

## Alternatives considered

### Single transport (gRPC everywhere)

Rejected for the hot path. gRPC adds HTTP/2 framing overhead and per-request header processing. Benchmarks show TCP + MessagePack is 3–5× lower latency at the 99th percentile for sub-millisecond financial transaction workloads.

### QUIC everywhere

Rejected. QUIC's connection migration and loss-recovery features are valuable over the public internet; they add overhead on a low-loss internal cluster network. May revisit when hardware offload for QUIC becomes mainstream.

### Apache Kafka for async paths

Deferred. Kafka provides durable ordered delivery which is valuable for event sourcing. The current synchronous ledger model requires immediate confirmation of debit/credit outcome. A Kafka-based outbox pattern will be considered in a future ADR when the event streaming requirement is formalised.

---

## Consequences

### Positive

- Each path operates at its theoretical maximum: gRPC delivers developer ergonomics externally; TCP + MessagePack delivers sub-millisecond throughput internally.
- MessagePack's array format (position-based serialisation via `rmp_serde`) is trivially supported in both Rust (`rmp_serde::to_vec`) and Go (`vmihailenco/msgpack v5`), keeping the two languages interoperable without a schema registry.
- Connection pooling amortises TCP handshake cost to zero for steady-state workloads.

### Negative / risks

- Three distinct transport codepaths increase testing surface. Mitigated by the `blazil-bench` harness which exercises all paths under load.
- MessagePack array format is position-sensitive: adding fields to `TransactionRequest` must always be done at the end to avoid deserialisation breakage. Enforced by convention and documented in `core/transport/src/protocol.rs`.
- Aeron requires a running media driver sidecar; the benchmark Dockerfile includes it but production manifests do not.

---

## References

- `core/transport/src/protocol.rs` — `TransactionRequest` wire format
- `core/transport/src/connection.rs` — TCP server + `build_event`
- `services/payments/internal/engine/client.go` — Go TCP client + connection pool
- `services/payments/internal/engine/transfer_client.go` — 2PC client
- `bench/src/scenarios/` — TCP, Aeron, and AF_XDP benchmark scenarios
- `core/aeron-sys/` — Aeron FFI bindings
