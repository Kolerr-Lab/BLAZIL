```
██████╗     ██╗      █████╗    ███████╗   ██╗   ██╗
██╔══██╗    ██║     ██╔══██╗   ╚══███╔╝   ██║   ██║
██████╔╝    ██║     ███████║     ███╔╝    ██║   ██║
██╔══██╗    ██║     ██╔══██║    ███╔╝     ██║   ██║
██████╔╝    ███████╗██║  ██║   ███████╗   ██║   ███████╗
╚═════╝     ╚══════╝╚═╝  ╚═╝   ╚══════╝   ╚═╝   ╚══════╝
```

> **Open-source financial infrastructure at the speed of fire**

[![Build Status](https://github.com/Kolerr-Lab/BLAZIL/actions/workflows/ci.yml/badge.svg)](https://github.com/Kolerr-Lab/BLAZIL/actions)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable%20%7C%202021-orange.svg)](https://www.rust-lang.org)
[![Go](https://img.shields.io/badge/go-1.22+-00ADD8.svg)](https://go.dev)

---

## Vision

Blazil is an open-source, ultra-high-performance transaction processing system purpose-built for Fintech and Banking. Designed to handle **1 million to 10 million transactions per second** with microsecond-level latencies, Blazil bridges the gap between the speed demands of modern digital finance and the correctness guarantees required by regulated financial institutions. Built in Rust for the performance-critical core and Go for domain services, Blazil provides a complete, production-grade financial infrastructure stack — from ingestion to ledger to compliance — that any bank, payment company, or fintech can deploy and extend.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    API Gateway (Go)                          │
└──────────────────────────┬──────────────────────────────────┘
                           │
          ┌────────────────┼────────────────┐
          │                │                │
   ┌──────▼──────┐  ┌──────▼──────┐  ┌──────▼──────┐
   │  Payments   │  │   Banking   │  │   Trading   │
   │   (Go)      │  │    (Go)     │  │    (Go)     │
   └──────┬──────┘  └──────┬──────┘  └──────┬──────┘
          │                │                │
          └────────────────┼────────────────┘
                           │
          ┌────────────────▼────────────────┐
          │     Rust Core Engine            │
          │  Transport → Engine → Ledger    │
          │         Risk checks             │
          └────────────────┬────────────────┘
                           │
          ┌────────────────▼────────────────┐
          │         TigerBeetle             │
          │    (1M+ TPS Ledger Engine)      │
          └─────────────────────────────────┘
```

See [docs/architecture/](docs/architecture/) for detailed architecture documentation.

---

## Quick Start

```bash
# 1. Set up your development environment
./scripts/setup.sh

# 2. Start all infrastructure services
docker compose -f infra/docker/docker-compose.dev.yml up -d

# 3. Build all components
cargo build --workspace && cd services && go build ./...
```

---

## Module Overview

| Module | Language | Description |
|--------|----------|-------------|
| `core/engine` | Rust | Ultra-low-latency transaction engine using Disruptor pipeline |
| `core/transport` | Rust | High-throughput network ingestion layer (Aeron + io_uring) |
| `core/ledger` | Rust | TigerBeetle client and double-entry accounting abstractions |
| `core/risk` | Rust | Pre-trade risk checks and AML rules engine |
| `core/common` | Rust | Shared types, error types, and traits across Rust crates |
| `services/gateway` | Go | API gateway — routing, auth, rate limiting, load balancing |
| `services/payments` | Go | Payment processing — ISO 20022, ISO 8583, ACH, SEPA rails |
| `services/banking` | Go | Core banking — accounts, deposits, withdrawals, interest |
| `services/trading` | Go | Order management system, FIX protocol, clearing |
| `services/crypto` | Go | Chain abstraction, digital asset custody, DeFi rails |
| `services/compliance` | Go | KYC workflows, sanctions screening, regulatory reporting |
| `bench` | Rust | Benchmark suite — 4 scenarios (ring buffer, pipeline, TCP, TigerBeetle) + Criterion |
| `infra` | YAML/HCL | Docker Compose, Kubernetes, Terraform, Ansible |
| `observability` | YAML | Prometheus, Grafana, OpenTelemetry collector configs |

---

## Benchmarks

Measured on Apple Silicon (ARM64) · macOS · Rust 1.94.0 · release build · March 2026

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 BLAZIL BENCHMARK RESULTS
 Hardware: Apple Silicon (ARM64)
 OS: macos
 Rust: rustc 1.94.0 (4a4ef493e 2026-03-02)
 Date: 2026-03-11
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

 Scenario                              TPS      P99 Latency
 ─────────────────────────────────────────────────────
 Ring Buffer (raw)              12,500,000            84 ns
 Pipeline (in-memory)           19,607,843            83 ns
 End-to-End TCP                     39,651            38 µs
 TigerBeetle (real)*               SKIPPED               —

 * Requires BLAZIL_TB_ADDRESS — skipped if not set

 Detailed latency (Pipeline in-memory):
   P50:   41 ns
   P95:   42 ns
   P99:   83 ns
   P99.9: 667 ns

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Context:
   Visa peak:        ~24,000 TPS
   NASDAQ:       ~2,000,000 TPS
   Blazil target: 10,000,000 TPS (multi-node)
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

> Ring buffer and pipeline numbers exceed 10M TPS target on a single core.
> TCP bottleneck is serialisation + loopback latency — expected for a single-node
> sequential client. Production clients use persistent connections and pipelining.
> TigerBeetle numbers require a Linux host with `io_uring`; run with
> `BLAZIL_TB_ADDRESS=127.0.0.1:3000 cargo run -p blazil-bench --release`.

Reproduce:
```bash
cargo run -p blazil-bench --release
# Criterion micro-benchmarks:
cargo bench -p blazil-bench
```

---

## Technology Stack

| Component | Technology | Rationale |
|-----------|-----------|-----------|
| Transaction Engine | Rust | Zero-cost abstractions, no GC pauses |
| Business Services | Go | Fast development, excellent concurrency |
| Ledger | TigerBeetle | Purpose-built for financial accounting at 1M+ TPS |
| Messaging | Redpanda | Kafka-compatible, 2x faster, no JVM |
| Identity | Keycloak | Enterprise-grade auth and authorization |
| Secrets | HashiCorp Vault | Dynamic secrets, audit logging |
| Observability | OTel + Prometheus + Grafana | Industry standard — traces, metrics, logs |

---

## Development

### Prerequisites

- Rust (stable, 2021 edition)
- Go 1.22+
- Docker + Docker Compose

### Getting Started

```bash
# Clone and setup
git clone https://github.com/Kolerr-Lab/BLAZIL.git
cd BLAZIL
./scripts/setup.sh
```

### Running Quality Checks

```bash
# Run all lints, tests, and security audits
./scripts/check.sh

# Run benchmarks
./scripts/bench.sh
```

---

## Contributing

We welcome contributions! Please read [CONTRIBUTING.md](CONTRIBUTING.md) before submitting a PR.

For security vulnerabilities, please read [SECURITY.md](SECURITY.md).

---

## License

Copyright 2026 Blazil Contributors

Licensed under the [Apache License, Version 2.0](LICENSE).
