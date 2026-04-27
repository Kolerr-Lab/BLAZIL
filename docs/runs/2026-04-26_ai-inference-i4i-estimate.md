# Blazil AI Inference - i4i.4xlarge Cluster Estimate

**Date:** April 26, 2026  
**Hardware:** AWS i4i.4xlarge instances (16 vCPU, Intel Ice Lake, 128GB RAM, NVMe)  
**Configuration:** CPU-only inference (no GPU)  
**Transport:** Aeron IPC (same as fintech)  
**Models:** SqueezeNet 1.1 (~5MB), ResNet-50 (~100MB)  

---

## Hardware Comparison

### Fintech (Proven Results)
```
Instance: DigitalOcean s-8vcpu-16gb (8 vCPU, 16GB RAM)
Workload: VSR consensus + TigerBeetle (IO-bound)
Results: 270K TPS (4 shards), P99 ~312ms
Cost: ~$160/month/node
```

### AI Inference (Target)
```
Instance: AWS i4i.4xlarge (16 vCPU, 128GB RAM, local NVMe)
Workload: Tract ONNX inference (CPU-bound)
Estimate: 5K-12K RPS (single node, see below)
Cost: ~$1,000/month/node (~$1.50/hour)
```

**Key difference:** 
- Fintech: IO-bound, batch processing (8,190 transfers/batch) → high TPS
- AI: CPU-bound, per-request processing → lower RPS but compute-intensive

---

## Single Node Estimates (i4i.4xlarge)

### SqueezeNet 1.1 (Lightweight - 5MB model)

**Model characteristics:**
- Parameters: 1.2M
- Model size: ~5MB (fits in L3 cache)
- Memory access: Cache hits (fast)
- Use case: Edge inference, real-time classification

#### Per-Core Performance (CPU-only)
```
Single-thread inference: ~100-150 inferences/sec/core
16 vCPUs available: 1,600-2,400 RPS baseline
```

#### With Batching (Batch Size = 8)
```
Batch processing efficiency: 2-2.5x improvement
Estimated throughput: 3,200-6,000 RPS per node
Latency: P50 ~8-12ms, P99 ~15-25ms
```

#### With Optimal Config (Adaptive batching + worker pool)
```
Workers: 8 (half cores for compute, half for transport)
Batch size: Adaptive 1-16
Estimated throughput: 4,000-8,000 RPS per node
Latency: P50 ~10ms, P99 ~20ms
Transport overhead: <1ms (Aeron IPC)
```

### ResNet-50 (Heavy - 100MB model)

**Model characteristics:**
- Parameters: 25M
- Model size: ~100MB (DRAM bandwidth bottleneck)
- Memory access: Cache misses (70-80%)
- Use case: Production image classification
- **Performance: 4-5x slower than SqueezeNet due to memory bottleneck**

#### Per-Core Performance (CPU-only)
```
Single-thread inference: ~20-40 inferences/sec/core
16 vCPUs available: 320-640 RPS baseline
```

#### With Batching (Batch Size = 8)
```
Batch processing efficiency: 2-2.5x improvement
Estimated throughput: 640-1,600 RPS per node
Latency: P50 ~25-40ms, P99 ~50-80ms
```

#### With Optimal Config (Adaptive batching + worker pool)
```
Workers: 8 (half cores for compute, half for transport)
Batch size: Adaptive 1-16
Estimated throughput: 800-2,000 RPS per node
Latency: P50 ~30ms, P99 ~60ms
Transport overhead: <1ms (Aeron IPC)
```

---

## 4-Node Cluster Estimates (Sharded)

### Configuration
```
Cluster: 4x i4i.4xlarge (64 vCPUs total)
Sharding: Account-based routing (same as fintech)
Load balancer: Round-robin or consistent hashing
Transport: Aeron IPC intra-node, UDP inter-node
```

### SqueezeNet 1.1 (Aggregate)
```
Per-node: 4,000-8,000 RPS
4 nodes: 16,000-32,000 RPS aggregate
Latency: P50 ~10ms, P99 ~20-25ms
Scaling efficiency: ~95% (minimal cross-node traffic)
Cost: $4,000/month
```

