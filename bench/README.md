# Blazil Benchmark Suite

Performance benchmarks for the Blazil transaction engine.

## Quick Start

```bash
cargo run --release -p blazil-bench
```

**⚠️ Always run in `--release` mode.** Debug builds are 10-100× slower.

---

## Benchmark Scenarios

### 1. **Sharded Pipeline** (Primary throughput metric)

Multi-shard parallel pipeline with independent ring buffers per shard.

**Architecture:**
- N independent pipelines (1 per shard)
- 1 producer thread per shard (LMAX Disruptor pattern)
- 1 consumer thread per shard (dedicated tokio runtime)
- Events route by `account_id % shard_count`

**Results (M4 MacBook Air, 4-shard):**
```
1-shard:  77M TPS (1 producer, 1 consumer)
4-shard: 167M TPS (4 producers, 4 consumers)
Speedup: 2.17× (54% efficiency)
```

**Key insight:** Each producer writes to ONLY ONE ring buffer for perfect cache locality.

### 2. **Single Pipeline** (Latency baseline)

Traditional single-threaded pipeline with per-event latency tracking.

**Results (M4 MacBook Air):**
```
Throughput: 24.4M TPS
P50: 38ns
P99: 42ns
P999: 89ns
```

**Handler chain (all scenarios):**
1. ValidationHandler (field validation)
2. RiskHandler (amount limits)
3. LedgerHandler (`InMemoryLedgerClient::new_unbounded()`)
4. PublishHandler (result storage)

---

## Methodology Notes

### ⚠️ Why Different Numbers?

**Single Pipeline: 24M TPS**  
**Sharded 1-shard: 77M TPS**

Both run the **same 4-handler pipeline**, but:

**Single Pipeline** (per-event timing):
```rust
for _ in 0..events {
    let t0 = Instant::now();  // ← System call overhead
    last_seq = publish(&pipeline, event.clone());
    latencies.push(t0.elapsed().as_nanos() as u64);  // ← Vec allocation
}
```

**Sharded Pipeline** (bulk timing):
```rust
let start = Instant::now();
for event in pre_allocated_events {
    publish(&sharded, event);  // ← No per-event overhead
}
let duration = start.elapsed();
```

**Measurement overhead:**
- `Instant::now()`: ~10-20ns per call × 1M = 10-20ms
- `Vec::push()`: bounds check + allocation × 1M
- `clone()`: UUID copying in tight loop

**Result:** 3× difference is measurement artifact, not architectural improvement.

### 🎯 Honest Reporting

For academic papers and benchmarks:

**Throughput (bulk timing):**
- 1-shard: 77M TPS
- 4-shard: 167M TPS
- 16-shard extrapolation: ~600-700M TPS (50% efficiency on bare metal)

**Latency (with tracking overhead):**
- Single pipeline: 24M TPS
- P99: 42ns

**Both are correct.** Document which method you're using.

---

## Extrapolation to 16-Core Bare Metal

**Conservative (50% efficiency):**
```
77M TPS × 16 shards × 0.50 = ~616M TPS
```

**Observed (54% efficiency on M4):**
```
77M TPS × 16 shards × 0.54 = ~665M TPS
```

**Optimistic (70% with tuning):**
```
77M TPS × 16 shards × 0.70 = ~862M TPS
```

**Why not 100% efficiency?**
- Shared memory bandwidth contention
- Inter-core communication overhead
- OS scheduler interference
- NUMA effects on bare metal

**Real-world target: 600-700M TPS on 16-core Xeon/EPYC.**

---

## Benchmark Configuration

**Event size:** 56 bytes  
**Ring buffer capacity:** 1,048,576 slots (1M)  
**Memory footprint:** 53 MB per shard  
**Warmup:** 100 events per scenario  
**Test events:** 1,000,000 per scenario  

**Ledger mode:** `InMemoryLedgerClient::new_unbounded()`  
- Skips balance validation
- No balance updates
- Pure throughput test
- Single account pair absorbs millions of debits

**Why unbounded?** To isolate pipeline performance from ledger validation overhead.

---

## Other Scenarios

### Ring Buffer (baseline)

Raw ring buffer throughput without handlers.

**Result:** ~12.5M ops/s, 84ns P99

### Full Pipeline (4 handlers)

Original benchmark with all handlers + latency tracking.

**Result:** 24.4M TPS, 42ns P99

### TCP Scenario

gRPC client → Go service → Rust engine

**Result:** ~50K TPS (network + serialization overhead)

### TigerBeetle Scenario

Full stack with real VSR consensus.

**Result:** ~2K TPS (disk + 3-node consensus overhead)

---

## Running Individual Scenarios

```bash
# Full suite
cargo run --release -p blazil-bench

# Specific scenario (code change required)
# Edit bench/src/main.rs to comment out unwanted scenarios
```

---

## Interpreting Results

**High numbers (>10M TPS):** In-memory, no network, optimized path  
**Medium numbers (1-10M TPS):** Some serialization, local TCP, batching  
**Low numbers (<100K TPS):** Network I/O, disk writes, consensus  

**All numbers are honest.**  
Methodology is clearly documented.  
Choose the right metric for your use case.

---

## Citation

If you use these benchmarks in research:

```bibtex
@software{blazil2026,
  title = {Blazil: Open-Core Financial Infrastructure},
  author = {Kolerr Lab},
  year = {2026},
  url = {https://github.com/Kolerr-Lab/BLAZIL},
  note = {Sharded pipeline: 167M TPS (4-shard, bulk timing), 
          Single pipeline: 24M TPS (latency-tracked)}
}
```

**Key points for academic integrity:**
1. Both measurements use identical handler chains
2. Difference is timing methodology, not architecture
3. Sharded implementation achieves 2.17× parallel scaling
4. Document which method you're comparing against

---

## Hardware Used

**Laptop benchmarks:** M4 MacBook Air (2024)  
- 4 performance cores + 4 efficiency cores
- 16 GB unified memory
- macOS 15

**Cluster benchmarks:** 3× DigitalOcean c2-4vcpu-8GB  
- Intel Xeon (4 vCPU)
- 8 GB RAM
- NVMe SSD
- Ubuntu 22.04

---

## See Also

- [Cluster benchmark report](../docs/do-benchmark-report-final.md)
- [Architecture decision records](../docs/adr/)
- [LMAX Disruptor patterns](../docs/architecture/001-monorepo-structure.md)
