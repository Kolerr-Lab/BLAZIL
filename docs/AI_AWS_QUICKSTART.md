# Blazil AI Benchmark - AWS Quick Start (May 2026)

**Status:** ✅ PRODUCTION-READY  
**Hardware:** AWS i4i.4xlarge  
**Cost:** $1.248/hour (~$44 for 35-minute benchmark)

---

## 🚀 Quick Commands (Copy-Paste Ready)

### 1. Provision AWS EC2 Instance (10 min)

**Instance Specs:**
```
Instance Type: i4i.4xlarge
Region: us-east-1 (or your preferred region)
vCPU: 16 (Intel Ice Lake @ 3.5 GHz)
RAM: 128 GB DDR4
Storage: 1× 1.9 TB NVMe SSD (instance store)
Network: Up to 25 Gbps
OS: Ubuntu 24.04 LTS
```

**Launch via AWS CLI:**
```bash
aws ec2 run-instances \
  --instance-type i4i.4xlarge \
  --image-id ami-0c55b159cbfafe1f0 \
  --key-name your-keypair \
  --security-group-ids sg-xxxxxxxxx \
  --subnet-id subnet-xxxxxxxxx \
  --block-device-mappings DeviceName=/dev/sda1,Ebs={VolumeSize=100} \
  --tag-specifications 'ResourceType=instance,Tags=[{Key=Name,Value=blazil-ai-bench}]'
```

**Get IP:**
```bash
INSTANCE_IP=$(aws ec2 describe-instances \
  --filters "Name=tag:Name,Values=blazil-ai-bench" "Name=instance-state-name,Values=running" \
  --query 'Reservations[0].Instances[0].PublicIpAddress' \
  --output text)
echo "Instance IP: $INSTANCE_IP"
```

---

### 2. Setup Node (20 min)

**SSH + Run Setup:**
```bash
# SSH into instance
ssh ubuntu@$INSTANCE_IP

# Run automated setup (installs Rust, builds code, downloads models, tunes system)
curl -fsSL https://raw.githubusercontent.com/Kolerr-Lab/BLAZIL/main/scripts/ai-aws-setup.sh | sudo bash

# Expected output:
#   - System tuning applied (CPU isolation, IRQ affinity, performance governor)
#   - Rust 1.88.0 installed
#   - Blazil repo cloned to /opt/blazil
#   - ml-bench built (8-12 min)
#   - SqueezeNet 1.1 downloaded (~5 MB)
#   - ResNet-50 downloaded (~100 MB)
#   - Synthetic dataset generation tested

# Verify build
ls -lh /opt/blazil/target/release/ml-bench
ls -lh /opt/blazil/models/*.onnx
```

---

### 3. Run 35-Minute Production Benchmark

**Full Production Test (Publication-Ready):**
```bash
cd /opt/blazil
sudo ./scripts/ai-benchmark.sh

# Phases:
#   Phase 1: Dataloader (600s / 10 min) - thermal stability
#   Phase 2: SqueezeNet (600s / 10 min) - lightweight model
#   Phase 3: ResNet-50 (900s / 15 min) - heavy model stress test
#
# Total: 35 minutes
# Output: docs/benchmark-screenshots/ai-bench-<timestamp>.log
```

**With Live Dashboard (Optional):**
```bash
# Terminal 1: Start metrics server
cd /opt/blazil
sudo ./target/release/ml-bench \
  --mode dataloader \
  --dataset synthetic \
  --path /tmp/blazil-synthetic \
  --duration 600 \
  --metrics-port 9092

# Terminal 2: Access dashboard (from local machine)
ssh -L 3331:localhost:3331 ubuntu@$INSTANCE_IP
# Then open: http://localhost:3331
```

---

### 4. Monitor Health & Metrics

**Check Health Status:**
```bash
# Real-time health monitoring
watch -n 2 'curl -s http://localhost:9092/health | jq'

# Expected output:
# {
#   "status": "healthy",
#   "uptime_secs": 155,
#   "success_rate": "0.9998",
#   "error_rate": "0.0002",
#   "latency": {
#     "p50_us": 6234,
#     "p99_us": 12345,
#     "p999_us": 18900
#   },
#   "sla": { "meets_sla": true }
# }
```

**Prometheus Metrics:**
```bash
# Scrape metrics
curl http://localhost:9092/metrics

# Key metrics:
# - ml_bench_uptime_seconds
# - ml_bench_requests_total{result="success"}
# - ml_bench_latency_microseconds{quantile="0.99"}
# - ml_bench_sla_compliance
```

**System Monitoring:**
```bash
# Terminal 1: CPU/Memory
htop

# Terminal 2: Disk I/O
iostat -x 1

# Terminal 3: Network
iftop
```

---

### 5. Collect Results

**Copy logs to local machine:**
```bash
# From local machine
scp ubuntu@$INSTANCE_IP:/opt/blazil/docs/benchmark-screenshots/ai-bench-*.log ~/blazil-results/

# Or view directly
ssh ubuntu@$INSTANCE_IP 'cat /opt/blazil/docs/benchmark-screenshots/ai-bench-*.log | tail -100'
```

**Extract key metrics:**
```bash
# Dataloader throughput
grep "samples/sec" ~/blazil-results/ai-bench-*.log

# Inference RPS
grep "inferences/sec" ~/blazil-results/ai-bench-*.log

# Latency percentiles
grep -E "P50|P99|P999" ~/blazil-results/ai-bench-*.log

# SLA compliance
grep "SLA compliance" ~/blazil-results/ai-bench-*.log
```

---

### 6. Cleanup

