# Architecture: Blazil System Design

**Version:** 0.1 (March 2026)  
**Status:** Current

---

## Overview

Blazil is a multi-language monorepo. The core transaction path is written in Rust; domain services are written in Go. The two worlds communicate exclusively over local gRPC with bidirectional streaming.

```
  ┌──────────────────────────────────────────────────────────┐
  │                  External Clients                        │
  │          gRPC (TLS) / REST (HTTPS)                       │
  └──────────────────────┬───────────────────────────────────┘
                         │
  ┌──────────────────────▼───────────────────────────────────┐
  │                  Go Services                             │
  │   payments · banking · trading · crypto · compliance     │
  │                                                          │
  │   Each service:                                          │
  │     • Validates input (schema, auth, rate limit)         │
  │     • Converts to internal domain types                  │
  │     • Streams to Rust engine via gRPC                    │
  └──────────────────────┬───────────────────────────────────┘
                         │  gRPC bidirectional stream
                         │  (ProcessPaymentStream / etc.)
  ┌──────────────────────▼───────────────────────────────────┐
  │               Rust Core Engine                           │
  │                                                          │
  │   Transport (io_uring)                                   │
  │     └─▶ Ring buffer (LMAX Disruptor, lock-free)          │
  │           └─▶ Risk checks (pre-commit)                   │
  │                 └─▶ Batch accumulator (≤100 transfers)   │
  │                       └─▶ TigerBeetle client             │
  └──────────────────────┬───────────────────────────────────┘
                         │  VSR consensus
  ┌──────────────────────▼───────────────────────────────────┐
  │           TigerBeetle VSR Cluster (3 replicas)           │
  │                                                          │
  │   replica-0 (node-1)  replica-1 (node-2)                │
  │   replica-2 (node-3)                                     │
  │                                                          │
  │   • Strict consistency (VSR protocol)                   │
  │   • io_uring storage path                                │
  │   • Double-entry accounting enforced at ledger level     │
  └──────────────────────────────────────────────────────────┘
```

---

## Monorepo layout

```
BLAZIL/
├── core/                    # Rust workspace
│   ├── engine/              # LMAX Disruptor pipeline
│   ├── transport/           # io_uring ingestion + metrics server
│   ├── ledger/              # TigerBeetle client + double-entry types
│   ├── risk/                # Pre-commit risk rules
│   └── common/              # Shared types, errors, traits
│
├── services/                # Go workspace
│   ├── payments/            # ISO 20022, ACH, SEPA
│   ├── banking/             # Accounts, deposits, withdrawals
│   ├── trading/             # OMS, FIX, clearing
│   └── crypto/              # Digital assets, chain abstraction
│
├── bench/                   # Rust benchmark harness
├── tools/stresstest/        # Go gRPC streaming stresstest
├── libs/                    # Go shared libraries
│   ├── auth/                # JWT validation, interceptors
│   ├── discovery/           # Service registry
│   ├── observability/       # OTel tracing + logging
│   ├── policy/              # OPA evaluator
│   ├── secrets/             # Vault client
│   └── sharding/            # Consistent hash, cross-shard
│
├── infra/
│   ├── docker/              # Docker Compose configs (dev, test, cluster)
│   ├── k8s/                 # Kubernetes base + overlays
│   ├── terraform/           # DigitalOcean provisioning
│   └── policies/            # OPA (Rego) policy files
│
├── observability/           # Prometheus rules, Grafana dashboards
├── docs/                    # Architecture docs, ADRs, runbooks
└── scripts/                 # Setup, bench, cluster, deploy
```

---

## Critical path: one transaction

```
1.  Client sends ProcessPaymentStream RPC (gRPC bidirectional stream)
2.  Payments service: auth check → schema validation → forward to engine
3.  Transport layer: recv via io_uring, enqueue onto Disruptor ring buffer
4.  Engine pipeline thread (pinned to core 0):
      a. Dequeue event from ring buffer
      b. Pre-commit risk check (limit, velocity, sanctions)
      c. Accumulate into batch (flush at 100 transfers OR 1 ms max age)
5.  TigerBeetle client submits batch:
      a. VSR proposal → consensus across 3 replicas (~1.6 ms)
      b. Batch committed atomically (all-or-nothing)
6.  Engine writes results back to ring buffer output slots
7.  Transport sends gRPC response back to payments service
8.  Payments service streams response to client

Total end-to-end P99: 23–31 ms on DigitalOcean VPC
```

---

## Key design decisions

### ADR-001: Monorepo
All components in one repo for atomic commits, shared CI, and cross-language refactoring. See [adr/](../adr/).

### ADR-002: TigerBeetle as ledger
TigerBeetle enforces double-entry accounting at the storage layer. No application-level accounting bugs are possible — the ledger rejects any transaction that would violate balance invariants. See [adr/0001-use-tigerbeetle-as-ledger.md](../adr/0001-use-tigerbeetle-as-ledger.md).

