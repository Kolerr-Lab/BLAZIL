# Scaling Runbook

## Current Limits

| Resource | Current | Max | Constraint |
|---|---|---|---|
| Nodes | 3 | Unlimited | TigerBeetle VSR quorum (odd number) |
| Shards | 3 | 8 | `MAX_SHARD_COUNT=8` in engine code |
| Ring buffer | 128K slots/shard | 512MB total | Compile-time assertion |
| TPS (local) | ~1M (Aeron IPC) | — | M4 thermal limit |
| TPS (DO 3-node) | TBD | — | VSR consensus + network |

---

## Scale Up: Increase Droplet Size

The fastest way to get more TPS on existing infrastructure.

```bash
cd infra/terraform/digitalocean

# Change to CPU-optimized droplets (8vCPU/16GB, ~$178/month each)
export TF_VAR_droplet_size="c-8"
terraform apply

# Re-run kernel tuning after resize
cd ../ansible
ansible-playbook -i inventory/production playbooks/site.yml --tags tune
```

---

## Scale Out: Add a 4th Node (Future)

> Note: TigerBeetle VSR is designed for 3, 5, or 6 replicas. Adding a 4th
> requires migrating to a 5-node cluster for odd-number quorum.

```bash
# Step 1: Plan the migration
# - TigerBeetle must be reconfigured with 5 replicas (can't add 1 to existing 3)
# - New cluster: 5 nodes, 5 TB replicas, up to 8 engine shards

# Step 2: Provision 2 additional droplets in Terraform
export TF_VAR_node_count=5  # NOTE: requires modifying validation in variables.tf

# Step 3: Initialize new TB replicas with existing data (see backup-restore.md)

# Step 4: Update BLAZIL_NODES ConfigMap / env to include all 5 nodes

# Step 5: Restart engines with new shard count
export BLAZIL_SHARD_COUNT=5
```

---

## Scale Shards: 3 → 8

The engine supports up to 8 shards (enforced by `MAX_SHARD_COUNT=8`). Shards are
in-memory ring buffers — increasing to 8 requires no hardware change on 16GB nodes.

```bash
# Current: 3 shards (one per node, tied to engine process)
# Future:  8 shards per node (8 ring buffers, 8 handler threads per engine)
# Note: shards and nodes are decoupled — each engine can run multiple shards

# Set via env var (no code change required)
BLAZIL_SHARD_COUNT=8

# Memory check: 8 shards × 128K slots × 56 bytes = ~56MB per engine
# Well within 16GB limit

# Update on all nodes
for NODE in node-1 node-2 node-3; do
  ansible $NODE -i inventory/production -m shell \
    -a "sed -i 's/BLAZIL_SHARD_COUNT=.*/BLAZIL_SHARD_COUNT=8/' /opt/blazil/.env.node"
done

# Rolling restart (see deploy.md for rolling procedure)
```

---

## Horizontal Read Scaling: Read Replicas

For read-heavy workloads (balance queries), add read replicas that connect to
TigerBeetle without write access:

```bash
# TigerBeetle supports read replicas via standby cluster mode
# (Available in TB 0.17+)
# See: https://docs.tigerbeetle.com/operating/cluster
```

---

## Performance Tuning Levers

| Lever | Current | Max | How to change |
|---|---|---|---|
| `AERON_TERM_BUFFER_LENGTH` | 128MB | 512MB (on 16GB nodes) | env var |
| `BLAZIL_SHARD_COUNT` | 3 | 8 | env var |
| `WINDOW_SIZE` in bench | 2048 | unlimited | `bench/src/scenarios/aeron_scenario.rs` |
| Huge pages | 512 × 2MB | 4096 × 2MB | `echo N > /proc/sys/vm/nr_hugepages` |
| CPU governor | performance | — | `do-tune.sh` (already set) |
