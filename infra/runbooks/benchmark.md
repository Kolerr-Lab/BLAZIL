# Benchmark Runbook

## Overview

This runbook covers running the Blazil benchmark suite against a live cluster to
validate performance before and after changes.

### Baseline Numbers

| Environment | Transport | TPS | P99 Latency |
|---|---|---|---|
| v0.1 (M2 MacBook) | gRPC | 62,770 | ~15ms |
| v0.2 (M4 MacBook, local) | Aeron IPC | 1,049,102 | <1ms |
| DO cluster (3× s-8vcpu-16gb) | io-uring | 2–3M (projected) | TBD |
| DO cluster (3× c-8) | io-uring | 3–5M (projected) | TBD |

---

## Quick Benchmark (single node target)

```bash
# From your local machine — requires stresstest binary
./tools/stresstest/stresstest-linux \
  -target=$NODE1_PUBLIC_IP:50051 \
  -duration=120s \
  -goroutines=64 \
  -window=2048
```

Or build the stresstest locally:
```bash
cd tools/stresstest && go build -o stresstest-local .
./stresstest-local -target=$NODE1_PUBLIC_IP:50051 -duration=120s
```

---

## Full Cluster Benchmark (via Ansible)

```bash
cd infra/ansible

# Run benchmark against the cluster (5-min warm-up + 5-min measurement)
ansible-playbook -i inventory/production playbooks/benchmark.yml \
  -e duration=300 \
  -e warmup=300 \
  -e goroutines=128

# Results saved to docs/benchmark-results/do-v0.2-YYYYMMDD.txt
```

### Benchmark playbook variables

| Variable | Default | Description |
|---|---|---|
| `duration` | 120 | Measurement window in seconds |
| `warmup` | 60 | Warmup period before measuring |
| `goroutines` | 64 | Concurrent gRPC clients |
| `window` | 2048 | Ring buffer drain batch size |

---

## Local Aeron IPC Benchmark

```bash
# Runs on macOS/Linux, requires no cluster
./scripts/aeron-bench.sh

# Or directly:
RUST_LOG=error cargo bench --bench sharded_pipeline_scenario -- \
  --profile-time=30
```

Expected output bands:
- Laptop (M4): 900K–1.05M TPS
- Laptop (M2): 600K–850K TPS
- Linux (c-8 DO): 1.5M–3M TPS

---

## Interpreting Results

```
Throughput: 1,045,320 TPS
P50:         0.45μs
P99:         2.1μs
P99.9:      18.3μs
Errors:      0
```

- **TPS** — primary metric. Should be stable within ±5% across 5 runs.
- **P99** — must be < 5ms for SLA compliance (payment settlement).
- **P99.9** — spikes here are OK (GC/OS jitter), watch for consistent outliers.
- **Errors** — must be zero. Any non-zero count indicates a correctness bug.

---

## Benchmark Before/After Protocol

Before merging a performance-relevant PR:

```bash
# 1. Baseline on main
git checkout main
./scripts/aeron-bench.sh 2>&1 | tee /tmp/baseline.txt

# 2. Run feature branch
git checkout feature-branch
./scripts/aeron-bench.sh 2>&1 | tee /tmp/feature.txt

# 3. Diff
diff /tmp/baseline.txt /tmp/feature.txt
```

---

## Stress Test (correctness + performance)

```bash
# Run the Rust stress test suite (checks correctness under load)
./scripts/stress.sh

# Run the Go stresstest with correctness checks enabled
./tools/stresstest/stresstest-linux \
  -target=$NODE1_PUBLIC_IP:50051 \
  -duration=300s \
  -goroutines=128 \
  -verify-balances=true  # checks TigerBeetle balance invariants
```

---

## Recording Results

After a significant benchmark run, add results to `docs/benchmark-report.md`:

```bash
# Auto-append benchmark output to report
echo "## $(date +%Y-%m-%d) DO Cluster v0.2 Benchmark" >> docs/benchmark-report.md
cat docs/benchmark-results/do-v0.2-$(date +%Y%m%d).txt >> docs/benchmark-report.md
git add docs/ && git commit -m "bench: DO cluster v0.2 results"
```

Screenshot the Grafana dashboard at peak TPS and save to
`docs/benchmark-screenshots/` for the CHANGELOG.