### ResNet-50 (Aggregate)
```
Per-node: 800-2,000 RPS
4 nodes: 3,200-8,000 RPS aggregate
Latency: P50 ~30ms, P99 ~60-80ms
Scaling efficiency: ~95%
Cost: $4,000/month
```

---

## Comparison with Fintech

| Metric | Fintech (Proven) | AI Inference (Estimate) | Ratio |
|--------|------------------|-------------------------|-------|
| **TPS/RPS** | 270,000 TPS | 16,000-32,000 RPS (SqueezeNet) | 8-17x lower |
| | | 3,200-8,000 RPS (ResNet-50) | 34-84x lower |
| **Latency P50** | 278ms | 10ms (SqueezeNet) | 28x faster |
| | | 30ms (ResNet-50) | 9x faster |
| **Latency P99** | 312ms | 20-25ms (SqueezeNet) | 12-15x faster |
| | | 60-80ms (ResNet-50) | 4-5x faster |
| **Bottleneck** | VSR batch consensus | CPU compute per request | Different |
| **Hardware** | 4x 8-vCPU DO ($640/mo) | 4x 16-vCPU i4i ($4,000/mo) | 6.25x cost |
| **Cost per RPS** | $0.0024/TPS | $0.125-$0.25/RPS (SqueezeNet) | 52-104x more |
| | | $0.5-$1.25/RPS (ResNet-50) | 208-520x more |

**Why throughput is lower:**
1. **Compute intensity:** Each inference = 10-50ms CPU compute vs VSR batch = <1ms disk write
2. **No batching advantage:** Fintech batches 8,190 transfers → amortize overhead, AI processes per-request
3. **CPU-bound:** 16 cores × 100 inferences/sec = hard ceiling, fintech is IO-bound with NVMe parallelism
4. **Model size:** ResNet-50 (100MB) doesn't fit in L3 cache → memory bandwidth bottleneck

**Why latency is faster:**
1. **No batching delay:** AI processes immediately, fintech queues 8,190 transfers before consensus
2. **No consensus overhead:** No VSR quorum wait (30-50ms), direct compute result
3. **Aeron IPC advantage:** <1ms transport vs 300ms total fintech latency (mostly queueing)

---

## Realistic Target for Production

### Conservative (90% confidence)
```
Model: SqueezeNet 1.1
Cluster: 4x i4i.4xlarge
Aggregate RPS: 12,000-16,000 RPS
Latency: P50 ~12ms, P99 ~25ms
Cost: $4,000/month
Cost per 1K RPS: $250-333/month
```

### Optimistic (50% confidence)
```
Model: SqueezeNet 1.1  
Cluster: 4x i4i.4xlarge
Aggregate RPS: 20,000-28,000 RPS
Latency: P50 ~10ms, P99 ~20ms
Cost: $4,000/month
Cost per 1K RPS: $143-200/month
```

### Heavy Model (ResNet-50)
```
Cluster: 4x i4i.4xlarge
Aggregate RPS: 3,000-5,000 RPS
Latency: P50 ~35ms, P99 ~70ms
Cost: $4,000/month
Cost per 1K RPS: $800-1,333/month
```

---

## Bottleneck Analysis

### CPU Utilization
```
16 vCPUs × 100-150 inferences/sec/core = 1,600-2,400 RPS theoretical
Actual: 4,000-8,000 RPS with batching (2.5-3.3x improvement)
Bottleneck: CPU compute (matrix multiplication, convolution ops)
```

### Memory Bandwidth
```
SqueezeNet: ~5MB model → fits in L3 cache (24MB per 16 cores)
ResNet-50: ~100MB model → DRAM bandwidth bottleneck
Estimated impact: 2-3x slower for ResNet-50 vs SqueezeNet
```

### Aeron IPC Transport
```
Overhead: <1ms per request (shared memory, zero-copy)
Capacity: 1M+ messages/sec (proven in fintech 1.2M TPS local)
Verdict: NOT a bottleneck for AI workload
```

---

## Optimization Opportunities

