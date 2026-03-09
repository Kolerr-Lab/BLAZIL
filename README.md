```
██████╗     ██╗      █████╗    ███████╗   ██╗   ██╗      
██╔══██╗    ██║     ██╔══██╗   ╚══███╔╝   ██║   ██║      
██████╔╝    ██║     ███████║     ███╔╝    ██║   ██║      
██╔══██╗    ██║     ██╔══██║    ███╔╝     ██║   ██║      
██████╔╝    ███████╗██║  ██║   ███████╗   ╚██████╔╝      
╚═════╝     ╚══════╝╚═╝  ╚═╝   ╚══════╝    ╚═════╝       
```

> **Open-source financial infrastructure at the speed of fire**

[![Build Status](https://github.com/blazil/blazil/actions/workflows/ci.yml/badge.svg)](https://github.com/blazil/blazil/actions)
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
| `bench` | Rust | Comprehensive performance benchmark suite (Criterion) |
| `infra` | YAML/HCL | Docker Compose, Kubernetes, Terraform, Ansible |
| `observability` | YAML | Prometheus, Grafana, OpenTelemetry collector configs |

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
git clone https://github.com/blazil/blazil.git
cd blazil
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
