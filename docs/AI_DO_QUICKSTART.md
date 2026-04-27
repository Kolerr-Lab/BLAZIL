# Blazil AI Benchmark - DO Quick Start (April 29, 2026)

**Status:** ✅ READY  
**Commit:** `b3e57b1` (pushed)  
**Hardware:** DO Premium AMD NVMe (s-4vcpu-8gb-amd)  
**Cost:** $0.125/hour = $3 for 24 hours

---

## 🚀 Quick Commands (Copy-Paste Ready)

### 1. Provision DO Droplet (15 min)

**Via Dashboard:**
```
Region: Singapore (SGP1)
Plan: Premium AMD → s-4vcpu-8gb-amd
  - 4 vCPU @ 2.0 GHz
  - 8 GB RAM
  - 160 GB NVMe SSD
  - $84/month
OS: Ubuntu 22.04 LTS
Hostname: blazil-ai-node-1
```

**Get IP:**
```bash
# Droplet created, note IP address
DROPLET_IP=<YOUR_IP_HERE>
```

---

### 2. Setup Node (30 min)

**SSH + Run Setup:**
```bash
# SSH into droplet
ssh root@$DROPLET_IP

# Run automated setup (installs Rust, builds code, downloads models)
curl -fsSL https://raw.githubusercontent.com/Kolerr-Lab/BLAZIL/main/scripts/ai-do-setup.sh | bash -s ai-node-1

# Expected output:
#   - Rust 1.88.0 installed
#   - Blazil repo cloned
#   - inference + ml-bench built (10-15 min)
#   - SqueezeNet 1.1 downloaded (~5 MB)
#   - ResNet-50 downloaded (~100 MB)
#   - Synthetic dataset structure created

# Verify build
ls -lh /opt/blazil/target/release/ml-bench
ls -lh /opt/blazil/models/*.onnx
```

---

### 3. Run Benchmark (5-10 min)

**Option A: Quick Benchmark (Single Script)**
```bash
cd /opt/blazil
./run-ai-bench.sh

# Runs:
#   - Dataloader: 60 sec (256 batch, 4 workers)
#   - SqueezeNet: 120 sec (64 batch, 4 workers)
# Output: docs/benchmark-screenshots/ai-bench-<timestamp>.log
```

**Option B: Full Benchmark Suite**
```bash
cd /opt/blazil
./scripts/ai-benchmark.sh 1000000

# Runs:
#   - Dataloader: 60 sec
#   - SqueezeNet: 120 sec
#   - ResNet-50: 120 sec
# Output: docs/benchmark-screenshots/ai-bench-<timestamp>.log
```

---

### 4. Monitor (Optional)

**Terminal 1: System Resources**
```bash
htop
# Watch: CPU usage (should be 80-90% during inference)
```

**Terminal 2: Disk I/O**
```bash
iostat -x 1
# Watch: %util, await (latency)
```

**Terminal 3: Benchmark Output**
```bash
tail -f /opt/blazil/docs/benchmark-screenshots/ai-bench-*.log
```

---

### 5. Collect Results (5 min)

**Copy log to local machine:**
```bash
# From local machine
scp root@$DROPLET_IP:/opt/blazil/docs/benchmark-screenshots/ai-bench-*.log ~/blazil-results/

# Or view directly
ssh root@$DROPLET_IP 'cat /opt/blazil/docs/benchmark-screenshots/ai-bench-*.log | tail -100'
```

**Key metrics to extract:**
```bash
# Dataloader throughput
grep "samples/sec" ~/blazil-results/ai-bench-*.log

# Inference RPS
grep "inferences/sec" ~/blazil-results/ai-bench-*.log

# Latency
grep -E "P50|P99|P999" ~/blazil-results/ai-bench-*.log
```

---

### 6. Cleanup

**Destroy droplet:**
```bash
# Via DO dashboard: Destroy blazil-ai-node-1
# Or via doctl CLI:
doctl compute droplet delete blazil-ai-node-1 -f
```

---

## 📊 Expected Results (Reference)

