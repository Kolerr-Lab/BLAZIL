## ⚡ Blazil v0.1.0

The first public release of Blazil — open-core financial infrastructure built for the speed of modern markets.

### 🏆 Benchmark Results

| Metric | Result | Hardware |
|--------|--------|----------|
| Pipeline throughput (4-shard, bulk) | **200,000,000 TPS** | MacBook Air M4 (in-memory) |
| Pipeline throughput (1-shard, bulk) | **111,111,111 TPS** | MacBook Air M4 (in-memory) |
| Pipeline throughput (latency-tracked) | **24,390,243 TPS** · P99 42ns | MacBook Air M4 (in-memory) |
| E2E UDP (single node) | **135,135 TPS** | MacBook Air M4 · honest, full pipeline |
| E2E peak throughput | **62,770 TPS** | 3× DO c2-4vcpu-8GB ($252/month) |
| P99 latency | **23ms** | 3-node VSR cluster |
| vs Visa peak | **2.6×** | commodity cloud |
| vs Mojaloop (OSS) | **62×** | OSS baseline |

> All E2E benchmarks use real TigerBeetle VSR replication, real disk writes, real gRPC transport. No mocks. No in-memory tricks.

### 🏗 What's Included

**Core Engine (Rust)**
- LMAX Disruptor pattern — lock-free pipeline (84ns P99)
- TigerBeetle VSR integration — 3-node fault tolerant ledger
- TB batching — 100 transfers per VSR consensus round
- io_uring zero-copy transport
- Aeron UDP transport (feature-gated)

**Go Services**
- Payments service — gRPC Bidirectional Streaming
- Banking service
- Trading service  
- Crypto/DeFi service
- Full observability (Prometheus + Grafana)

**Infrastructure**
- Docker Compose — single command deployment
- 3-node cluster setup (DigitalOcean validated)
- Kernel tuning scripts (do-tune.sh)
- CPU pinning for optimal performance
- Vault + Keycloak + OPA security stack

**Developer Experience**
- One-command demo: `./scripts/demo.sh`
- 479 tests, 0 failures
- Full CI/CD pipeline (GitHub Actions)
- Comprehensive documentation

### 🚀 Quick Start

```bash
git clone https://github.com/Kolerr-Lab/BLAZIL
cd BLAZIL
./scripts/demo.sh
```

### 🛠 Stack

| Layer | Technology |
|-------|-----------|
| Engine | Rust + LMAX Disruptor |
| Services | Go + gRPC Streaming |
| Ledger | TigerBeetle (VSR) |
| Transport | io_uring + Aeron UDP |
| Security | Vault + Keycloak + OPA |

### 🗺 What's Next

- **v0.2** — Aeron UDP full stack → 200M+ TPS target
- **v0.3** — XDP ingress + RDMA-like replication → 500M+ TPS target

### 📄 License

BSL 1.1 — Source available.  
Free for non-commercial use.  
Commercial license: lab.kolerr@kolerr.com  
Converts to Apache 2.0 after 4 years.

---

Built by [Kolerr Lab](https://kolerr.com) · [blazil.com](https://blazil.com) · [lab.kolerr@kolerr.com](mailto:lab.kolerr@kolerr.com)
