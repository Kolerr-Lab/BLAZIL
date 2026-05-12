# Blazil AI Inference - Performance Baselines

**Last updated:** April 28, 2026

This document tracks known performance baselines for AI inference workloads across different hardware configurations.

---

## Hardware Profiles

### AWS i4i.4xlarge (Production Benchmark Target)
```
Instance: i4i.4xlarge
CPU: 16 vCPU @ 3.5 GHz (Intel Ice Lake)
RAM: 128 GB DDR4
Storage: 1× 1.9 TB NVMe SSD (instance store)
Network: Up to 25 Gbps
Cost: $1.248/hour (~$900/month spot)
Note: Production benchmark target with enhanced networking
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

**Multi-threaded (16 cores, batch=64, AWS i4i.4xlarge):**
```
Throughput: 1,600-2,400 inferences/sec
Latency: P50 ~8-12ms, P99 ~15-25ms (with batching)
CPU util: 80-90% (compute-bound)
Memory: 200-400 MB RSS total
```

### ResNet-50 (100 MB model)

**Single-threaded (1 core):**
```
CPU: 20-40 inferences/sec
Latency: P50 ~25-50ms, P99 ~60-100ms
Memory: 200-400 MB RSS (model in RAM)
```

**Multi-threaded (16 cores, batch=32, AWS i4i.4xlarge):**
```
Throughput: 320-640 inferences/sec
Latency: P50 ~25-40ms, P99 ~50-80ms (with batching)
CPU util: 90-95% (memory bandwidth bottleneck)
Memory: 500-800 MB RSS total
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

## Expected AWS Results (i4i.4xlarge, 16 vCPU, May 2026)

### Conservative (90% confidence)
```
Dataloader: 1M-5M samples/sec (io_uring, sequential)
           500K-1M samples/sec (with shuffle)
SqueezeNet: 1,600-2,000 inferences/sec
ResNet-50: 320-450 inferences/sec
Latency: P50 ~8ms, P99 ~15ms (SqueezeNet)
         P50 ~25ms, P99 ~50ms (ResNet-50)
```

### Optimistic (50% confidence - Blazil Track Record)
```
Dataloader: 5M-10M+ samples/sec (sequential, zero-copy)
           1M-2M samples/sec (shuffle, random access)
SqueezeNet: 2,000-2,400 inferences/sec
ResNet-50: 450-640 inferences/sec
Latency: P50 ~6ms, P99 ~12ms (SqueezeNet)
         P50 ~20ms, P99 ~40ms (ResNet-50)

Note: Blazil Fintech proven 233,894 TPS with VSR overhead.
      AI workload has NO consensus = potentially faster.
```

---

## Comparison with Fintech Workload

| Metric | Fintech (Proven) | AI Inference (Target) | Ratio |
|--------|------------------|----------------------|-------|
| **TPS/RPS** | 233,894 TPS | 1,600-2,400 RPS (SqueezeNet) | ~100-150x lower |
| **Latency** | P99 ~300ms | P99 ~15ms | ~20x faster |
| **Bottleneck** | VSR consensus | CPU/memory bandwidth | Different |
| **Hardware** | AWS (Fintech) | AWS i4i.4xlarge 16 vCPU | - |
| **Workload** | I/O bound (batch 8K) | CPU bound (batch 32-64) | - |

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

**Phase 1 (v0.3.1 - Implementation Complete):**
- ✅ Core inference working (Tract)
- ✅ Dataloader working (io_uring)
- ✅ Benchmark tool (ml-bench)
- ❌ Baseline benchmark not conducted (planned for v0.5 on AWS)

**Phase 2 (Future):**
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