### Conservative (90% confidence)
```
Dataloader:  500K-1M samples/sec
SqueezeNet:  300-500 inferences/sec
ResNet-50:   60-120 inferences/sec
Latency:     P50 ~15ms, P99 ~30ms (SqueezeNet)
             P50 ~50ms, P99 ~100ms (ResNet-50)
```

### Optimistic (50% confidence)
```
Dataloader:  1M-2M samples/sec
SqueezeNet:  500-700 inferences/sec
ResNet-50:   120-180 inferences/sec
Latency:     P50 ~10ms, P99 ~20ms (SqueezeNet)
             P50 ~35ms, P99 ~70ms (ResNet-50)
```

**Baselines from literature:**
- PyTorch DataLoader: 10K-200K samples/sec
- TensorFlow Serving: 100-3K RPS (GPU)
- ONNX Runtime: 1K-2K inferences/sec (8 threads, CPU)
- Tract (our backend): 500-800 inferences/sec (4 cores)

**Comparison with Fintech:**
- Fintech TPS: 130K-270K (DO, 4 vCPU) ← 185-900x higher!
- Fintech Latency: P99 ~300ms ← 10-15x slower
- Why: Fintech batches 8K transfers, AI processes per-request

---

## 🐛 Troubleshooting

**Build fails:**
```bash
# Check Rust version
rustc --version  # Should be 1.88.0

# Re-run setup
cd /opt/blazil && git pull
cargo clean
cargo build --release -p ml-bench
```

**ml-bench not found:**
```bash
# Verify binary exists
ls -lh /opt/blazil/target/release/ml-bench

# Add to PATH
export PATH="/opt/blazil/target/release:$PATH"
```

**Model download fails:**
```bash
# Manual download
cd /opt/blazil/models
curl -L -o squeezenet1.1.onnx https://github.com/onnx/models/raw/main/validated/vision/classification/squeezenet/model/squeezenet1.1-7.onnx
curl -L -o resnet50.onnx https://github.com/onnx/models/raw/main/validated/vision/classification/resnet/model/resnet50-v1-7.onnx
```

**Low performance:**
```bash
# Check CPU throttling
cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor
# Should be "performance", not "powersave"

# Set to performance mode
echo performance | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor
```

---

## 📝 Report Template

```markdown
# Blazil AI Inference - DO Benchmark Results

**Date:** April 29, 2026
**Hardware:** DO Premium AMD NVMe (s-4vcpu-8gb-amd)
**Git commit:** b3e57b1

## Results

### Dataloader Throughput
- Samples/sec: X
- Batch size: 256
- Workers: 4
- Duration: 60s

### SqueezeNet 1.1 Inference
- Inferences/sec: Y
- Latency P50: Z ms
- Latency P99: Z ms
- Batch size: 64
- Workers: 4

### ResNet-50 Inference
- Inferences/sec: Y
- Latency P50: Z ms
- Latency P99: Z ms
- Batch size: 32
- Workers: 4

## Comparison with Baselines
- vs PyTorch DataLoader: X% faster
- vs ONNX Runtime: Y% performance
- vs Fintech TPS: 1/Z ratio (expected)

## Cost Analysis
- Instance cost: $0.125/hour
- Benchmark duration: N hours
- Total cost: $M
- Cost per 1K inferences: $P
```

---

## ⏱️ Timeline (Tomorrow)

```
08:00 - Create DO droplet (15 min)
08:15 - SSH + run ai-do-setup.sh (30 min)
08:45 - Coffee break ☕
09:00 - Run ai-benchmark.sh (10 min)
09:10 - Analyze results
09:30 - Write report
10:00 - Destroy droplet
10:15 - Commit results to repo
```

**Total time:** ~2 hours  
**Total cost:** $0.25 (2 hours @ $0.125/hour)

---

## ✅ Pre-flight Checklist

- [x] Scripts written (`ai-do-setup.sh`, `ai-benchmark.sh`)
- [x] Baselines documented (`docs/AI_BASELINES.md`)
- [x] CI passing (7/7 green, commit `b3e57b1`)
- [x] Hardware spec selected (DO s-4vcpu-8gb-amd)
- [x] Expected results documented
- [ ] DO account has credit
- [ ] SSH key uploaded to DO
- [ ] Terminal ready

**🚀 READY TO GO!**
