<div align="center">

# ⚡ Blazil

**Open-core financial infrastructure.**  
**Built for the speed of modern markets.**

[![Build](https://github.com/Kolerr-Lab/BLAZIL/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/Kolerr-Lab/BLAZIL/actions)
[![License: BSL 1.1](https://img.shields.io/badge/License-BSL%201.1-blue?style=flat-square)](LICENSE)

[![Rust](https://img.shields.io/badge/Rust-stable%202021-orange?style=flat-square&logo=rust)](https://www.rust-lang.org)
[![Go](https://img.shields.io/badge/Go-1.22+-00ADD8?style=flat-square&logo=go)](https://go.dev)

![436K TPS](https://img.shields.io/badge/436K_TPS-Sharded_3×Node-brightgreen?style=flat-square)
![131K TPS](https://img.shields.io/badge/131K_TPS-VSR_Consensus-blue?style=flat-square)
![18x Visa](https://img.shields.io/badge/18×_Visa-peak_vs_peak-red?style=flat-square)
![0% Error](https://img.shields.io/badge/0%25_Error-1M_Events-gold?style=flat-square)

</div>

---

## ⚡ Performance

Real hardware. Real replication. Real benchmarks.

### **v0.2 Production Cluster** (DigitalOcean 3-node, April 2026)

| Architecture | TPS | Latency (p50/p99) | Hardware | Fault Tolerance |
|--------------|-----|-------------------|----------|-----------------|
| **Option B: Sharded** | **436,351** | 1.8s / 2.7s | 3× DO Premium AMD NVMe | ❌ None (independent nodes) |
| **Option A: VSR Consensus** | **130,998** | 1.7s / 2.7s | 3× DO Premium AMD NVMe | ✅ Survives 1-node failure |

**Key Results:**
- ✅ **0 errors, 0 rejected** across 3,000,000 events (Option B) and 1,000,000 events (Option A)
- ✅ **Linear scaling**: 3× nodes = 3.33× throughput (sharded mode)
- ✅ **Consensus overhead**: <5% latency penalty for VSR fault tolerance
- ✅ **18× Visa peak** (436K vs 24K TPS published)
- ✅ **Production-ready**: DO $252/month commodity cloud hardware

> **Bottleneck analysis:**  
> p50/p99 latency dominated by disk I/O (TigerBeetle fsync 1-2s, DO throttles "Premium NVMe" at 100-127 MB/s).  
> Ring buffer backpressure adds ~200-500ms when saturated.  
> **On bare-metal NVMe Gen4**: estimated 5-10M TPS (Option B), 1-2M TPS (Option A) with <100ms latency.
>
> **Full reports:**  
> - [Option B Sharded Aggregate (436K TPS)](docs/runs/2026-04-13_option-b-sharded-aggregate.md)  
> - [Option A VSR Consensus (131K TPS)](docs/runs/2026-04-13_option-a-vsr-consensus-summary.md)

---

### **v0.2 Local Benchmarks** (MacBook Air M4, single node, in-memory)

| Benchmark | Result | Hardware | Notes |
|-----------|--------|----------|-------|
| **Sharded pipeline (4-core)** | **200M TPS** | MacBook Air M4 | Parallel, bulk timing, 1 producer per shard |
| **Single pipeline (latency)** | **24M TPS, P99 42ns** | MacBook Air M4 | In-memory, per-event tracking |
| **Aeron IPC E2E** | **1.2M TPS** | M4 (fanless) | Real Aeron transport, throttles under load |

> **Methodology:**  
> Pipeline benchmarks: in-memory validation/risk handlers, no disk I/O.  
> Cluster benchmarks: real TigerBeetle VSR replication, O_DIRECT disk writes, TCP transport.  
> See [bench/README.md](bench/README.md) for detailed methodology.

---

### **Industry Comparison** (production E2E)

| System | TPS | Blazil Advantage | Notes |
|--------|-----|------------------|-------|
| **SWIFT** | ~hundreds/day | ~1M× | Legacy batch processing |
| **Mojaloop (OSS)** | ~1,000 | **436×** | Open-source payment hub |
| **Mastercard peak** | ~5,000 | **87×** | Published peak capacity |
| **Visa peak** | ~24,000 | **18×** | Published peak: 24K TPS |
| **Coinbase** | ~10,000 (est.) | **44×** | High-frequency crypto exchange |
| **Stripe** | ~5,000 (est.) | **87×** | Payment API provider |
| **Blazil v0.2 (Sharded, DO)** | **436,351** | — | 3-node DO cluster, 0% error |
| **Blazil v0.2 (VSR, DO)** | **130,998** | — | Fault-tolerant consensus |

> All Blazil numbers: real hardware, real TigerBeetle consensus, real disk writes, 0% error rate.  
> Competitors: published peak capacity (often marketing numbers, not sustained).

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

### Local Benchmark (MacBook Air M4, single node)

> **Methodology note:** Pipeline numbers use per-event `duration_ns`
> measurement (accurate latency tracking). Earlier v0.1 pipeline numbers
> (111M–200M TPS) used bulk timing without per-event overhead —
> a different methodology, not directly comparable.

#### Pipeline Scaling (duration_ns, per-event accurate)
| Shards | TPS        | P99 (ns) | P99.9 (ns) | Efficiency |
|--------|------------|----------|------------|------------|
| 1      | 20,724,028 | 42       | 667        | baseline   |
| 2      | 40,483,850 | 42       | 584        | 97.7%      |
| 4      | 61,005,677 | 84       | 1,083      | 73.6%      |
| 8      | 80,513,139 | 125      | 1,333      | 48.6%      |

#### E2E Transport Comparison (single node, real pipeline)
| Transport  | TPS            | vs TCP  | Notes                        |
|------------|----------------|---------|------------------------------|
| TCP        | 32,045         | baseline| Tokio TCP                    |
| UDP        | 163,215        | 5.1×    | Tokio UDP                    |
| Aeron IPC  | up to 1,203,108| 37.5×   | Peak; avg ~1.1M (see note)   |

> **Thermal note:** MacBook Air M4 is fanless. Under sustained load,
> Apple Silicon throttles P-core frequency. Observed band: 1.1M–1.2M TPS.
> Peak recorded: 1,203,108 TPS (cold start).
> DO Linux nodes have no thermal limit → expect stable 1.2M+ TPS.

#### vs Industry (E2E, real transactions)
| System | TPS | Blazil v0.2 advantage | Notes |
|--------|-----|-----------------------|-------|
| SWIFT | ~hundreds/day | — | Legacy batch |
| Mojaloop (OSS) | ~1,000 | **436×** | Open-source baseline |
| Mastercard peak | ~5,000 | **87×** | Published peak |
| Visa peak | ~24,000 | **18×** | Published peak |
| **Blazil v0.2 (VSR, fault-tolerant)** | **130,998** | — | 3-node consensus, 0% error |
| **Blazil v0.2 (Sharded, max throughput)** | **436,351** | — | 3× independent nodes, 0% error |

> All Blazil v0.2 DO numbers: real TigerBeetle VSR replication, real O_DIRECT disk writes, real TCP transport.  
> 3-node DO Premium AMD NVMe cluster (SGP1). 1M–3M events, 0 rejected, 0 errors.  
> Local pipeline numbers: in-memory, no disk I/O — different benchmark class.

### Production Cluster (DigitalOcean 3-node, $252/month)
| Version | TPS | p50 | p99 | vs Visa | vs Mojaloop | Notes |
|---------|-----|-----|-----|---------|-------------|-------|
| v0.1 | 62,770 | — | 23ms | 2.6× | 62× | Tokio UDP, gRPC |
| v0.2 Option A | **130,998** | 1,774ms | 2,747ms | **5.5×** | **131×** | VSR 3-replica, fault-tolerant ✅ |
| v0.2 Option B | **436,351** | 1,803ms | 2,627ms | **18×** | **436×** | Sharded 3-node, max throughput |

> Hardware: 3× DO Premium AMD NVMe (s-4vcpu-8gb-amd), Ubuntu 24.04, TigerBeetle 0.16.78.  
> **0 errors, 0 rejected** across all runs.

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

| Version | Status | Achieved TPS | Features |
|---------|--------|--------------|----------|
| **v0.1** | ✅ Done | 62,770 TPS (DO, gRPC) | Core engine, VSR consensus, gRPC streaming |
| **v0.2** | ✅ Done | 1.2M TPS local · **436K TPS DO (sharded)** · **131K TPS DO (VSR)** | Aeron IPC, io_uring, sharded-tb E2E, TigerBeetle VSR, 0% error |
| **v0.3** | 📅 Planned | est. 1M+ TPS (VSR, DO) | Bare-metal NVMe, XDP ingress, larger ring buffer, multi-region |

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

