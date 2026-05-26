# Runbook: Service Down

**Severity**: P0 (full outage) or P1 (partial)  
**Trigger**: Alert `ServiceDown` fires for `job=~"blazil-.*"` for > 2 minutes

---

## Quick diagnosis

```bash
kubectl get pods -n blazil -o wide
kubectl get events -n blazil --sort-by='.lastTimestamp' | tail -20
```

---

## By service type

---

### blazil-gateway

Gateway is the external entry point. If it is down, **all client traffic is blocked**.

```bash
# Check gateway pod state
kubectl get pods -l app=blazil-gateway -n blazil
kubectl describe pod -l app=blazil-gateway -n blazil | grep -A 10 "Conditions\|Events"

# Logs
kubectl logs -l app=blazil-gateway -n blazil --tail=100

# Common causes:
# 1. gateway-secret missing (GATEWAY_DATABASE_URL / GATEWAY_ADMIN_TOKEN)
kubectl get secret gateway-secret -n blazil

# 2. Postgres unreachable (database migration fails at startup → exit 1)
#    Check GATEWAY_DATABASE_URL points to a live Postgres instance.

# 3. OOMKilled — increase memory limit in prod overlay
kubectl describe pod -l app=blazil-gateway -n blazil | grep -i "oom\|killed"

# Recovery
kubectl rollout restart deployment/blazil-gateway -n blazil
kubectl rollout status  deployment/blazil-gateway -n blazil --timeout=5m
```

---

### blazil-payments / blazil-banking / blazil-trading / blazil-crypto

These services process domain-specific transactions via the Rust engine.

```bash
SERVICE=blazil-payments   # change per affected service

kubectl get pods -l app=$SERVICE -n blazil
kubectl logs -l app=$SERVICE -n blazil --tail=200 --since=30m

# Common causes:
# 1. Engine unreachable — check engine pods
kubectl get pods -l app=blazil-engine -n blazil

# 2. Auth failure (BLAZIL_AUTH_REQUIRED=true but KEYCLOAK_URL not set/reachable)
kubectl get configmap blazil-cluster -n blazil -o yaml | grep AUTH

# 3. gRPC crash on startup — check if port 50051 is already bound
kubectl describe pod -l app=$SERVICE -n blazil | grep "Port already in use"

# Recovery
kubectl rollout restart deployment/$SERVICE -n blazil
kubectl rollout status  deployment/$SERVICE -n blazil --timeout=5m
```

---

### blazil-engine (StatefulSet)

The engine is the Rust core that writes to TigerBeetle. Downtime here means
no transactions can be committed.

```bash
kubectl get pods -l app=blazil-engine -n blazil
kubectl logs blazil-engine-0 -n blazil --tail=200
kubectl logs blazil-engine-0 -n blazil --previous  # if restarting

# Common causes:
# 1. TigerBeetle unreachable — engine panics/exits if TB is not reachable at startup
kubectl get pods -l app=tigerbeetle -n blazil

# 2. io_uring not supported on kernel < 5.11 — check node kernel version
kubectl get nodes -o wide | awk '{print $1, $5}'
# If kernel < 5.11, set BLAZIL_TRANSPORT=tcp in configmap

# 3. Aeron media driver port conflict
kubectl logs blazil-engine-0 -n blazil --tail=50 | grep -i "aeron\|media driver"

# Recovery — rolling restart (safe, StatefulSet rolls one pod at a time)
kubectl rollout restart statefulset/blazil-engine -n blazil
kubectl rollout status  statefulset/blazil-engine -n blazil --timeout=10m
```

---

### TigerBeetle (StatefulSet)

TigerBeetle runs a 3-replica VSR cluster. It can tolerate 1 replica failure
(`f=1`, requires 2/3 quorum). Losing 2 replicas stops all commits.

```bash
kubectl get pods -l app=tigerbeetle -n blazil
kubectl logs tigerbeetle-0 -n blazil --tail=100
kubectl logs tigerbeetle-1 -n blazil --tail=100
kubectl logs tigerbeetle-2 -n blazil --tail=100

# How many replicas are up?
kubectl get pods -l app=tigerbeetle -n blazil | grep Running | wc -l
# Must be >= 2 for VSR to make progress.

# Common causes:
# 1. Data volume full
kubectl exec tigerbeetle-0 -n blazil -- df -h /data

# 2. hostNetwork port conflict (TigerBeetle uses hostNetwork for VSR IPs)
kubectl describe pod tigerbeetle-0 -n blazil | grep "host port"

# Recovery — restart single replica (never restart all 3 simultaneously)
kubectl delete pod tigerbeetle-0 -n blazil
# Wait for it to rejoin VSR before restarting the next one.
kubectl wait pod/tigerbeetle-0 -n blazil --for=condition=Ready --timeout=2m
```

**⚠️ NEVER** run `kubectl delete pod -l app=tigerbeetle` — this removes all 3
replicas simultaneously and will halt VSR consensus permanently until pods come
back up. Data is safe (PVC-backed), but zero commits will succeed during the
window.

---

### blazil-inference

Inference serves HTTP requests for ML tenants. Its failure is isolated and does
not affect payment processing.

```bash
kubectl get pods -l app=blazil-inference -n blazil
kubectl logs -l app=blazil-inference -n blazil --tail=100

# Common causes:
# 1. inference-secret missing (BLAZIL_INFERENCE_API_KEY)
kubectl get secret inference-secret -n blazil

# 2. /health endpoint not responding — Aeron IPC failed to start
#    The inference-server still starts the HTTP API even if Aeron init fails.
#    Check for: 'aeron' or 'media driver' in logs.

# 3. OOMKilled — ML model loaded into RAM, may need higher limit
kubectl describe pod -l app=blazil-inference -n blazil | grep -i oom

# Recovery
kubectl rollout restart deployment/blazil-inference -n blazil
kubectl rollout status  deployment/blazil-inference -n blazil --timeout=5m
```

---

## If restart doesn't help

1. Check if a recent config change broke the service:
   ```bash
   kubectl get configmap blazil-cluster -n blazil -o yaml
   ```

2. Check if the image is corrupted or missing:
   ```bash
   kubectl describe pod <pod-name> -n blazil | grep "ErrImagePull\|ImagePullBackOff"
   ```

3. If the image is bad — rollback to the previous image tag (see [rollback.md](rollback.md)).

4. Escalate to the service owner (see `CODEOWNERS`) if unresolved after 15 minutes.
