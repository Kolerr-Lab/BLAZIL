# SOC 2 Type II — Compliance Evidence Package

**Organisation:** Blazil Financial Inc. (Kolerr Lab)
**Generated:** 2026-05-10T04:22:16Z
**Commit:** [`a2cc672`](https://github.com/Kolerr-Lab/BLAZIL/commit/a2cc672fad499d8d208e36e307485d63ab094554) on `main`
**Evidence collection method:** Automated — `scripts/collect-evidence.sh`

---

## Executive Summary

| Check | Status |
|-------|--------|
| Build (Rust 1.88.0) | ✅ PASS |
| Format (`rustfmt`) | ✅ PASS |
| Lint (`clippy -D warnings`) | ✅ PASS |
| Test suite | ✅ PASS |
| Dependency audit (`cargo audit`) | ✅ PASS |
| Container scan (Trivy) | ✅ PASS |
| CI pipeline | ℹ️ N/A |

---

## CC6 — Logical and Physical Access Controls

### CC6.1 — Access restriction via policy

Access is enforced through role-based controls defined in `libs/auth/`
(0 Rust source files) and infrastructure policy files in
`infra/policies/` (1 policy documents).

See: [docs/compliance/access-control-framework.md](access-control-framework.md)

### CC6.2 — Cryptographic protections

All data in transit uses TLS 1.3. Encryption at rest is documented in
[docs/compliance/encryption-at-rest-and-in-transit.md](encryption-at-rest-and-in-transit.md).

---

## CC7 — System Operations

### CC7.1 — Change detection and audit logging

Every transaction produces an immutable audit trail via `blazil-audit`
(32 unit tests). Audit log records carry SHA-256 integrity hashes.
Retention: 5 years per MAS Notice 626 §6.

See: [docs/compliance/PHASE5_AUDIT_LOGGING.md](PHASE5_AUDIT_LOGGING.md)

### CC7.2 — System monitoring

OpenTelemetry tracing and Prometheus metrics are exported from all services.
Grafana dashboards: `observability/grafana/`.

### CC7.3 — Vulnerability management

**cargo audit result:** ✅ PASS — 0 known CVEs in dependency tree

```
    Fetching advisory database from `https://github.com/RustSec/advisory-db.git`
      Loaded 1068 security advisories (from /Users/rickyanhnguyen/.cargo/advisory-db)
    Updating crates.io index
    Scanning Cargo.lock for vulnerabilities (454 crate dependencies)
Crate:     paste
Version:   1.0.15
Warning:   unmaintained
Title:     paste - no longer maintained
Date:      2024-10-07
ID:        RUSTSEC-2024-0436
URL:       https://rustsec.org/advisories/RUSTSEC-2024-0436
Dependency tree:
paste 1.0.15
├── tract-linalg 0.21.10
│   └── tract-core 0.21.10
│       ├── tract-nnef 0.21.10
│       │   ├── tract-onnx-opl 0.21.10
│       │   │   └── tract-onnx 0.21.10
│       │   │       └── blazil-inference 0.1.0
│       │   │           ├── ml-bench 0.1.0
│       │   │           └── blazil-inference-service 0.1.0
│       │   └── tract-onnx 0.21.10
│       ├── tract-hir 0.21.10
│       │   └── tract-onnx 0.21.10
│       └── blazil-inference 0.1.0
└── tract-core 0.21.10

Crate:     rand
Version:   0.8.5
Warning:   unsound
Title:     Rand is unsound with a custom logger using `rand::rng()`
Date:      2026-04-09
ID:        RUSTSEC-2026-0097
URL:       https://rustsec.org/advisories/RUSTSEC-2026-0097
Dependency tree:
rand 0.8.5
├── tungstenite 0.21.0
│   └── tokio-tungstenite 0.21.0
│       └── warp 0.3.7
│           └── blazil-inference-service 0.1.0
├── tract-onnx-opl 0.21.10
│   └── tract-onnx 0.21.10
│       └── blazil-inference 0.1.0
│           ├── ml-bench 0.1.0
│           └── blazil-inference-service 0.1.0
├── rust_decimal 1.41.0
│   ├── blazil-transport 0.1.0
│   │   ├── blazil-inference-service 0.1.0
│   │   └── blazil-bench 0.1.0
│   ├── blazil-risk 0.1.0
│   ├── blazil-ledger 0.1.0
│   │   ├── blazil-transport 0.1.0
│   │   ├── blazil-engine 0.1.0
│   │   │   ├── blazil-transport 0.1.0
│   │   │   └── blazil-bench 0.1.0
│   │   └── blazil-bench 0.1.0
│   ├── blazil-engine 0.1.0
│   ├── blazil-common 0.1.0
│   │   ├── blazil-transport 0.1.0
│   │   ├── blazil-ledger 0.1.0
│   │   ├── blazil-inference-service 0.1.0
│   │   ├── blazil-inference 0.1.0
│   │   ├── blazil-engine 0.1.0
│   │   ├── blazil-dataloader 0.1.0
│   │   │   ├── ml-bench 0.1.0
│   │   │   ├── blazil-inference-service 0.1.0
│   │   │   └── blazil-inference 0.1.0
│   │   └── blazil-bench 0.1.0
│   └── blazil-bench 0.1.0
├── rand_distr 0.4.3
│   └── tract-onnx-opl 0.21.10
└── blazil-dataloader 0.1.0

warning: 2 allowed warnings found
```

**Trivy filesystem scan:** ✅ PASS — CRITICAL: 0, HIGH: 0, MEDIUM: 0

```
2026-05-10T11:23:08+07:00	INFO	[vuln] Vulnerability scanning is enabled
2026-05-10T11:23:29+07:00	INFO	Suppressing dependencies for development and testing. To display them, try the '--include-dev-deps' flag.
2026-05-10T11:23:29+07:00	INFO	Number of language-specific files	num=15
2026-05-10T11:23:29+07:00	INFO	[cargo] Detecting vulnerabilities...
2026-05-10T11:23:29+07:00	INFO	[gomod] Detecting vulnerabilities...
2026-05-10T11:23:29+07:00	INFO	[npm] Detecting vulnerabilities...

Report Summary

┌─────────────────────────────────────────┬───────┬─────────────────┐
│                 Target                  │ Type  │ Vulnerabilities │
├─────────────────────────────────────────┼───────┼─────────────────┤
│ Cargo.lock                              │ cargo │        1        │
├─────────────────────────────────────────┼───────┼─────────────────┤
│ libs/auth/go.mod                        │ gomod │        0        │
├─────────────────────────────────────────┼───────┼─────────────────┤
│ libs/discovery/go.mod                   │ gomod │        0        │
├─────────────────────────────────────────┼───────┼─────────────────┤
│ libs/observability/go.mod               │ gomod │        0        │
├─────────────────────────────────────────┼───────┼─────────────────┤
│ libs/policy/go.mod                      │ gomod │        0        │
├─────────────────────────────────────────┼───────┼─────────────────┤
│ libs/secrets/go.mod                     │ gomod │        0        │
├─────────────────────────────────────────┼───────┼─────────────────┤
│ libs/sharding/go.mod                    │ gomod │        0        │
├─────────────────────────────────────────┼───────┼─────────────────┤
│ services/banking/go.mod                 │ gomod │        0        │
├─────────────────────────────────────────┼───────┼─────────────────┤
│ services/crypto/go.mod                  │ gomod │        0        │
├─────────────────────────────────────────┼───────┼─────────────────┤
│ services/payments/go.mod                │ gomod │        0        │
├─────────────────────────────────────────┼───────┼─────────────────┤
│ services/trading/go.mod                 │ gomod │        0        │
├─────────────────────────────────────────┼───────┼─────────────────┤
│ tools/ai-dashboard/package-lock.json    │  npm  │        0        │
├─────────────────────────────────────────┼───────┼─────────────────┤
│ tools/bench-dashboard/package-lock.json │  npm  │        0        │
├─────────────────────────────────────────┼───────┼─────────────────┤
│ tools/loadgen/go.mod                    │ gomod │        0        │
├─────────────────────────────────────────┼───────┼─────────────────┤
│ tools/stresstest/go.mod                 │ gomod │        0        │
└─────────────────────────────────────────┴───────┴─────────────────┘
Legend:
- '-': Not scanned
- '0': Clean (no security findings detected)


Cargo.lock (cargo)
==================
Total: 1 (UNKNOWN: 0, LOW: 1, MEDIUM: 0, HIGH: 0, CRITICAL: 0)

┌─────────┬─────────────────────┬──────────┬────────┬───────────────────┬──────────────────────┬────────────────────────────────────────────────────────┐
│ Library │    Vulnerability    │ Severity │ Status │ Installed Version │    Fixed Version     │                         Title                          │
├─────────┼─────────────────────┼──────────┼────────┼───────────────────┼──────────────────────┼────────────────────────────────────────────────────────┤
│ rand    │ GHSA-cq8v-f236-94qc │ LOW      │ fixed  │ 0.8.5             │ 0.9.3, 0.10.1, 0.8.6 │ Rand is unsound with a custom logger using rand::rng() │
│         │                     │          │        │                   │                      │ https://github.com/advisories/GHSA-cq8v-f236-94qc      │
└─────────┴─────────────────────┴──────────┴────────┴───────────────────┴──────────────────────┴────────────────────────────────────────────────────────┘

📣 [34mNotices:[0m
  - Version 0.70.0 of Trivy is now available, current version is 0.69.3

To suppress version checks, run Trivy scans with the --skip-version-check flag
```

---

## CC8 — Change Management

### CC8.1 — Software development lifecycle

| Metric | Value |
|--------|-------|
| Repository | [Kolerr-Lab/BLAZIL](https://github.com/Kolerr-Lab/BLAZIL) |
| Branch | `main` |
| HEAD commit | `a2cc672fad499d8d208e36e307485d63ab094554` |
| Commit date | 2026-05-10 |
| Total commits | 327 |
| Contributors | 1 |

All changes require:
1. Feature branch + pull request
2. CI pipeline green (`fmt` → `clippy -D warnings` → `test --workspace`)
3. Peer code review before merge

**Last CI run:** N/A — ℹ️ N/A → [https://github.com/Kolerr-Lab/BLAZIL/actions](https://github.com/Kolerr-Lab/BLAZIL/actions)

### CC8.2 — Code quality gates

| Gate | Command | Result |
|------|---------|--------|
| Format | `cargo +1.88.0 fmt --all -- --check` | ✅ PASS |
| Linting | `cargo +1.88.0 clippy -D warnings` | ✅ PASS |
| Tests | `cargo +1.88.0 test --workspace` | ✅ PASS — 582 passing |
| Dependency CVEs | `cargo audit` | ✅ PASS — 0 CVEs |

---

## CC9 — Risk Mitigation

### CC9.1 — AML / KYC controls

`blazil-screening` (39 unit tests) implements:
- Real-time screening with 50 ms deadline (fail-open)
- Batch worker queue with back-pressure
- Rule-based MockScreener; Sardine / Chainalysis / Elliptic provider stubs
- SAR generation (FinCEN SAR XML v2.0)
- InMemoryHoldStore for transaction holds pending review

### CC9.2 — Data governance

Data residency enforced by `blazil-mas-trm` (54 unit tests).
Singapore personal/financial data is denied egress to non-SG regions (fail-closed).

---

## A1 — Availability

### A1.1 — Business continuity targets

BCP targets per MAS TRM Chapter 7 (from `libs/mas_trm/src/bcp.rs`):

| Criticality | Max RTO | Max RPO |
|-------------|---------|---------|
| Critical | 4 h | 4 h |
| High | 8 h | 8 h |
| Medium | 24 h | 24 h |
| Low | 72 h | 72 h |

Compliance verification: `BcpAssessment::all_compliant()` + `non_compliant()`

---

## Codebase Metrics

| Metric | Value |
|--------|-------|
| Rust source files | 150 |
| Lines of Rust code | 36812 |
| Workspace crates | 14 |
| Shell scripts | 22 |
| Kubernetes manifests | 22 |
| Test cases (workspace) | 582 passing |

---

*Generated by `scripts/collect-evidence.sh` — do not edit manually.*
