# Runbook: Scaling

**Severity:** Standard operational change  
**Estimated time:** 15–30 minutes per dimension  
**Requires:** `kubectl` with prod context, Terraform write access (for node scaling)

---

## Overview

Blazil has three independently scalable layers:

| Layer | Component | Scaling mechanism |
|-------|-----------|-------------------|
| Gateway / API | `blazil-gateway` | Kubernetes HPA (CPU + custom metric) |
| Engine (hot path) | `blazil-engine` | Manual replicas (stateful — single writer) |
| TigerBeetle ledger | `blazil-tigerbeetle` | Cluster node addition (requires data migration) |
| Go microservices | `payments`, `banking`, etc. | Kubernetes HPA |
| Kubernetes nodes | DigitalOcean node pool | Terraform |

---

## 1. Scale Go microservices (horizontal)

```bash
# Check current HPA state
kubectl -n blazil get hpa

# Manually adjust max replicas if HPA is not keeping up
kubectl -n blazil patch hpa blazil-payments-hpa \
  -p '{"spec":{"maxReplicas":20}}'

# Or adjust CPU target (%)
kubectl -n blazil patch hpa blazil-payments-hpa \
  -p '{"spec":{"targetCPUUtilizationPercentage":60}}'
```

---

## 2. Scale the engine (vertical or replica)

The engine uses a single Disruptor ring buffer — multiple engine replicas require sharding at the TCP load balancer level. Before scaling to > 1 replica, confirm the load balancer (nginx or Envoy) is configured with **session affinity** so a given account's transfers always land on the same engine replica.

```bash
# Scale engine replicas (only if session affinity is confirmed)
kubectl -n blazil scale deployment blazil-engine --replicas=2

# Monitor ring buffer queue depth (should not grow)
kubectl -n blazil exec -it deploy/blazil-engine -- \
  curl -s http://localhost:9090/metrics | grep engine_queue_depth
```

If vertical scaling is needed (memory/CPU), edit the Deployment resource limits in `infra/k8s/base/engine/deployment.yaml` and apply via `kubectl apply -k infra/k8s/base/`.

---

## 3. Scale Kubernetes node pool (Terraform)

```bash
cd infra/terraform/digitalocean

# Edit node pool size in variables
# node_pool_size = 5   →   node_pool_size = 8

terraform plan -var-file=prod.tfvars
# Review plan — should show only node pool count change

terraform apply -var-file=prod.tfvars
```

Wait for new nodes to join the cluster:

```bash
kubectl get nodes -w
# Wait until all new nodes show Ready
```

---

## 4. Add a TigerBeetle shard (advanced)

> This operation requires a maintenance window. Coordinate with all stakeholders.

Adding a shard changes the `account_id % num_shards` routing function, which means existing accounts must be reassigned. The process is:

1. Deploy new TigerBeetle node/cluster.
2. Freeze new account creation (prevent routing inconsistency).
3. Run the shard migration tool (`tools/loadgen/shard-migrate`) to export accounts from old shards and import to new shard assignments.
4. Update `BLAZIL_SHARD_COUNT` environment variable in all engine and payments deployments.
5. Roll deployments to pick up the new shard count.
6. Unfreeze account creation.

See ADR 0006 for the sharding design rationale.

---

## 5. Scale-in (reduce replicas)

```bash
# Graceful scale-in: Kubernetes drains in-flight requests before terminating
kubectl -n blazil scale deployment blazil-payments --replicas=3

# Verify no requests were dropped
kubectl -n blazil logs deploy/blazil-payments --since=5m | grep -c 'error'
```

Do not scale the engine below 1 replica while there is active traffic.

---

## Monitoring during scaling

Watch these metrics in Grafana during and after any scaling event:

- `transaction_p99_latency_ms` — should not increase after scale-out
- `engine_queue_depth` — should decrease after scale-out
- `kube_pod_container_resource_requests` — verify new pods have correct resource limits
- `node_cpu_usage_seconds_total` — confirm load distributes across new nodes
