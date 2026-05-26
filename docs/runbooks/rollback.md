# Runbook: Emergency Rollback

**Severity**: P0 / P1 depending on impact  
**Estimated time**: 5–15 minutes  
**Requires**: kubectl with prod context

---

## When to rollback

- New deployment causes **error rate > 5%** (alert: `TransactionErrorRateHigh`)
- **ServiceDown** alert fires for any blazil pod within 5 minutes of deployment
- Transaction throughput drops below baseline (alert: `TransactionThroughputLow`)
- Manual smoke test fails after deployment

**Decision rule**: If symptoms started within 30 minutes of a deployment and no
external root cause is identified, roll back first, investigate second.

---

## 1. Identify the failing deployment

```bash
kubectl get pods -n blazil
kubectl get events -n blazil --sort-by='.lastTimestamp' | tail -20
```

---

## 2. Rollback a Deployment (stateless Go services)

```bash
# Roll back to the previous ReplicaSet
kubectl rollout undo deployment/blazil-gateway   -n blazil
kubectl rollout undo deployment/blazil-payments  -n blazil
kubectl rollout undo deployment/blazil-banking   -n blazil
kubectl rollout undo deployment/blazil-trading   -n blazil
kubectl rollout undo deployment/blazil-crypto    -n blazil
kubectl rollout undo deployment/blazil-inference -n blazil

# Or roll back a single service
kubectl rollout undo deployment/<name> -n blazil

# Verify
kubectl rollout status deployment/<name> -n blazil
```

---

## 3. Rollback the Engine StatefulSet (Rust)

The engine StatefulSet manages stateful replicas. Roll back with care.

```bash
kubectl rollout undo statefulset/blazil-engine -n blazil
kubectl rollout status statefulset/blazil-engine -n blazil --timeout=10m
```

**Note**: Engine rollback does NOT affect TigerBeetle data. VSR consensus
ensures in-flight transactions committed before rollback are durable.

---

## 4. TigerBeetle rollback

TigerBeetle uses VSR — **do NOT roll back TigerBeetle** unless explicitly
instructed by the Rust team and all 3 replicas have been taken offline first.

If TigerBeetle pods are crashlooping, see [service-down.md](service-down.md).

---

## 5. Verify after rollback

```bash
# All pods running
kubectl get pods -n blazil

# No recent error events
kubectl get events -n blazil --sort-by='.lastTimestamp' | tail -10

# Metrics recovering
kubectl port-forward svc/prometheus 9090:9090 -n blazil &
curl -sf 'http://localhost:9090/api/v1/query?query=up{job=~"blazil-.*"}' | \
  python3 -m json.tool | grep '"value"'
```

---

## 6. Revert image tags in git

After a successful rollback, update `infra/k8s/overlays/prod/kustomization.yaml`
to the previous known-good SHA and merge a fix PR before the next deployment.

```bash
git log --oneline infra/k8s/overlays/prod/kustomization.yaml | head -5
git revert HEAD  # or manually edit + PR
```

---

## 7. Post-rollback actions

- [ ] Alert `#ops` that rollback is complete
- [ ] Open a postmortem issue in GitHub
- [ ] Document the failed SHA and root cause in CHANGELOG.md under the release
- [ ] Do NOT redeploy the same SHA without a fix
