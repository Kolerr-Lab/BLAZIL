# Changelog

All notable changes to Blazil will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

---

## v0.3.2 (May 2026)

### 🎯 Priority Queuing

**Production-ready multi-stream priority routing** for critical event handling.

#### Architecture
- **Multi-stream Aeron IPC**: 3 independent priority levels (Critical/High/Normal)
- **Stream allocation**: Critical (100/101), High (200/201), Normal (300/301)
- **Legacy support**: Streams 1001/1002 automatically map to Normal priority
- **Independent backpressure**: Each stream has isolated flow control

#### Performance Guarantees
- **Critical events**: <1ms latency (margin calls, fraud alerts, circuit breakers)
- **High priority**: <5ms latency (VIP customers, large transactions >$1M)
- **Normal traffic**: <50ms latency (standard operations, batch processing)

#### Implementation
- New module: `core/transport/src/priority.rs` (450 LOC)
- `PriorityPublisher`: Multi-stream publisher for request/response routing
- `PrioritySubscriber`: Priority-ordered polling (Critical → High → Normal)
- **Test coverage**: 429 tests passing (426 unit + 3 Aeron integration)
- **Code quality**: Zero Clippy warnings, zero dead code, zero technical debt

#### Use Cases
- **Fintech**: Margin calls, fraud detection, compliance violations
- **AI**: Model drift alerts, anomaly detection, critical inference requests
- **Operations**: System health alerts, circuit breaker activation

**Documentation**: [docs/PRIORITY_QUEUING.md](docs/PRIORITY_QUEUING.md)

---

## v0.3.1 (April 2026)

### 🤖 AI Infrastructure

**Production-grade ML inference with zero-copy data pipeline.**

#### Datasets
- **5 production datasets**: Text/NLP, Time Series, Features, Audio, Object Detection
- **2,291 LOC**: Comprehensive data loading and preprocessing
- **57 tests passing**: Full test coverage across all datasets
- **Zero-copy I/O**: io_uring on Linux, mmap fallback on macOS

#### Capabilities
- **Text/NLP**: Vocabulary management, tokenization, special tokens ([PAD], [UNK])
- **Time Series**: Sliding windows (configurable size/stride), CSV format
- **Features**: Z-score/Min-max normalization, anomaly detection
- **Audio**: WAV processing, resampling, mono conversion, padding
- **Object Detection**: YOLO/COCO format, bounding boxes, multi-bbox support

#### Performance Target
- **1,500-2,000 RPS**: Tract ONNX inference on DO Premium AMD (4 vCPU, $84/month)
- **Cost efficiency**: 8-12× cheaper than NVIDIA Triton ($0.042 vs $0.266 per RPS/month)

**Documentation**: [docs/DATASETS_IMPLEMENTATION.md](docs/DATASETS_IMPLEMENTATION.md)

---

## v0.3 (April 2026)

### 🚀 AWS Production Benchmarks

**237K TPS on AWS i4i.4xlarge with live VSR failover testing.**

#### Performance
- **Peak TPS**: 237,763 (4-shard VSR configuration)
- **Average TPS**: 103,421 sustained
- **Latency**: P99 120ms (includes disk fsync)
- **Hardware**: AWS i4i.4xlarge (16 vCPU, 128GB RAM, 1.9TB NVMe)
- **Cost**: $1.496/hour on-demand ($0.0000063 per TPS/hour)

#### Fault Tolerance
- **Live failover**: VSR replica killed at t=80s, recovered in 37s
- **Zero errors**: 12,421,068 events, 0% error rate
- **3-node consensus**: TigerBeetle VSR quorum maintained during failure

#### Architecture
- **Dedicated TB client per shard**: Eliminates cross-shard queue contention
- **4-shard configuration**: Optimal for 16 vCPU hardware
- **VSR replication**: 3-node fault-tolerant consensus

**Full report**: [docs/runs/2026-04-19_16-44-35_sharded-tb-e2e-(4-shards).md](docs/runs/2026-04-19_16-44-35_sharded-tb-e2e-(4-shards).md)

---

## v0.2 (March 2026)

### 🚀 Performance
- **1.2M TPS new record**: Aeron IPC E2E peaks at 1,203,108 TPS
  (stable band 1.1M–1.2M on MacBook Air M4, fanless)
- Pipeline 8-shard: 80,513,139 TPS (per-event); 1-shard: 20,724,028 TPS
- 2-shard scaling: 99–110% efficiency (superlinear via account routing)
- align(128) on TransactionEvent: +31% TPS (M4 prefetcher isolation)

### Features
- `blazil-aeron-sys`: embedded Aeron C FFI transport (Aeron 1.44.1)
- `io_uring` UDP transport (Linux kernel 5.11+, disabled on macOS)
- Dynamic shard count via `BLAZIL_SHARD_COUNT` env var
- Account-based shard routing: `route_to_shard(account_id, n)`
- Cross-shard coordination via TigerBeetle linked transfers
- Per-shard metrics: `ShardMetrics` (lock-free `AtomicU64`)
- `ShardedPipeline::resize()`: live shard count change
- macOS QoS USER_INTERACTIVE + Linux core_affinity hard pinning

