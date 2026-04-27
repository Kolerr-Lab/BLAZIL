# Blazil AI Inference - Performance Baselines

**Last updated:** April 28, 2026

This document tracks known performance baselines for AI inference workloads across different hardware configurations.

---

## Hardware Profiles

### DO Premium AMD NVMe (Target for April 29, 2026 Benchmark)
```
Instance: s-4vcpu-8gb-amd
CPU: 4 vCPU @ 2.0 GHz (AMD EPYC)
RAM: 8 GB
Storage: 160 GB NVMe SSD
Network: 5 TB transfer
Cost: $84/month ($0.125/hour)
```

### AWS i4i.4xlarge (Reference from docs)
```
Instance: i4i.4xlarge
CPU: 16 vCPU @ 2.9 GHz (Intel Ice Lake)
RAM: 128 GB
Storage: 1.9 TB NVMe (local)
Network: Up to 12.5 Gbps
Cost: ~$1.50/hour (~$1,080/month)
```

### Local Development (MacBook Air M4)
```
CPU: Apple M4 (10-core, 4 P + 6 E)
RAM: 16 GB unified memory
Storage: 512 GB SSD
```

---

## Baseline Performance (Estimated)

### SqueezeNet 1.1 (5 MB model)

**Single-threaded (1 core):**
```
CPU: 100-150 inferences/sec
Latency: P50 ~6-10ms, P99 ~12-15ms
Memory: 50-100 MB RSS
```

**Multi-threaded (4 cores, batch=64):**
```
Throughput: 400-600 inferences/sec (DO 4 vCPU estimate)
           1,600-2,400 inferences/sec (AWS 16 vCPU estimate)
Latency: P50 ~10-15ms, P99 ~20-30ms (with batching)
CPU util: 80-90% (compute-bound)
```

### ResNet-50 (100 MB model)

**Single-threaded (1 core):**
```
CPU: 20-40 inferences/sec
Latency: P50 ~25-50ms, P99 ~60-100ms
Memory: 200-400 MB RSS (model in RAM)
```

**Multi-threaded (4 cores, batch=32):**
```
Throughput: 80-160 inferences/sec (DO 4 vCPU estimate)
           320-640 inferences/sec (AWS 16 vCPU estimate)
Latency: P50 ~30-50ms, P99 ~80-120ms (with batching)
CPU util: 90-95% (memory bandwidth bottleneck)
```

---

## Dataloader Performance (Synthetic Dataset)

**io_uring Reader (Linux):**
```
Throughput: 10M+ samples/sec (zero-copy, sequential read)
           1M+ samples/sec (with shuffle, random access)
Latency: P50 ~1-2ms, P99 ~5-10ms (per batch)
CPU util: 20-40% (I/O bound)
Bottleneck: Disk IOPS (NVMe: 200K+ IOPS)
```

**mmap Reader (macOS/cross-platform):**
```
Throughput: 5M+ samples/sec (memory-mapped, sequential)
           500K+ samples/sec (with shuffle)
Latency: P50 ~2-5ms, P99 ~10-20ms (per batch)
CPU util: 30-50% (page fault overhead)
```

---

## Known Benchmarks (From Literature)

### PyTorch DataLoader (Python baseline)
```
Throughput: 10K-50K samples/sec (single worker)
           50K-200K samples/sec (8 workers)
Bottleneck: GIL, pickle serialization
Source: PyTorch official docs
```

### TensorFlow Serving (REST API)
```
Model: ResNet-50
Throughput: 100-500 RPS (single GPU, batch=1)
           1K-3K RPS (single GPU, batch=32)
Latency: P50 ~50ms, P99 ~150ms (HTTP overhead)
Source: TensorFlow Serving benchmark repo
```

### ONNX Runtime (C++ baseline)
```
Model: SqueezeNet 1.1
Throughput: 1,000-2,000 inferences/sec (CPU, 8 threads)
Latency: P50 ~5ms, P99 ~10ms
Source: ONNX Runtime performance docs
```

### Tract (Blazil's backend)
```
Model: MobileNetV2
Throughput: 500-800 inferences/sec (4 cores, CPU)
Latency: P50 ~8ms, P99 ~15ms
Pure Rust, zero-copy
Source: Tract GitHub benchmarks
```

---

## Expected DO Results (April 29, 2026)

### Conservative (90% confidence)
```
Dataloader: 500K-1M samples/sec
SqueezeNet: 300-500 inferences/sec (4 vCPU)
ResNet-50: 60-120 inferences/sec (4 vCPU)
Latency: P50 ~15ms, P99 ~30ms (SqueezeNet)
         P50 ~50ms, P99 ~100ms (ResNet-50)
```

### Optimistic (50% confidence)
```
Dataloader: 1M-2M samples/sec
SqueezeNet: 500-700 inferences/sec
ResNet-50: 120-180 inferences/sec
Latency: P50 ~10ms, P99 ~20ms (SqueezeNet)
         P50 ~35ms, P99 ~70ms (ResNet-50)
```

---

## Comparison with Fintech Workload

| Metric | Fintech (Proven) | AI Inference (Estimate) | Ratio |
|--------|------------------|-------------------------|-------|
| **TPS/RPS** | 130K-270K TPS | 300-700 RPS | 185-900x lower |
| **Latency** | P99 ~300ms | P99 ~20-30ms | 10-15x faster |
| **Bottleneck** | VSR consensus | CPU compute | Different |
| **Hardware** | 4 vCPU DO | 4 vCPU DO | Same |
| **Workload** | I/O bound (batch) | CPU bound (per-request) | - |

**Why TPS is much lower:**
- Fintech batches 8,190 transfers → amortize overhead
- AI processes per-request (or small batches of 32-64)
- Each inference = 10-50ms CPU vs VSR batch = <1ms write

**Why latency is faster:**
- No batching delay (immediate compute)
- No consensus wait (30-50ms VSR quorum)
- Aeron IPC < 1ms vs fintech 300ms total (mostly queueing)

---

## Optimization Targets

**Phase 1 (Current):**
- ✅ Core inference working (Tract)
- ✅ Dataloader working (io_uring)
- ✅ Benchmark tool (ml-bench)
- 🎯 Establish baseline on DO hardware

**Phase 2 (Next week):**
- 🎯 Adaptive batching (dynamic batch size)
- 🎯 Worker pool optimization (thread pinning)
- 🎯 Model caching (reduce load time)
- 🎯 Batch size tuning per model

**Phase 3 (Future):**
- 🎯 GPU support (CUDA via tract-gpu)
- 🎯 Model quantization (INT8, FP16)
- 🎯 Multi-model serving
- 🎯 TensorRT backend (NVIDIA)

---

## References

- TensorFlow Serving benchmarks: https://github.com/tensorflow/serving/tree/master/tensorflow_serving/g3doc/performance
- ONNX Runtime performance: https://onnxruntime.ai/docs/performance/
- Tract benchmarks: https://github.com/sonos/tract/tree/main/benches
- PyTorch DataLoader: https://pytorch.org/docs/stable/data.html#multi-process-data-loading
- ImageNet: http://www.image-net.org/challenges/LSVRC/2012/
