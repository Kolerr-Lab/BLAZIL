# Runbook: High Latency / Engine Back-pressure

**Severity**: P1 (p99 > 500ms) or P0 (p99 > 2s or throughput collapse)  
**Trigger**: Alert `RingBufferHighUtilization` (> 80%) or manual observation

---

## Baseline numbers

| Metric | Normal | Warning | Critical |
|--------|--------|---------|---------|
| Engine p99 latency | < 50 µs | 500 µs | > 2 ms |
| Ring buffer utilization | < 40% | > 60% | > 80% |
| TigerBeetle commit latency | < 5 ms | 20 ms | > 100 ms |
| Transaction throughput | > 50k TPS | < 10k TPS | < 1k TPS |

---

## Step 1 — Confirm the symptom

```bash
kubectl port-forward svc/prometheus 9090:9090 -n blazil &
PROM=http://localhost:9090

# Ring buffer utilization (main back-pressure indicator)
curl -sf "$PROM/api/v1/query?query=blazil_ring_buffer_utilization_ratio" \
  | python3 -m json.tool | grep value

# Transaction error rate
curl -sf "$PROM/api/v1/query?query=sum(rate(blazil_transactions_total{status!%3D%22success%22}[5m]))/sum(rate(blazil_transactions_total[5m]))" \
  | python3 -m json.tool | grep value

# Throughput
curl -sf "$PROM/api/v1/query?query=sum(rate(blazil_transactions_total[1m]))" \
  | python3 -m json.tool | grep value
```

---

## Step 2 — Identify the bottleneck

### Is TigerBeetle slow?

TigerBeetle is typically the slowest component (disk I/O bound).

```bash
# VSR commit latency in TB logs
kubectl logs tigerbeetle-0 -n blazil --tail=100 | grep -i "commit\|latency\|slow"

# Check disk I/O saturation on TB nodes
kubectl top nodes
kubectl exec tigerbeetle-0 -n blazil -- cat /proc/diskstats 2>/dev/null || \
  kubectl exec tigerbeetle-0 -n blazil -- iostat -x 1 5

# Is the volume almost full?
kubectl exec tigerbeetle-0 -n blazil -- df -h /data
```

**If TB disk is > 80% full**:
```bash
# Expand the PVC (requires the StorageClass to support volume expansion)
kubectl patch pvc tigerbeetle-data-tigerbeetle-0 -n blazil \
  -p '{"spec":{"resources":{"requests":{"storage":"200Gi"}}}}'
```

### Is the engine ring buffer backed up?

```bash
kubectl logs -l app=blazil-engine -n blazil --tail=200 | \
  grep -i "ring\|disruptor\|back.pressure\|overflow\|slow"
```

**Cause A — Batch accumulator saturated**: too many small transactions, batch
limit of 100 not reached quickly enough, or TB is rejecting batches.

**Cause B — Cross-shard traffic spikes**: one shard is receiving disproportionate
load. Check shard assignments:

```bash
kubectl get configmap blazil-shard-map -n blazil -o yaml
```

### Is the network saturated?

```bash
kubectl exec -it blazil-engine-0 -n blazil -- cat /proc/net/dev
# Rx/Tx bytes per second on the primary interface
```

### Is io_uring the problem?

```bash
# io_uring backlog
kubectl logs -l app=blazil-engine -n blazil --tail=100 | grep -i "io.uring\|sq_ring\|cq_ring"

# Fall back to TCP transport temporarily (loses ~30% throughput, but stable)
kubectl set env statefulset/blazil-engine BLAZIL_TRANSPORT=tcp -n blazil
kubectl rollout status statefulset/blazil-engine -n blazil --timeout=10m
# Revert once io_uring issue is resolved:
# kubectl set env statefulset/blazil-engine BLAZIL_TRANSPORT=io-uring -n blazil
```

---

## Step 3 — Mitigation

### Reduce load (temporary)

```bash
# Scale Go services down to reduce inbound TPS
kubectl scale deployment blazil-payments --replicas=3 -n blazil
kubectl scale deployment blazil-trading  --replicas=3 -n blazil
# Reduces the volume of gRPC→engine calls.
```

### Increase engine resources (if CPU-bound)

```bash
# Patch engine CPU limit in prod overlay, then apply
kubectl patch statefulset blazil-engine -n blazil \
  --type='json' \
  -p='[{"op":"replace","path":"/spec/template/spec/containers/0/resources/limits/cpu","value":"10"}]'
```

### Restart the engine (last resort)

Only if the ring buffer is stuck and not draining after 10 minutes.

```bash
# Rolling restart — StatefulSet replaces pods one at a time
kubectl rollout restart statefulset/blazil-engine -n blazil
kubectl rollout status  statefulset/blazil-engine -n blazil --timeout=10m
```

TigerBeetle retains all committed data. In-flight transactions that were in
the ring buffer at restart time will be retried by Go services (gRPC deadline
exceeded → client retry). This is safe.

---

## Step 4 — Verify recovery

```bash
# Ring buffer should be < 40%
curl -sf "$PROM/api/v1/query?query=blazil_ring_buffer_utilization_ratio" \
  | python3 -m json.tool | grep value

# Throughput recovering
curl -sf "$PROM/api/v1/query?query=sum(rate(blazil_transactions_total[1m]))" \
  | python3 -m json.tool | grep value
```

---

## Step 5 — Post-incident

- [ ] Record the peak utilization value and timestamp
- [ ] Check if an upcoming release includes Aeron IPC tuning
- [ ] Consider increasing `AERON_TERM_BUFFER_LENGTH` (currently 128 MiB) if the
      ring buffer was saturated by large message bursts — update the ConfigMap
      and restart engine
- [ ] Open a capacity-planning issue if TPS headroom < 3× sustained peak