**Terminate EC2 instance:**
```bash
# Get instance ID
INSTANCE_ID=$(aws ec2 describe-instances \
  --filters "Name=tag:Name,Values=blazil-ai-bench" "Name=instance-state-name,Values=running" \
  --query 'Reservations[0].Instances[0].InstanceId' \
  --output text)

# Terminate
aws ec2 terminate-instances --instance-ids $INSTANCE_ID

# Or via AWS Console: EC2 → Instances → Select → Instance State → Terminate
```

---

## 📊 Expected Results (AWS i4i.4xlarge, 16 vCPU)

### Conservative (90% confidence)
```
Dataloader:  1M-5M samples/sec (sequential, io_uring)
             500K-1M samples/sec (with shuffle)
SqueezeNet:  1,600-2,000 inferences/sec
ResNet-50:   320-450 inferences/sec
Latency:     P50 ~8ms, P99 ~15ms (SqueezeNet)
             P50 ~25ms, P99 ~50ms (ResNet-50)
```

### Optimistic (Blazil Track Record)
```
Dataloader:  5M-10M+ samples/sec (zero-copy, NVMe)
             1M-2M samples/sec (random access)
SqueezeNet:  2,000-2,400 inferences/sec
ResNet-50:   450-640 inferences/sec
Latency:     P50 ~6ms, P99 ~12ms (SqueezeNet)
             P50 ~20ms, P99 ~40ms (ResNet-50)

Note: Blazil Fintech proven 233,894 TPS with VSR consensus overhead.
      AI workload = NO consensus → potentially faster.
```

**Baselines from literature:**
- PyTorch DataLoader: 10K-200K samples/sec (Python, GIL bottleneck)
- TensorFlow Serving: 100-3K RPS (GPU, HTTP overhead)
- ONNX Runtime: 1K-2K inferences/sec (8 threads, CPU)
- Tract (Blazil backend): Rust native, zero-copy, io_uring

**Comparison with Blazil Fintech:**
- Fintech: 233,894 TPS (with VSR consensus + TigerBeetle)
- AI: NO consensus overhead = pure compute/IO throughput
- Architecture: Same Rust codebase, same performance DNA

---

## 🐛 Troubleshooting

**Build fails:**
```bash
# Check Rust version
rustc --version  # Should be 1.88.0+

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
# Check CPU governor
cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor
# Should be "performance", not "powersave"

# Set to performance mode (if needed)
echo performance | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor

# Verify NVMe is detected
lsblk | grep nvme
# Should show instance store volumes

# Check system tuning was applied
grep isolcpus /proc/cmdline
# Should show isolated CPU cores (if ai-aws-setup.sh ran successfully)
```

---

## 📝 Report Template

```markdown
# Blazil AI Inference - AWS Benchmark Results

**Date:** May 12, 2026
**Hardware:** AWS i4i.4xlarge (16 vCPU, 128 GB RAM, NVMe SSD)
**Git commit:** dc15fd2

## Results

### Phase 1: Dataloader (10 min)
- Throughput: X samples/sec
- Batch size: 256
- Workers: 8-16
- Access pattern: Sequential / Shuffle

### Phase 2: SqueezeNet 1.1 Inference (10 min)
- Throughput: Y inferences/sec
- Latency: P50 Xms, P99 Yms, P999 Zms
- Batch size: 64
- Workers: 12
- Model: ~5 MB

### Phase 3: ResNet-50 Inference (15 min)
- Throughput: Z inferences/sec
- Latency: P50 Xms, P99 Yms, P999 Zms
- Batch size: 32
- Workers: 8
- Model: ~100 MB

## Health & SLA
- Uptime: X seconds
- Success rate: 99.XX%
- Error rate: 0.XX%
- SLA compliance: PASS/FAIL (error rate < 1%, P99 < 50ms, uptime > 99.9%)

## Comparison with Baselines
- vs PyTorch DataLoader: 10-50× faster (Python GIL vs Rust)
- vs ONNX Runtime: X% performance (depends on threading)
- vs Blazil Fintech: Different workload (no consensus overhead)

## Cost Analysis
- Instance: AWS i4i.4xlarge @ $1.248/hour
- Benchmark: 35 minutes = 0.583 hours
- Total: ~$0.73
- Per 1M inferences: ~$X (depends on throughput)
```

---

## ⏱️ Execution Timeline

```
T+0:00 - Launch AWS i4i.4xlarge via AWS Console/CLI (10 min)
T+0:10 - SSH + run ai-aws-setup.sh (20 min)
         → Install deps, tune system, build Rust, download models
T+0:30 - Start 35-minute benchmark
         Phase 1: Dataloader (10 min)
         Phase 2: SqueezeNet (10 min)
         Phase 3: ResNet-50 (15 min)
T+1:05 - Collect logs, verify health endpoint
T+1:15 - Terminate EC2 instance
T+1:20 - Analyze results locally
T+1:40 - Commit benchmark report to repo
```

**Total time:** ~2 hours  
**Total cost:** $1.248/hr × 1.25 hr = ~$1.56  
**For benchmark only:** 35 min = $0.73

---

## ✅ Pre-flight Checklist

- [x] Production resilience features complete (health, SLA, graceful shutdown)
- [x] CI passing (7/7 green, commit `dc15fd2`)
- [x] Hardware: AWS i4i.4xlarge (16 vCPU, 128 GB, NVMe)
- [x] Performance baselines documented (docs/AI_BASELINES.md - AWS targets)
- [x] Setup script ready: scripts/ai-aws-setup.sh
- [ ] AWS account with appropriate limits (i4i.4xlarge availability)
- [ ] SSH keypair configured in AWS region
- [ ] Terminal ready

**🚀 READY TO GO!**
