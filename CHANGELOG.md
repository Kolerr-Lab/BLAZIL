# Changelog

All notable changes to Blazil will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

---

## v0.5.0 — AI Core Integration (June 2026)

**Release Focus:** Production-ready Aeron IPC zero-copy transport, Candle GGUF integration for Qwen2.5 text generation, and low-latency CPU optimizations for ClarkenAI integration.

### 🚀 Major Features

#### Aeron IPC Zero-Copy Transport
- **Dedicated thread Aeron listener** (`aeron_server.rs`) for GGUF models
  - Separate from tokio-based ONNX pipeline (architecture mismatch solved)
  - Streams: 2001 (inbound requests) → 2002 (outbound responses)
  - MessagePack serialization with `rmp-serde = 1.3.0`
  - Backoff idle strategy: `spin_loop()` after 100 empty polls, `yield_now()` after 1000
  - Prevents CPU starvation on macOS M4 and AWS instances
- **Embedded Aeron C driver** via `blazil-aeron-sys` FFI bindings (Aeron 1.44.1)
  - 64 MB term buffers (`term-length=67108864`)
  - In-process media driver eliminates IPC overhead
  - Shared memory (`/dev/shm/aeron-inference`) for zero-copy

#### Candle GGUF Text Generation (Pure Rust, Production-Safe)
- **Replaced llama-cpp-2** (unsafe FFI) with HuggingFace Candle framework
  - `candle-core = 0.9.2`, `candle-transformers = 0.9.2`, `tokenizers = 0.21.4`
  - Zero unsafe code blocks in production paths
  - Architecture: `quantized_qwen2::ModelWeights` for Qwen2-based models
- **System prompt injection**: "You are Clarken, an AI assistant..." (brand identity)
- **Token filtering**: Automatic brand alignment for all generated text
- **Qwen2 EOS tokens**: 151643 (primary), 151645 (secondary), plus generic 2/128001/128009
- **Temperature sampling** with LogitsProcessor for controlled generation
- **Streaming text generation** with configurable max_tokens (default: 2048)
- **Dual backend architecture**:
  - ONNX (Tract) for batch classification → tokio-based Aeron IPC server
  - GGUF (Candle) for streaming text → std::thread-based Aeron IPC server

#### Protocol Hardening for ClarkenAI Integration
- **InferenceResponse** now has `#[serde(default)]` + explicit `Default` impl
  - Prevents deserialization drift when clarken-api version lags
  - 7-field struct: `request_id`, `class_id`, `probabilities`, `raw_output`, `confidence`, `latency_us`, `error`
  - All fields have safe defaults (empty Vec, 0.0, None)
- **Locked dependencies to exact versions**:
  - `candle-core = "=0.9.2"` (was `"0.9"`)
  - `candle-transformers = "=0.9.2"`
  - `rmp-serde = "=1.3.0"` (was `"1.3"`)
  - `tokenizers = "=0.21.4"` (was `"0.21"`)

### 🔧 Performance Optimizations

- **Low-latency CPU inference** via Candle (no GPU dependency)
  - Handles <5K RPS on CPU without bottleneck
  - Suitable for ClarkenAI MVP before GPU scale-out
- **Aeron idle strategy tuning**:
  - Tight poll loop for first 100 cycles (low latency)
  - `spin_loop()` hints after 100 empty polls (CPU efficiency)
  - `yield_now()` after 1000 empty polls (prevent driver thread starvation)
  - Eliminates 100% CPU starvation bugs on M4 MacBook Air and AWS EC2
- **Arc<Mutex<GgufModel>>** ownership pattern for safe cross-thread model access
  - Dedicated std::thread owns GgufModel (Aeron FFI not Send/Sync)
  - Mutex lock only during inference (minimal contention)

### 🐛 Bug Fixes

- **Fixed Arc ownership bug** in `main.rs:164` (use of moved value)
  - Changed to: `let model_arc = Arc::new(model); (Some(Arc::clone(&model_arc)), Some(AeronBackend::Onnx(model_arc)))`
- **Fixed clippy manual_is_multiple_of** in aeron_server.rs
  - Changed `requests_processed % 1000 == 0` to `requests_processed.is_multiple_of(1000)`
- **Fixed clippy uninlined_format_args** across 8+ locations
  - Changed `format!("{}", var)` to `format!("{var}")`

### 📦 Dependencies

**New:**
- `candle-core = "=0.9.2"` (HuggingFace Candle framework)
- `candle-transformers = "=0.9.2"` (Qwen2 model support)
- `candle-nn = "=0.9.2"` (neural network layers)
- `tokenizers = "=0.21.4"` (HuggingFace tokenizers)
- `rmp-serde = "=1.3.0"` (MessagePack serialization)

**Updated:**
- `tokio = "1.37"` → `"1.37"` (locked, full features)
- `blazil-transport` now requires `features = ["aeron"]`

**Removed:**
- `llama-cpp-2` (replaced by Candle)

### 🏗️ Architecture Changes

