# Runbook: Blue-Green Deployment

**Severity:** Standard change  
**Estimated time:** 30–60 minutes  
**Requires:** `kubectl` with prod context, write access to GHCR, Vault read access

---

## Overview

Blazil uses a blue-green deployment model to achieve zero-downtime releases. The **blue** environment carries live traffic; the **green** environment receives the new version and is validated before traffic is switched.

---

## Pre-deployment checklist

- [ ] CI is green on the target commit (all tests, `cargo audit`, `trivy scan`)
- [ ] `CHANGELOG.md` updated for this release
- [ ] All database schema migrations applied and verified compatible with both versions
- [ ] New container images built and pushed to GHCR (`ghcr.io/kolerr-lab/`)
- [ ] Cosign signatures verified on all new images (done automatically by CI)
- [ ] Notify `#ops` channel with maintenance window start and estimated duration

---

## 1. Identify current live colour

```bash
kubectl -n blazil get service blazil-gateway-lb -o jsonpath='{.spec.selector.colour}'
# Output: blue   (or green)
export LIVE=blue
export NEXT=green
```

---

## 2. Deploy new version to the inactive colour

```bash
export GIT_SHA=$(git rev-parse --short HEAD)
export REGISTRY=ghcr.io/kolerr-lab

# Patch each deployment's image tag
for svc in gateway payments banking trading crypto inference engine; do
  kubectl -n blazil set image deployment/blazil-${svc}-${NEXT} \
    ${svc}=${REGISTRY}/blazil-${svc}:${GIT_SHA}
done

# Wait for all green pods to be ready
kubectl -n blazil rollout status deployment -l colour=${NEXT} --timeout=300s
```

---

## 3. Run smoke tests against the green environment

```bash
# Port-forward the green gateway (internal ClusterIP)
kubectl -n blazil port-forward svc/blazil-gateway-${NEXT} 50055:50050 &
PF_PID=$!

# Run smoke test suite targeting localhost:50055
cargo test --package blazil-bench -- smoke --ignored

kill $PF_PID
```

If any smoke test fails, abort: do not switch traffic. Investigate and redeploy to green.

---

## 4. Switch traffic

```bash
# Patch the public LoadBalancer service selector
kubectl -n blazil patch service blazil-gateway-lb \
  -p "{\"spec\":{\"selector\":{\"colour\":\"${NEXT}\"}}}"

# Confirm
kubectl -n blazil get service blazil-gateway-lb -o jsonpath='{.spec.selector.colour}'
# Expected: green
```

---

## 5. Monitor for 10 minutes

Watch error rate and latency in Grafana (`/d/blazil-overview`):

- `http_server_requests_errors_total` — should not spike
- `transaction_p99_latency_ms` — should remain < 5 ms
- `engine_queue_depth` — should not grow unbounded

If anomalies appear, rollback immediately (§6).

---

## 6. Rollback (if needed)

```bash
kubectl -n blazil patch service blazil-gateway-lb \
  -p "{\"spec\":{\"selector\":{\"colour\":\"${LIVE}\"}}}"

# Confirm rollback
kubectl -n blazil get service blazil-gateway-lb -o jsonpath='{.spec.selector.colour}'
# Expected: blue
```

---

## 7. Clean up old colour (after 24h stability)

```bash
# Scale down old colour
kubectl -n blazil scale deployment -l colour=${LIVE} --replicas=0
```

Do not delete deployments yet — keep them at zero replicas for 7 days in case a late rollback is needed.

---

## Post-deployment checklist

- [ ] Grafana dashboards show normal metrics for 30 minutes
- [ ] Alert manager shows no new firing alerts
- [ ] `#ops` channel notified with deployment completion and SHA
- [ ] `CHANGELOG.md` entry linked in the release tag