### Bug Fixes
- OOM kill on DO 3-node (8GB): TB batch bounded to 8,190 max
- Aeron `/dev/shm` capped: 128MB term buffer (256MB caused mmap hang on macOS)
- Ring buffer compile-time memory assertion (≤512MB total)
- Benchmark: single shared tokio runtime (was 1 runtime per shard)
- Benchmark: `duration_ns` measurement (was `ms` floor → fake 1M TPS)
- Benchmark: `spin_loop` backpressure with bounded retry + `yield_now` fallback
- Benchmark: `aeron:ipc` channel (was `aeron:udp` loopback — eliminates UDP stack overhead)
- Script permissions: `chmod +x` + `git update-index`

### vs v0.1
| Metric | v0.1 | v0.2 |
|--------|------|------|
| Aeron IPC local | 717,306 TPS | ~1,000,000 TPS (+39%) |
| E2E DO 3-node | 62,770 TPS | pending (est. 2–4M TPS) |
| OOM on DO cluster | yes (crashes) | fixed |
| Pipeline measurement | bulk timing | per-event duration_ns |

### License
BSL 1.1 — source available, converts to Apache 2.0 after 4 years.

---

### Updated — Phase 10: Performance Breakthrough (2026-03-19)
- Sharded pipeline 4-shard bulk timing: **200,000,000 TPS** (was 167M TPS)
- Sharded pipeline 1-shard bulk timing: **111,111,111 TPS** (was 77M TPS)
- Pipeline latency-tracked: **24,390,243 TPS**, P99 42ns, P99.9 83ns
- UDP E2E: **163,215 TPS** (5.1× TCP)
- TCP E2E: **32,045 TPS** baseline
- DO cluster E2E: **62,770 TPS** unchanged (real VSR, real disk, $252/month)
- Added `UdpTransportServer` with split-fd + mpsc response channel design
- Fixed executor starvation with `Semaphore(2048)` + `sleep(1µs)` in wait loop
- Upgraded all Go modules to 1.25.8 (10 critical stdlib CVEs resolved)

### Added — Prompt #7: Benchmark Suite (2026-03-11)
- `bench/` crate: four measurement scenarios (ring buffer, pipeline, TCP, TigerBeetle)
- `bench/src/metrics.rs`: `BenchmarkResult` with P50/P95/P99/P99.9 percentiles
- `bench/src/report.rs`: structured stdout report with hardware/OS/Rust metadata
- `bench/src/scenarios/ring_buffer_scenario.rs`: raw ring-buffer throughput (no handlers)
- `bench/src/scenarios/pipeline_scenario.rs`: full 4-handler pipeline with `InMemoryLedgerClient`
- `bench/src/scenarios/tcp_scenario.rs`: end-to-end TCP with persistent connection (no TIME_WAIT)
- `bench/src/scenarios/tigerbeetle_scenario.rs`: real TigerBeetle scenario (env-gated)
- `bench/benches/ring_buffer.rs` and `bench/benches/latency.rs`: Criterion micro-benchmarks
- `InMemoryLedgerClient::new_unbounded()`: bench-only mode that skips balance validation
- Measured numbers on Apple Silicon: pipeline 19.6M TPS (latency-tracked), TCP 40K TPS
- Updated `scripts/bench.sh` to run both scenario suite and Criterion
- README Benchmarks section with full results table

### Added — Prompts #1–#6 (2026-03-09)
- Initial monorepo skeleton with Rust core workspace and Go services
- Core Rust crates: `blazil-engine`, `blazil-transport`, `blazil-ledger`, `blazil-risk`, `blazil-common`
- Go microservices: gateway, payments, banking, trading, crypto, compliance
- Docker Compose development stack (TigerBeetle, Redpanda, Vault, Keycloak, OTel, Prometheus, Grafana)
- GitHub Actions CI pipeline (Rust + Go)
- Disruptor ring-buffer pipeline with `ValidationHandler`, `RiskHandler`, `LedgerHandler`, `PublishHandler`
- TCP transport layer with MessagePack framing, backpressure guard, `MockTransportClient`
- TigerBeetle real client (`tigerbeetle-unofficial`), feature-gated, with currency round-trip via `user_data_32`
- `BlazerError::RingBufferFull { retry_after_ms }` with gating-sequence backpressure
- Integration tests for TigerBeetle (env-gated, skip on macOS `io_uring` unavailable)
- E2E smoke tests: pipeline → TigerBeetle
- ADR 0001: TigerBeetle as ledger
- ADR 0003: ring-buffer claim/cursor separation
- Benchmark regression checks workflow
- Security scanning workflow (Trivy + cargo audit)
- Setup, benchmark, and quality check scripts

---

<!-- 
Format for entries:

## [X.Y.Z] - YYYY-MM-DD

### Added
- New feature description

### Changed
- Changed feature description

### Deprecated
- Deprecated feature description

### Removed
- Removed feature description

### Fixed
- Bug fix description

### Security
- Security fix description
-->
