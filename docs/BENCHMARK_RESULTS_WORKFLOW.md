# AI Benchmark Results - Storage & Workflow

## 📁 Storage Structure

```
docs/
├── benchmark-screenshots/       # Raw benchmark logs
│   └── ai-bench-<timestamp>.log
│
└── runs/                        # Markdown reports (for Git)
    └── <timestamp>_ai-inference-aws-i4i4xl.md
```

**Strategy:**
- **Raw logs:** `docs/benchmark-screenshots/` (committed, full output for reproducibility)
- **Reports:** `docs/runs/` (committed, human-readable summary with analysis)
- **Format:** Both committed to Git for full audit trail

---

## 🔄 Workflow (After Benchmark Completes)

### Option 1: Automated (Recommended)

```bash
# Run this after benchmark completes on AWS instance
./scripts/commit-bench-results.sh docs/benchmark-screenshots/ai-bench-2026-05-12_18-30-00.log

# This will:
# 1. Generate markdown report
# 2. Stage both log + report
# 3. Create descriptive commit message
# 4. Optionally push to origin/main
```

**Interactive prompts:**
- Confirm commit? [y/N]
- Push to remote? [y/N]

---

### Option 2: Manual

```bash
# Step 1: Generate report
./scripts/gen-ai-report.sh docs/benchmark-screenshots/ai-bench-2026-05-12_18-30-00.log

# Step 2: Review report
cat docs/runs/2026-05-12_18-30-00_ai-inference-aws-i4i4xl.md

# Step 3: Stage files
git add docs/benchmark-screenshots/ai-bench-2026-05-12_18-30-00.log
git add docs/runs/2026-05-12_18-30-00_ai-inference-aws-i4i4xl.md

# Step 4: Commit with metrics
git commit -m "results(ai): AWS i4i.4xlarge benchmark (2026-05-12_18-30-00)

Dataloader: 5,234,567 samples/sec
SqueezeNet 1.1: 2,156 inferences/sec
ResNet-50: 487 inferences/sec
SLA compliance: PASS

Hardware: AWS i4i.4xlarge (16 vCPU, 128 GB RAM, NVMe)
Duration: 35 minutes (3 phases)
Commit: $(git rev-parse --short HEAD)

Files:
- Raw log: ai-bench-2026-05-12_18-30-00.log
- Report: 2026-05-12_18-30-00_ai-inference-aws-i4i4xl.md"

# Step 5: Push
git push origin main
```

---

## 📊 Report Format (Auto-Generated)

The markdown report includes:

### Sections
1. **Results** - Primary metrics table
2. **Health & SLA** - Success rate, error rate, uptime
3. **Test Procedure** - 3-phase configuration details
4. **Configuration** - Hardware + software stack
5. **Analysis** - Comparison with targets & baselines
6. **Observations** - Strengths, bottlenecks, future optimizations
7. **Conclusion** - Production readiness assessment

### Metrics Extracted
- Dataloader: throughput (samples/sec), P50/P99 latency
- SqueezeNet: RPS, P50/P99/P999 latency
- ResNet-50: RPS, P50/P99/P999 latency
- SLA: compliance status, success rate, error rate

### Comparisons
- **vs Targets:** docs/AI_BASELINES.md (1,600-2,400 SqueezeNet)
- **vs Fintech:** Blazil proven 233K TPS comparison
- **vs Baselines:** PyTorch, TensorFlow Serving, ONNX Runtime

---

## 🎯 Example Commit History

```
60352b3 docs(ai): Remove DigitalOcean references, focus AWS i4i.4xlarge only
a1b2c3d results(ai): AWS i4i.4xlarge benchmark (2026-05-12_18-30-00)
        Dataloader: 5,234,567 samples/sec
        SqueezeNet 1.1: 2,156 inferences/sec
        ResNet-50: 487 inferences/sec
        SLA compliance: PASS
```

**Benefit:** Git history = audit trail for all benchmark runs.

---

## 📝 Naming Convention

