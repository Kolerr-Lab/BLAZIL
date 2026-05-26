# Runbook: Production Deployment

**Severity**: Standard change  
**Estimated time**: 20–40 minutes  
**Requires**: access to ghcr.io, kubectl with prod context, Vault write access

---

## Pre-deployment checklist

- [ ] CI is green on the target commit (`cargo audit`, `trivy-scan`, all tests)
- [ ] `CHANGELOG.md` updated with this release
- [ ] All required Kubernetes Secrets exist in the cluster (see §4)
- [ ] Peer review approved, PR merged to `main`
- [ ] Notify `#ops` channel with maintenance window start time

---

## 1. Set context

```bash
# Confirm you are targeting the correct cluster
kubectl config current-context
kubectl config use-context blazil-prod   # adjust to your context name

# Confirm namespace
kubectl get ns blazil
```

---

## 2. Build and push Docker images

Run in CI or manually:

```bash
export GIT_SHA=$(git rev-parse --short HEAD)
export REGISTRY=ghcr.io/kolerr-lab

# Rust services (built together from repo root)
docker build -f infra/docker/Dockerfile.engine   -t $REGISTRY/blazil-engine:$GIT_SHA   .
docker build -f infra/docker/Dockerfile.inference -t $REGISTRY/blazil-inference:$GIT_SHA .

# Go services (each has its own Dockerfile)
docker build -f infra/docker/Dockerfile.gateway  -t $REGISTRY/blazil-gateway:$GIT_SHA  .
docker build -f infra/docker/Dockerfile.payments  -t $REGISTRY/blazil-payments:$GIT_SHA  .
docker build -f infra/docker/Dockerfile.banking   -t $REGISTRY/blazil-banking:$GIT_SHA   .
docker build -f infra/docker/Dockerfile.trading   -t $REGISTRY/blazil-trading:$GIT_SHA   .
docker build -f infra/docker/Dockerfile.crypto    -t $REGISTRY/blazil-crypto:$GIT_SHA    .

# Push all
for svc in engine inference gateway payments banking trading crypto; do
  docker push $REGISTRY/blazil-$svc:$GIT_SHA
done
```

---

## 3. Update image tags in prod overlay

Edit `infra/k8s/overlays/prod/kustomization.yaml` and set `newTag` for every
changed service to `$GIT_SHA`. Example:

```yaml
images:
  - name: ghcr.io/kolerr-lab/blazil-engine
    newTag: "abc1234"
  - name: ghcr.io/kolerr-lab/blazil-gateway
    newTag: "abc1234"
  # ... all other services
```

Commit this change to `main` or the release branch before applying.

---

## 4. Ensure Kubernetes Secrets exist

These Secrets are **not** stored in git. Create them once per cluster.

```bash
# Gateway — PostgreSQL connection + admin token
kubectl create secret generic gateway-secret \
  --namespace blazil \
  --from-literal=database-url='postgres://blazil:<password>@<host>:5432/blazil?sslmode=require' \
  --from-literal=admin-token='<strong-random-token>' \
  --dry-run=client -o yaml | kubectl apply -f -

# Inference — API key for HTTP endpoint auth
kubectl create secret generic inference-secret \
  --namespace blazil \
  --from-literal=api-key='<strong-random-api-key>' \
  --dry-run=client -o yaml | kubectl apply -f -
```

Verify:

```bash
kubectl get secret gateway-secret inference-secret -n blazil
```

---

## 5. Apply with Kustomize

```bash
# Dry-run first — review the diff
kubectl diff -k infra/k8s/overlays/prod/

# Apply
kubectl apply -k infra/k8s/overlays/prod/
```

---

## 6. Monitor rollout

```bash
# Watch all deployments roll
kubectl rollout status deployment/blazil-gateway   -n blazil --timeout=5m
kubectl rollout status deployment/blazil-payments  -n blazil --timeout=5m
kubectl rollout status deployment/blazil-banking   -n blazil --timeout=5m
kubectl rollout status deployment/blazil-trading   -n blazil --timeout=5m
kubectl rollout status deployment/blazil-crypto    -n blazil --timeout=5m
kubectl rollout status deployment/blazil-inference -n blazil --timeout=5m

# StatefulSets
kubectl rollout status statefulset/blazil-engine      -n blazil --timeout=10m
kubectl rollout status statefulset/tigerbeetle         -n blazil --timeout=10m
```

---

## 7. Smoke test

```bash
GATEWAY_ADDR=$(kubectl get svc blazil-gateway -n blazil \
  -o jsonpath='{.status.loadBalancer.ingress[0].ip}')

# gRPC health (requires grpc_health_probe or grpcurl)
grpcurl -plaintext $GATEWAY_ADDR:50050 list

# Prometheus metrics accessible
curl -sf http://$GATEWAY_ADDR:9090/metrics | grep 'up'
```

---

## 8. Rollback if needed

See [rollback.md](rollback.md) for immediate rollback steps.

---

## 9. Post-deployment

- [ ] Update `CHANGELOG.md` with deployment timestamp
- [ ] Verify Grafana dashboards show normal metrics
- [ ] Close maintenance window in `#ops`
- [ ] Update Jira/Linear ticket if applicable
