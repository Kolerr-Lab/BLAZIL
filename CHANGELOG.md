# Changelog

All notable changes to Blazil will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Updated — Phase 10: Performance Breakthrough (2026-03-19)
- Sharded pipeline 4-shard bulk timing: **200,000,000 TPS** (was 167M TPS)
- Sharded pipeline 1-shard bulk timing: **111,111,111 TPS** (was 77M TPS)
- Pipeline latency-tracked: **24,390,243 TPS**, P99 42ns, P99.9 83ns
- UDP E2E: **135,135 TPS** (window-based pipelining, concurrent tasks, semaphore backpressure)
- TCP E2E: **38,610 TPS** baseline
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