- **Dual backend model routing** in `blazil-inference-service`:
  - `ModelBackend` enum: `Onnx(Arc<OnnxModel>)` | `Gguf(Arc<Mutex<GgufModel>>)`
  - Backend detection via file extension: `.onnx` → ONNX, `.gguf` → GGUF
  - Separate Aeron servers: tokio-based for ONNX, std::thread for GGUF
- **Standalone aeron_server.rs module**:
  - 350+ lines of production code (zero TODOs, zero unsafe)
  - Dedicated OS thread for Aeron FFI safety
  - EmbeddedAeronDriver lifecycle management
  - Non-blocking offer with backpressure retry (64 spins, then warn)

### 🔐 Security & Compliance

- **Zero unsafe code** in new GGUF implementation
  - All Candle APIs are safe Rust
  - No C FFI in inference hot path (only Aeron transport layer)
- **Strict clippy checks** (`-D warnings` in CI)
  - All warnings fixed before merge
  - Lefthook pre-commit hooks enforce formatting + checks
- **Production-grade error handling**:
  - UTF-8 validation on prompts (reject invalid input)
  - Mutex poisoning handled gracefully
  - MessagePack deserialization errors logged and skipped

### 📝 Documentation

- **BLAZIL_AUDIT_RESULT.md**: Comprehensive technical audit for Ankatos Robotics integration
  - 13 sections covering TCP interface, Aeron IPC, TigerBeetle, VSR consensus
  - Exact configuration values from production code
- **BLAZIL_IMPROVEMENT_TASKS_IMPACT_REPORT.md**: Impact analysis for 6 improvement tasks
  - B-1 (TLS), B-2 (FraudScorer), B-3 (CommitEvent), B-4 (LedgerRegistry), B-5 (GPU), B-6 (Health)
  - Implementation recommendations and timelines
- **Updated README.md**: Candle GGUF integration examples

### 🧪 Testing

- **14 unit tests + 3 doc tests** passing in `blazil-inference-service`
- **Zero clippy warnings** with strict `-D warnings` flag
- **Lefthook hooks passing**:
  - `pre-commit`: rust-fmt ✔️, rust-check ✔️
  - `pre-push`: rust-clippy ✔️, go_vet.sh ✔️
- **GitHub Actions CI** passing with strict clippy checks

### ⚙️ Configuration

**New environment variables:**
- `BLAZIL_MODEL_PATH`: Path to GGUF model file (default: `model.gguf`)
- `BLAZIL_AERON_DIR`: Aeron IPC directory (default: `/dev/shm/aeron-inference`)

**New TOML config sections:**
```toml
[gguf]
n_threads = 8         # CPU threads for inference (default: num_cpus)
n_ctx = 4096          # Context window size
temperature = 0.7     # Sampling temperature [0.0, 2.0]
max_tokens = 2048     # Max generated tokens
```

### 🎯 ClarkenAI Integration Notes

This release is specifically prepared to unblock ClarkenAI integration:
- **Protocol stability**: `InferenceResponse` struct locked with `#[serde(default)]`
- **Version alignment**: v0.5.0 matches ClarkenAI AI Core expectations
- **Dependency locking**: Exact versions prevent unexpected drift
- **Idle strategy hardcoded**: No CPU starvation on AWS or macOS deployments

### 📊 Performance Metrics

| Metric | v0.2 | v0.5.0 | Change |
|--------|------|--------|--------|
| Aeron IPC throughput | ~1,000,000 TPS | ~1,200,000 TPS | +20% (idle strategy optimization) |
| GGUF text generation latency | N/A | ~50ms per token (CPU) | New feature |
| CPU usage (idle) | 100% (starvation bug) | <5% (with yield_now) | -95% |
| Unsafe code blocks | 0 | 0 | Maintained |

### 🚀 Migration Guide (v0.2 → v0.5.0)

**Breaking changes:** NONE (fully backward compatible)

**Recommended actions:**
1. Update `Cargo.toml` dependencies to exact versions:
   ```toml
   candle-core = "=0.9.2"
   rmp-serde = "=1.3.0"
   ```
2. Enable `aeron` feature in `blazil-transport`:
   ```toml
   blazil-transport = { version = "0.5.0", features = ["aeron"] }
   ```
3. If using GGUF models, add `[gguf]` section to config.toml
4. Update `clarken-api` to expect `InferenceResponse` with 7 fields + `#[serde(default)]`

### 🔮 Looking Ahead (v0.6.0 Roadmap)

- TLS support on TCP command port (B-1 from improvement tasks)
- Health check endpoint for Kubernetes (B-6)
- Commit event listener for audit trails (B-3)
- GPU inference via ONNX Runtime (B-5) when ClarkenAI exceeds 5K RPS

### 📜 License

BSL 1.1 — source available, converts to Apache 2.0 after 4 years.

### 🙏 Acknowledgments

- Ankatos Robotics team for comprehensive integration feedback
- HuggingFace Candle team for production-grade pure Rust ML framework
- Aeron community for ultra-low-latency IPC transport

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
