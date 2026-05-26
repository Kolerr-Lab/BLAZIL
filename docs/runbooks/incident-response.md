# Runbook: Incident Response

**Applies to**: All Blazil production incidents  
**On-call rotation**: `#oncall` Slack channel

---

## Severity levels

| Severity | Definition | Response SLA | Examples |
|----------|-----------|-------------|---------|
| **P0** | Complete service outage; all transactions failing | 15 min | TigerBeetle down, engine crash-loop, gateway unreachable |
| **P1** | Partial outage or degradation; >5% error rate | 30 min | One service down, high latency >1s p99 |
| **P2** | Minor degradation; <5% error rate or non-critical path | 2 hours | Metrics missing, one replica unhealthy |
| **P3** | No user impact; informational | Next business day | Certificate expiry warning, disk >70% |

---

## 1. Acknowledge the incident

1. Claim ownership in `#oncall`: `@here I am taking this — <your name>`
2. Start a war-room thread in `#incidents` with title: `[P<N>] <brief description> <date>`
3. Set a 15-minute timer for the first status update

---

## 2. Triage (first 5 minutes)

```bash
# Pod status
kubectl get pods -n blazil -o wide

# Recent events
kubectl get events -n blazil --sort-by='.lastTimestamp' | tail -30

# Active Prometheus alerts
kubectl port-forward svc/prometheus 9090:9090 -n blazil &
curl -sf 'http://localhost:9090/api/v1/alerts' | python3 -m json.tool | grep '"state"'
```

**Quick signal matrix**:

| Symptom | Likely cause | Runbook |
|---------|-------------|---------|
| `blazil-*` pod CrashLoopBackOff after deploy | Bad image | [rollback.md](rollback.md) |
| All engine pods `Pending` | Insufficient node resources | Check node capacity |
| `TransactionErrorRateHigh` alert | Service logic error or downstream | [service-down.md](service-down.md) |
| `RingBufferHighUtilization` | Engine back-pressure / slow TB | [high-latency.md](high-latency.md) |
| TigerBeetle pods restarting | VSR quorum lost | [service-down.md](service-down.md) |

---

## 3. Contain

Choose the appropriate action based on triage:

**Option A — Rollback** (if incident started after a deployment):

```bash
kubectl rollout undo deployment/blazil-<service> -n blazil
```

See [rollback.md](rollback.md) for full steps.

**Option B — Scale down the failing service** (stops bleeding, buys time):

```bash
kubectl scale deployment blazil-<service> --replicas=0 -n blazil
# Restore once the issue is identified
kubectl scale deployment blazil-<service> --replicas=2 -n blazil
```

**Option C — Circuit break via gateway** (disable route to one backend):

```bash
# Remove the failing upstream from gateway routes
kubectl set env deployment/blazil-gateway \
  BLAZIL_<SERVICE>_GRPC_ADDR="-" -n blazil
```

---

## 4. Diagnose

```bash
# Logs — last 200 lines from all pods of a deployment
kubectl logs -l app=blazil-<service> -n blazil --tail=200 --since=30m

# Logs from a specific crashing pod
kubectl logs <pod-name> -n blazil --previous

# Describe pod for OOMKilled, resource pressure, probe failures
kubectl describe pod <pod-name> -n blazil

# TigerBeetle VSR status
kubectl logs -l app=tigerbeetle -n blazil --tail=100 | grep -i "vsr\|replica\|error"

# Engine ring buffer
kubectl logs -l app=blazil-engine -n blazil --tail=100 | grep -i "ring\|disruptor\|overflow"
```

---

## 5. Resolve

Apply the fix:

```bash
# If a config change:
kubectl apply -k infra/k8s/overlays/prod/

# If a code fix — deploy new image following deployment.md §2-6
```

Verify recovery:

```bash
kubectl get pods -n blazil
# All Running/Ready

curl -sf 'http://localhost:9090/api/v1/query?query=rate(blazil_transactions_total[5m])' \
  | python3 -m json.tool
# Rate > 0
```

---

## 6. Communicate

**During incident** (every 15 min P0, every 30 min P1):

```
[Status Update] <time>
Impact: <what users see>
Status: Investigating / Identified / Mitigating / Resolved
Next update: <time>
```

**Resolution message**:

```
[RESOLVED] <time>
Root cause: <brief>
Fix applied: <what was done>
Duration: <start> → <end>
Follow-up: postmortem issue #<N>
```

---

## 7. Post-incident

- [ ] Open a postmortem issue within 24 hours (template: `.github/ISSUE_TEMPLATE/`)
- [ ] 5 whys analysis
- [ ] Action items with owners and due dates
- [ ] Update this runbook if a step was missing or unclear