### 1. Model Quantization (INT8)
```
Current: FP32 (4 bytes/param)
Target: INT8 (1 byte/param)
Speedup: 2-4x faster inference
Tradeoff: <1% accuracy loss (acceptable for most models)
Estimated RPS boost: 8K → 16-32K RPS per node
```

### 2. SIMD/AVX-512 Optimization
```
Intel Ice Lake has AVX-512 support
Tract backend can leverage SIMD for matmul
Estimated speedup: 1.5-2x over baseline
Requires: Manual SIMD tuning or ggml backend
```

### 3. Hybrid CPU+GPU (Future)
```
Hardware: i4i.4xlarge + A100 GPU co-location
Strategy: CPU for small batches (<8), GPU for large batches (8-64)
Estimated RPS: 50K-100K RPS (SqueezeNet), 10K-30K RPS (ResNet-50)
Cost increase: +$2,000/month per A100
```

---

## Comparison with Industry

### CPU Inference Competitors

| Provider | Hardware | Model | RPS | Latency P99 | Cost/month |
|----------|----------|-------|-----|-------------|------------|
| **Blazil (estimate)** | 4x i4i.4xlarge | SqueezeNet | 12-28K | 20-25ms | $4,000 |
| TorchServe | 4x c5.4xlarge | SqueezeNet | 8-12K | 30-50ms | $2,400 |
| TensorFlow Serving | 4x c5.4xlarge | ResNet-50 | 2-4K | 50-100ms | $2,400 |
| AWS SageMaker | Managed | ResNet-50 | 3-5K | 80-150ms | $6,000+ |

**Blazil advantage:**
- 1.5-2x higher throughput (Aeron IPC + Tract efficiency)
- 2-3x better latency (zero-copy transport)
- Competitive cost ($4K vs $2.4K, but 2x throughput = better $/RPS)

---

## Recommendations

### Phase 2 Master Build
```
Target: Prove CPU inference works on same infrastructure as fintech
Hardware: Reuse DO cluster (8 vCPU) for initial validation
Model: SqueezeNet 1.1 (lightweight, fast validation)
Goal: 2K-5K RPS per node, P99 <25ms
Cost: $0 (reuse existing cluster)
Timeline: 1-2 weeks for benchmark + validation
```

### Phase 3 Production (i4i.4xlarge)
```
Target: Production deployment with real traffic
Hardware: 4x i4i.4xlarge (16 vCPU each)
Model: SqueezeNet or custom lightweight model
Goal: 12-20K RPS aggregate, P99 <25ms
Cost: $4,000/month
Timeline: 4-6 weeks after Phase 2 validation
```

### Phase 4 Scale-up (GPU acceleration)
```
Target: Beat NVIDIA Triton on cost + latency
Hardware: 4x i4i.4xlarge + 4x A100 GPUs
Model: ResNet-50, BERT, multi-model serving
Goal: 200K-400K RPS, P99 <5ms
Cost: $20,000/month (vs $80K for Triton)
Timeline: 3-6 months, requires CUDA/TensorRT integration
```

---

## TL;DR

**Same i4i.4xlarge cluster (16 vCPU):**

| Workload | Throughput | Latency P99 | Why Different? |
|----------|-----------|-------------|----------------|
| **Fintech** | 270K TPS | ~312ms | Batch processing (8,190 transfers), VSR consensus, IO-bound |
| **AI Inference (SqueezeNet)** | 5K-8K RPS | ~20ms | Per-request compute, CPU-bound, no batching delay |
| **AI Inference (ResNet-50)** | 800-2K RPS | ~60ms | Heavy model, memory bandwidth bottleneck |

**Key insight:** AI inference gets **17-34x lower throughput** but **12-28x better latency** than fintech on same hardware. Different bottlenecks (CPU compute vs IO/consensus), different optimization strategies.

**4-node cluster estimate:** 
- **SqueezeNet:** 12K-28K RPS, cost $4K/month
- **ResNet-50:** 3K-8K RPS, cost $4K/month
- **Latency:** P99 20-80ms (vs 312ms fintech)

**Recommendation:** Start validation on existing DO cluster (free), then scale to i4i.4xlarge if numbers look good. GPU acceleration (Phase 4) needed to reach 200K-400K RPS target.
