# Blazil

**Source-available financial infrastructure.**  
**62,770 TPS on commodity cloud hardware.**

[![Build](https://github.com/Kolerr-Lab/BLAZIL/actions/workflows/ci.yml/badge.svg)](https://github.com/Kolerr-Lab/BLAZIL/actions)
[![License: BSL 1.1](https://img.shields.io/badge/license-BSL%201.1-orange.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable%202021-orange.svg)](https://www.rust-lang.org)
[![Go](https://img.shields.io/badge/go-1.22%2B-00ADD8.svg)](https://go.dev)
[![TPS](https://img.shields.io/badge/peak%20TPS-62%2C770-brightgreen.svg)](docs/do-benchmark-report-final.md)

---

## Results

Measured on 3× DigitalOcean c2-4vcpu-8GB droplets — $252/month total.

| Metric | Result |
|--------|--------|
| Peak TPS | **62,770** |
| P99 Latency | **23 ms** |
| Error Rate | **0.00%** |
| Hardware | 3× DO c2-4vcpu-8GB ($252/mo) |
| vs Visa peak | **2.6× faster** |
| vs Mojaloop | **62× faster** |
| Cost per million TXN | **$0.0018** |

Full methodology: [docs/do-benchmark-report-final.md](docs/do-benchmark-report-final.md)

---

## Architecture

```
  Clients (gRPC / REST)
         │
  ┌──────▼──────────────────────────────────────────┐
  │              Go Services                         │
  │  ┌──────────┐ ┌─────────┐ ┌────────┐ ┌───────┐  │
  │  │ Payments │ │ Banking │ │Trading │ │Crypto │  │
  │  └────┬─────┘ └────┬────┘ └───┬────┘ └───┬───┘  │
  └───────┼────────────┼──────────┼───────────┼──────┘
          │            │          │           │
          └────────────▼──────────▼───────────┘
                       │  gRPC (bidirectional streaming)
  ┌────────────────────▼────────────────────────────┐
  │              Rust Core Engine                    │
  │                                                 │
  │  Transport (io_uring)  →  LMAX Disruptor Ring   │
  │                        →  Risk checks           │
  │                        →  Ledger batch          │
  └────────────────────┬────────────────────────────┘
                       │  TigerBeetle client
  ┌────────────────────▼────────────────────────────┐
  │          TigerBeetle VSR Cluster                │
  │   replica-0       replica-1       replica-2     │
  │  (node-1:3000)  (node-2:3001)  (node-3:3002)   │
  └─────────────────────────────────────────────────┘
```

**How a transaction flows:**

1. Client sends a `ProcessPaymentStream` gRPC request  
2. Payments service validates and forwards to the Rust engine  
3. Engine enqueues onto the LMAX Disruptor ring buffer (lock-free)  
4. Pipeline thread dequeues, runs risk checks, accumulates a batch of ≤100 transfers  
5. Batch is committed to TigerBeetle in one VSR round (~1.6 ms) — one consensus cost for 100 transfers  
6. Response is streamed back; client measures end-to-end latency  

**Why it's fast:**
- Batch commits: 100 transfers × 1 VSR round = 100× multiplier over unary commits  
- Streaming: 256 in-flight requests per stream = no round-trip blocking  
- io_uring: zero-syscall I/O path in the transport layer  
- Single goroutine per stream: eliminates gRPC conn lock contention  

---

## Quick Start

### Demo (single node, no external deps)

```bash
git clone https://github.com/Kolerr-Lab/BLAZIL.git
cd BLAZIL
./scripts/demo.sh
```

### Development stack

```bash
# Prerequisites: Docker, Rust stable, Go 1.22+
./scripts/setup.sh
docker compose -f infra/docker/docker-compose.dev.yml up -d
cargo build --workspace
cd services && go build ./...
```

### 3-node production cluster (DigitalOcean)

```bash
# Provision three droplets, then on node-1:
BLAZIL_NODE_ID=node-1 ./scripts/do-start.sh 10.0.0.1 10.0.0.2 10.0.0.3

# On node-2:
BLAZIL_NODE_ID=node-2 ./scripts/do-start.sh 10.0.0.1 10.0.0.2 10.0.0.3

# On node-3:
BLAZIL_NODE_ID=node-3 ./scripts/do-start.sh 10.0.0.1 10.0.0.2 10.0.0.3
```

Grafana is available at `http://<node-1-ip>:3001` (admin / blazil).

---

## Stack

| Layer | Technology | Why |
|-------|-----------|-----|
| Core engine | **Rust** | Zero-cost abstractions, no GC pauses, deterministic latency |
| Ring buffer | **LMAX Disruptor** | Lock-free single-producer pipeline; 12M+ ops/s on one core |
| Ledger | **TigerBeetle** | Purpose-built financial ledger; VSR consensus; io_uring |
| Transport | **io_uring** | Zero-syscall I/O; avoids kernel/user context switches |
| Services | **Go** | Fast iteration, first-class gRPC, excellent concurrency |
| RPC | **gRPC bidirectional streaming** | Async pipelining; 300× vs unary RPC |
| Observability | **Prometheus + Grafana + OTel** | Real-time metrics, distributed tracing |
| Policy | **OPA (Rego)** | Declarative authorization; auditable |
| Secrets | **HashiCorp Vault** | Dynamic secrets, audit log |

---

## Modules

| Module | Language | Purpose |
|--------|----------|---------|
| `core/engine` | Rust | LMAX Disruptor pipeline; transaction batching |
| `core/transport` | Rust | io_uring ingestion; Prometheus metrics server |
| `core/ledger` | Rust | TigerBeetle VSR client; double-entry abstractions |
| `core/risk` | Rust | Pre-commit risk checks and AML rules |
| `core/common` | Rust | Shared types, errors, traits |
| `services/payments` | Go | ISO 20022, ACH, SEPA payment rails |
| `services/banking` | Go | Core banking accounts, deposits, withdrawals |
| `services/trading` | Go | Order management, FIX protocol, clearing |
| `services/crypto` | Go | Digital asset custody, chain abstraction |
| `bench` | Rust | Criterion benchmarks + 4-scenario load harness |
| `tools/stresstest` | Go | gRPC streaming stress tester (62K TPS verified) |
| `infra` | YAML/HCL | Docker Compose, Kubernetes, Terraform |

---

## Benchmarks

### Production cluster (3× DO c2-4vcpu-8GB)

```
Configuration: 1 goroutine × 256 in-flight window
Duration:       120 s sustained

Throughput:     62,770 TPS
P50 latency:    12.3 ms
P99 latency:    26.8 ms
P99.9 latency:  43.2 ms
Error rate:     0.00%

vs Visa (24,000 TPS peak):   2.6×
vs Mojaloop (~1,000 TPS):   62×
```

### Local (Apple Silicon, single process, in-memory)

```
Ring buffer (raw):    12,500,000 ops/s   P99  84 ns
Pipeline (in-memory): 19,607,843 ops/s   P99  83 ns
End-to-end TCP:           39,651 TPS     P99  38 µs
```

Run locally:
```bash
cargo run -p blazil-bench --release     # 4-scenario harness
cargo bench -p blazil-bench             # Criterion micro-benchmarks
```

Run against the cluster:
```bash
cd tools/stresstest
GOOS=linux GOARCH=amd64 go build -o stresstest-linux .
scp stresstest-linux root@<node-1>:~/
ssh root@<node-1> './stresstest-linux -target=<node-1-private-ip>:50051 -duration=120s'
```

---

## Roadmap

### v0.1 — Production baseline ✅ (March 2026)
- [x] 62,770 TPS on 3-node cluster
- [x] TigerBeetle VSR 3-replica consensus
- [x] gRPC bidirectional streaming pipeline
- [x] io_uring zero-copy transport
- [x] Prometheus + Grafana observability
- [x] Docker Compose cluster deployment

### v0.2 — Multi-shard + HA (Q2 2026)
- [ ] Dynamic sharding with consistent hashing
- [ ] Cross-shard atomic transactions (2PC)
- [ ] Automatic shard rebalancing
- [ ] 5-node TigerBeetle cluster (double fault tolerance)
- [ ] Kubernetes Helm chart with HPA

### v0.3 — Compliance + rails (Q3 2026)
- [ ] ISO 20022 message validation
- [ ] SEPA / ACH / SWIFT integration
- [ ] KYC/AML workflow engine
- [ ] Real-time sanctions screening (OFAC, EU)
- [ ] Regulatory audit export (Basel III, PCI DSS)

### v1.0 — Enterprise GA (Q4 2026)
- [ ] 500,000+ TPS target (upgraded instance sizes)
- [ ] Multi-region active-active deployment
- [ ] SOC 2 Type II audit
- [ ] 99.999% availability SLA
- [ ] Managed cloud offering

---

## Contributing

Contributions are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md) first.

Key areas where help is most valuable:

- **Performance** — profiling, hot-path optimisation, new transport backends
- **Rails** — new payment rail integrations (SEPA, RTP, PIX, UPI)
- **Compliance** — KYC/AML rule libraries, regulatory report templates
- **Observability** — additional Grafana dashboards, alert rules
- **Documentation** — runbooks, architecture diagrams, API references

For security issues, see [SECURITY.md](SECURITY.md).

---

## License

Copyright 2026 Kolerr Lab — Licensed under the [Business Source License 1.1](LICENSE).

Blazil is **free for non-commercial use**. Commercial production use requires a separate license.  
For commercial licensing, contact: **hello@blazil.com**

The Business Source License automatically converts to Apache 2.0 four years after each release.