### Raw Logs
```
docs/benchmark-screenshots/ai-bench-<YYYY-MM-DD_HH-MM-SS>.log
```

### Reports
```
docs/runs/<YYYY-MM-DD_HH-MM-SS>_ai-inference-aws-i4i4xl.md
```

**Rationale:**
- ISO 8601 timestamp prefix = sortable
- Suffix describes hardware = distinguishes multiple runs
- Consistent with fintech reports in `docs/runs/`

---

## 🔍 Querying Results

### Find all AI benchmarks
```bash
ls -lh docs/runs/*ai-inference*.md
```

### Compare throughput over time
```bash
grep -h "SqueezeNet.*Throughput" docs/runs/*ai-inference*.md
```

### Find SLA compliance issues
```bash
grep -l "SLA Compliance.*FAIL" docs/runs/*ai-inference*.md
```

### Extract all latency P99s
```bash
grep -h "P99" docs/runs/*ai-inference*.md | sort
```

---

## ⚙️ Integration with CI/CD

### GitHub Actions (Future)
```yaml
name: AI Benchmark

on:
  schedule:
    - cron: '0 0 * * 0'  # Weekly on Sunday
  workflow_dispatch:

jobs:
  benchmark:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - name: Launch AWS instance
        run: ./scripts/launch-aws-bench.sh
      - name: Run benchmark
        run: ./scripts/ai-benchmark.sh
      - name: Commit results
        run: ./scripts/commit-bench-results.sh docs/benchmark-screenshots/ai-bench-*.log
      - name: Push to main
        run: git push origin main
```

---

## 🎓 Best Practices

### DO commit:
- ✅ Raw logs (full output for reproducibility)
- ✅ Markdown reports (human-readable analysis)
- ✅ System info (CPU, memory, disk from logs)
- ✅ Git commit SHA (for code version traceability)

### DON'T commit:
- ❌ Binary artifacts (models already in separate repo)
- ❌ Temporary files (.tmp, .swp)
- ❌ Credentials or API keys (should never be in logs)
- ❌ Large datasets (synthetic = generated on-the-fly)

### Commit Message Format:
```
results(ai): <hardware> benchmark (<timestamp>)

<key metrics>

Hardware: <instance type>
Duration: <minutes>
Commit: <sha>

Files:
- Raw log: <filename>
- Report: <filename>
```

---

## 🚀 Quick Reference

**After benchmark completes on AWS:**
```bash
# 1. Copy log from AWS to local
scp ubuntu@$INSTANCE_IP:/opt/blazil/docs/benchmark-screenshots/ai-bench-*.log docs/benchmark-screenshots/

# 2. Run automated workflow
./scripts/commit-bench-results.sh docs/benchmark-screenshots/ai-bench-*.log

# 3. Respond to prompts
#    Commit? [y/N] → y
#    Push? [y/N] → y

# Done! Results published to GitHub.
```

---

## 📊 Versioning Strategy

**Benchmark results are immutable historical records:**
- Each run = separate file (timestamped)
- Never edit past results (append new runs instead)
- Git history = full audit trail
- Can compare across commits to track regressions/improvements

**Example timeline:**
```
v0.3.0 (May 2026):  2,156 SqueezeNet RPS
v0.3.1 (Jun 2026):  2,340 SqueezeNet RPS (+8% optimization)
v0.4.0 (Jul 2026):  8,234 SqueezeNet RPS (5x breakthrough!)
```

Each version has corresponding benchmark in `docs/runs/`.

---

## 🔐 Security Considerations

**Safe to commit:**
- Performance metrics (public benchmarks)
- System specs (AWS instance types are public info)
- Logs with sanitized paths (no internal IPs)

**Redact before commit:**
- AWS account IDs
- Private IP addresses
- API keys or credentials
- Internal hostnames

The generated scripts automatically use generic paths and sanitize sensitive info.

---

**Questions?** See docs/ML_BENCH_RESILIENCE_IMPLEMENTATION.md for technical details.