### ADR-003: LMAX Disruptor ring buffer
The engine pipeline uses a single-producer, single-consumer ring buffer (Disruptor pattern). Claim and cursor are pointer-separated to avoid false sharing. 12.5M ops/s on a single core with 84 ns P99. See [adr/0003-ringbuffer-claim-cursor-separation.md](../adr/0003-ringbuffer-claim-cursor-separation.md).

### Streaming over unary RPC
gRPC unary RPC has a hard ceiling of ~200 TPS due to stop-and-wait round trips. Bidirectional streaming with a 256-slot in-flight window achieves 62,770 TPS — a 314× improvement — by decoupling send from receive.

### Batch commits, not individual transfers
TigerBeetle VSR consensus costs ~1.6 ms regardless of batch size. Batching 100 transfers per round amortises this cost 100×. Individual transfer commits would cap throughput at ~625 TPS; batching raises the ceiling to 62K+ TPS.

---

## Observability

All metrics are exported by the Rust transport layer via a Prometheus HTTP endpoint (`:9090`). Grafana dashboards are provisioned automatically on startup.

| Metric | Type | Description |
|--------|------|-------------|
| `blazil_pipeline_events_published_total` | Counter | Events received by the engine |
| `blazil_pipeline_events_committed_total` | Counter | Events committed to TigerBeetle |
| `blazil_pipeline_events_rejected_total` | Counter | Events rejected (risk / ledger error) |
| `blazil_pipeline_avg_latency_ns` | Gauge | Rolling average end-to-end latency |
| `blazil_pipeline_p99_ns` | Gauge | Rolling P99 latency |
| `blazil_ring_buffer_utilization_ratio` | Gauge | Ring buffer fill level (0–1) |
| `blazil_payments_concurrency_limit_reached_total` | Counter | Payments backpressure events |
| `blazil_cross_shard_total` | Counter | Cross-shard transaction routing events |

---

## Deployment topology (production)

```
  node-1 (159.223.85.45)         node-2                  node-3
  ┌─────────────────────┐  ┌───────────────────┐  ┌───────────────────┐
  │ tigerbeetle-0 :3000 │  │ tigerbeetle-1:3001│  │ tigerbeetle-2:3002│
  │ blazil-engine :7878 │  │ blazil-engine:7878│  │ blazil-engine:7878│
  │ blazil-payments:50051│  │ payments  :50051 │  │ payments  :50051 │
  │ prometheus    :9090 │  │                   │  │                   │
  │ grafana       :3001 │  │                   │  │                   │
  └─────────────────────┘  └───────────────────┘  └───────────────────┘
        ▲
        │  All nodes connected via VPC private network
```

Prometheus on node-1 scrapes:
- `blazil-engine:9090` (local, Docker bridge)
- `<node-2-ip>:9096` and `<node-3-ip>:9097` via `file_sd_configs`

---

## Scaling path

| Instance | Est. TPS | Monthly cost |
|----------|----------|--------------|
| 3× c2-4vcpu-8GB (current) | 62,770 | $252 |
| 3× c2-8vcpu-16GB | ~120,000 | $480 |
| 3× c2-16vcpu-32GB | ~250,000 | $960 |
| 9× c2-8vcpu-16GB (3 shards) | ~360,000 | $1,440 |

Bottleneck at current scale: network bandwidth (~180 Mbps). Next limit: 1 Gbps NIC → ~350K TPS theoretical.

- Terraform for cloud provisioning
- Ansible for bare metal

### Observability
Centralized monitoring configuration in `observability/`:
- Grafana dashboards
- Prometheus scrape configs and alerts
- OpenTelemetry collector configuration

## Consequences

### Positive
- **Atomic changes**: Changes spanning Rust core and Go services can be committed together
- **Simplified CI**: Single CI pipeline can test all components together
- **Shared tooling**: Scripts, linters, formatters can be centralized
- **Version synchronization**: No need to coordinate versions across multiple repos
- **Easy refactoring**: Moving code between modules doesn't require repo migrations
- **Consistent development environment**: One setup script works for everything

### Negative
- **Repository size**: Will grow larger over time (mitigated by Git's efficiency)
- **CI complexity**: Need to detect changed components and run targeted tests
- **Access control**: Can't easily restrict access to specific components (acceptable for open source)
- **Clone time**: Initial clone is larger (one-time cost)

### Neutral
- **Build system complexity**: Each language ecosystem (Rust/Go) maintains its own workspace
- **Deployment**: Services still deploy independently despite being in the same repo

## Alternatives Considered

### Multi-repo (Polyrepo)
One repository per service/component. Rejected because:
- Coordination overhead for cross-component changes
- Complex version management
- Difficult to maintain consistency
- Harder to onboard new contributors

### Hybrid Approach
Core in one repo, services in another. Rejected because:
- Still requires coordination for changes spanning both
- Loses benefits of true monorepo
- Added complexity with minimal benefit
