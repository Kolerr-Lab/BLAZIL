# Runbook: Alert Runbook

**Severity:** Reference  
**Requires:** Grafana access, `kubectl` with prod context

---

## Purpose

This runbook maps each Prometheus alert to its diagnosis steps and resolution actions.  
For each alert: verify the signal is real, identify the root cause, and apply the remediation.

---

## CRITICAL alerts

---

### `EngineDown`

**Meaning:** No engine pods are `Ready` for > 2 minutes.  
**Impact:** All transaction processing halted.

```bash
# Check pod state
kubectl -n blazil get pods -l app=blazil-engine

# Check recent crash logs
kubectl -n blazil logs deploy/blazil-engine --previous | tail -50

# Check events for OOM or scheduling failures
kubectl -n blazil describe pod -l app=blazil-engine | grep -A5 Events
```

**Remediation:**

- OOMKilled → increase memory limit in `infra/k8s/base/engine/deployment.yaml` and redeploy.
- CrashLoopBackOff → check logs for panic message; likely a configuration error or TigerBeetle connectivity issue.
- Pending (not scheduled) → check node resources with `kubectl top nodes`.

---

### `TigerBeetleUnreachable`

**Meaning:** Engine cannot connect to TigerBeetle for > 1 minute.  
**Impact:** All ledger writes failing; transactions rejected.

```bash
# Check TigerBeetle pod
kubectl -n blazil get pods -l app=blazil-tigerbeetle
kubectl -n blazil logs deploy/blazil-tigerbeetle | tail -50

# Test connectivity from engine pod
kubectl -n blazil exec -it deploy/blazil-engine -- \
  nc -zv blazil-tigerbeetle 3001
```

**Remediation:**

- Pod not running → restart: `kubectl -n blazil rollout restart deployment/blazil-tigerbeetle`
- Network policy blocking → check `kubectl -n blazil get networkpolicy`
- Data file corruption → see `data-corruption.md`

---

### `HighTransactionErrorRate`

**Meaning:** Transaction error rate > 1% over 5 minutes.  
**Impact:** Revenue impact; customer-facing failures.

```bash
# Identify error types
kubectl -n blazil logs deploy/blazil-engine --since=15m \
  | grep 'ERROR\|WARN' | sort | uniq -c | sort -rn | head -20

# Check ledger error codes
kubectl -n blazil exec -it deploy/blazil-engine -- \
  curl -s http://localhost:9090/metrics | grep 'ledger_error'
```

**Common error codes:**

| Code | Meaning | Action |
|------|---------|--------|
| `exceeds_debit_reserved` | Insufficient balance | Normal — surface to user |
| `linked_event_failed` | Linked transfer chain broken | Investigate linked transfer IDs |
| `timestamp_must_advance` | Clock skew between replicas | Check NTP sync on nodes |
| `exists_with_different_flags` | Duplicate transfer ID with different flags | Idempotency key collision — investigate caller |

---

## WARNING alerts

---

### `EngineQueueDepthHigh`

**Meaning:** Ring buffer occupancy > 80% for > 30 seconds.  
**Impact:** Increased latency; risk of dropped transactions if 100% reached.

```bash
# Check processing rate
kubectl -n blazil exec -it deploy/blazil-engine -- \
  curl -s http://localhost:9090/metrics | grep 'engine_event_processing_rate'

# Check TigerBeetle write latency
kubectl -n blazil exec -it deploy/blazil-tigerbeetle -- \
  curl -s http://localhost:9090/metrics | grep 'tb_commit_latency'
```

**Remediation:**

- TigerBeetle I/O stall → check disk I/O: `kubectl -n blazil exec deploy/blazil-tigerbeetle -- iostat -x 1 5`
- Burst traffic → if persistent, scale engine (see `scaling.md`)

---

### `CrossShard2PCVoidRateHigh`

**Meaning:** > 5% of cross-shard 2PC reservations are being voided.  
**Impact:** Degraded cross-shard transfer reliability.

```bash
# Check which shard is failing
kubectl -n blazil logs deploy/blazil-payments --since=30m \
  | grep '2pc\|void\|commit' | tail -100
```

**Common causes:**

- Target shard engine is down → see `EngineDown`
- Network timeout between payments and engine → increase `BLAZIL_TCP_TIMEOUT_MS` env var
- Orphan pending transfers from previous crash → see `data-corruption.md §3a`

---

### `PVCUsageHigh`

**Meaning:** A PersistentVolumeClaim is > 80% full.  
**Impact:** Prometheus or TigerBeetle may stop writing data.

```bash
kubectl -n blazil get pvc
kubectl -n monitoring get pvc

# Identify which PVC is affected from the alert label
kubectl -n <namespace> describe pvc <pvc-name>
```

**Remediation:**

- Prometheus: reduce retention (`--storage.tsdb.retention.time=10d`) or increase PVC size via `kubectl edit pvc`.
- TigerBeetle: add storage (requires node restart with larger volume).

---

### `CertificateExpiringSoon`

**Meaning:** A cert-manager Certificate will expire within 7 days.  
**Impact:** mTLS connections will fail after expiry.

```bash
kubectl -n blazil get certificate
kubectl -n blazil describe certificate <NAME>

# Force renewal
kubectl -n blazil delete certificaterequest <NAME>-<HASH>
# cert-manager will automatically re-issue
```

---

## INFO alerts

---

### `PodRestartFrequent`

**Meaning:** A pod has restarted > 5 times in 1 hour.  
**Impact:** Potential instability; investigate during business hours.

```bash
kubectl -n blazil get pods --sort-by='.status.containerStatuses[0].restartCount'
kubectl -n blazil logs <POD_NAME> --previous | tail -30
```

---

## Escalation path

1. **On-call engineer** — first responder; handle SEV-2 and below.
2. **Incident commander** — required for SEV-1 (financial data integrity, full outage).
3. **TigerBeetle support** — for database-level issues not resolvable by the on-call team.

All incidents must be logged in the incident tracker with timeline, actions taken, and resolution.
