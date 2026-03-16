<div align="center">

# ⚡ Blazil

**Open-core financial infrastructure.**  
**Built for the speed of modern markets.**

[![Build](https://github.com/Kolerr-Lab/BLAZIL/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/Kolerr-Lab/BLAZIL/actions)
[![License: BSL 1.1](https://img.shields.io/badge/License-BSL%201.1-blue?style=flat-square)](LICENSE)

[![Rust](https://img.shields.io/badge/Rust-stable%202021-orange?style=flat-square&logo=rust)](https://www.rust-lang.org)
[![Go](https://img.shields.io/badge/Go-1.22+-00ADD8?style=flat-square&logo=go)](https://go.dev)

![62,770 TPS](https://img.shields.io/badge/62%2C770_TPS-E2E_Cluster-brightgreen?style=flat-square)
![P99 23ms](https://img.shields.io/badge/P99_23ms-3--node-blue?style=flat-square)
![2.6x Visa](https://img.shields.io/badge/2.6×_Visa-peak_vs_peak-red?style=flat-square)
![18.8M TPS](https://img.shields.io/badge/18.8M_TPS-Pipeline-orange?style=flat-square)

</div>

---

## ⚡ Performance

Real hardware. Real replication. Real benchmarks.

| Benchmark | Result | Hardware | Notes |
|-----------|--------|----------|-------|
| **Pipeline throughput** | **18,867,924 TPS** | MacBook Air M4 | In-memory, single node, lock-free |
| **E2E peak throughput** | **62,770 TPS** | 3× DO c2-4vcpu-8GB | Real VSR consensus + disk writes |
| **P99 latency** | **26.8 ms** | 3-node cluster | gRPC bidirectional streaming |
| **vs Visa (peak)** | **2.6×** | $252/month cloud | Published peak: 24,000 TPS |
| **vs Mojaloop (OSS)** | **62×** | commodity hardware | Open-source baseline: ~1,000 TPS |

> **No mocks. No in-memory tricks.**  
> All cluster benchmarks use real TigerBeetle VSR replication (3-node consensus),  
> real disk writes (io_uring), and real gRPC transport over the network.
> 
> Full methodology: [docs/do-benchmark-report-final.md](docs/do-benchmark-report-final.md)

---

## 🏗 Architecture

```mermaid
graph LR
    A[Client] -->|gRPC Stream| B[Go Services]
    B -->|TCP/io_uring| C[Rust Engine]
    C -->|LMAX Disruptor| D[Pipeline]
    D -->|Batch 100×| E[TigerBeetle VSR]
    E -->|3-node consensus| F[Ledger]
```

**Zero-copy stack from client to disk:**

- **Ingress**: gRPC bidirectional streaming (zero RTT, 256 in-flight window)
- **Logic**: Rust + LMAX Disruptor ring buffer (12.5M ops/s, 84ns P99)
- **Storage**: TigerBeetle VSR (3-node fault-tolerant consensus)
- **I/O**: io_uring (zero-copy kernel bypass, no syscalls)

**How a transaction flows:**

1. Client opens a persistent gRPC stream to the payments service
2. Service validates and forwards to the Rust engine via TCP
3. Engine enqueues onto the LMAX Disruptor ring buffer (lock-free, single producer)
4. Pipeline thread dequeues, runs risk checks, accumulates batches of ≤100 transfers
5. Batch commits to TigerBeetle in one VSR round (~1.6ms) — one consensus cost for 100 transfers
6. Response streams back; client measures end-to-end latency

**Why it's fast:**

- **Batch commits**: 100 transfers × 1 VSR round = 100× throughput multiplier
- **Streaming**: 256 in-flight requests per stream = no round-trip blocking
- **io_uring**: zero-syscall I/O path in transport layer
- **Lock-free pipeline**: single-producer ring buffer eliminates contention  

---

## 🚀 Quick Start

**One command. Zero configuration.**

```bash
git clone https://github.com/Kolerr-Lab/BLAZIL
cd BLAZIL
./scripts/demo.sh
```

Starts a single-node cluster with all services on `localhost`.

**Development stack:**

```bash
# Prerequisites: Docker, Rust stable, Go 1.22+
./scripts/setup.sh
docker compose -f infra/docker/docker-compose.dev.yml up -d
cargo build --workspace
cd services && go build ./...
```

**3-node production cluster (DigitalOcean):**

```bash
# On node-1:
BLAZIL_NODE_ID=node-1 ./scripts/do-start.sh 10.0.0.1 10.0.0.2 10.0.0.3

# On node-2:
BLAZIL_NODE_ID=node-2 ./scripts/do-start.sh 10.0.0.1 10.0.0.2 10.0.0.3

# On node-3:
BLAZIL_NODE_ID=node-3 ./scripts/do-start.sh 10.0.0.1 10.0.0.2 10.0.0.3
```

Grafana → `http://<node-1-ip>:3001` (admin / blazil)

---

## 🛠 Stack

| Layer | Technology | Why |
|-------|-----------|-----|
| **Engine** | Rust + LMAX Disruptor | Lock-free pipeline, 84ns P99 latency |
| **Services** | Go + gRPC Streaming | Zero RTT, 256 in-flight window |
| **Ledger** | TigerBeetle VSR | Fastest financial database on Earth |
| **Transport** | io_uring | Zero-copy kernel I/O bypass |
| **Replication** | VSR consensus | 3-node fault tolerance |
| **Observability** | Prometheus + Grafana + OTel | Real-time metrics, distributed tracing |
| **Security** | Vault + Keycloak + OPA | Production-grade secrets & policy |

---

## 📊 Benchmarks

**Production cluster (3× DO c2-4vcpu-8GB, $252/month):**

```
Configuration: 1 goroutine × 256 in-flight window
Duration:       120s sustained

Throughput:     62,770 TPS
P50 latency:    12.3 ms
P99 latency:    26.8 ms
P99.9 latency:  43.2 ms
Error rate:     0.00%

vs Visa (24,000 TPS peak):     2.6×
vs Mojaloop (~1,000 TPS):     62×
```

**Local (Apple Silicon M4, single process, in-memory):**

```
Ring buffer (raw):    12,500,000 ops/s   P99  84 ns
Pipeline (no I/O):    18,867,924 ops/s   P99  83 ns
End-to-end TCP:           39,947 TPS     P99  38 µs
```

**Run the benchmarks:**

```bash
# Local micro-benchmarks
cargo run -p blazil-bench --release
cargo bench -p blazil-bench

# Cluster stress test
cd tools/stresstest
GOOS=linux GOARCH=amd64 go build -o stresstest-linux .
scp stresstest-linux root@<node-1>:~/
ssh root@<node-1> './stresstest-linux -target=<private-ip>:50051 -duration=120s'
```

---

## 🗺 Roadmap

| Version | Status | Target TPS | Features |
|---------|--------|-----------|----------|
| **v0.1** | ✅ Done | 62,770 TPS | Core engine, VSR consensus, gRPC streaming |
| **v0.2** | 🔄 Next | 200M+ TPS | Aeron UDP, full io_uring stack, multi-shard |
| **v0.3** | 📅 Planned | 500M+ TPS | XDP ingress, RDMA replication, compliance |

---

## 🤝 Contributing

We welcome contributions — see [CONTRIBUTING.md](CONTRIBUTING.md).

**High-value areas:**

- **Performance** — hot-path optimization, profiling, new transports
- **Rails** — payment rail integrations (SEPA, RTP, PIX, UPI)
- **Compliance** — KYC/AML rules, regulatory reporting
- **Documentation** — runbooks, architecture diagrams, API references

For security issues: [SECURITY.md](SECURITY.md)

---

## 📄 License

Blazil is source-available under [Business Source License 1.1](LICENSE).

- ✅ **Free for non-commercial use**
- ✅ **Free for research & evaluation**
- 💼 **Commercial license for production use**
- 🔄 **Converts to Apache 2.0 after 4 years**

**Commercial licensing:** lab.kolerr@kolerr.com

---

<div align="center">

**Built by [Kolerr Lab](https://github.com/Kolerr-Lab)**  
Copyright © 2026 Kolerr Lab

</div>

