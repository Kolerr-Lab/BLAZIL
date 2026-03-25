# Blazil Runbooks

Operational procedures for the Blazil payment engine cluster.

## Index

| Runbook | When to use |
|---|---|
| [deploy.md](deploy.md) | Deploy a new version to DO cluster |
| [benchmark.md](benchmark.md) | Run the official TPS benchmark |
| [incident-response.md](incident-response.md) | Node down, TigerBeetle crash, OOM |
| [scaling.md](scaling.md) | Add nodes, increase shards |
| [backup-restore.md](backup-restore.md) | TigerBeetle data backup and restore |

## Cluster Overview

```
                    ┌─────────────────────────────────┐
                    │         DO Private VPC           │
                    │         10.10.0.0/24             │
  ┌─────────────────┴─────────────────┐               │
  │  node-1 (10.10.0.1)              │               │
  │  ├── tigerbeetle-0  :3000         │               │
  │  ├── blazil-engine  :7878 shard=0 │               │
  │  ├── payments       :50051        │               │
  │  ├── banking        :50052        │               │
  │  ├── trading        :50053        │               │
  │  ├── crypto         :50054        │               │
  │  ├── prometheus     :9090         │               │
  │  └── grafana        :3001  ◄──────┼── public      │
  └──────────────────────────────────┘               │
  ┌──────────────────────────────────┐               │
  │  node-2 (10.10.0.2)              │               │
  │  ├── tigerbeetle-1  :3001         │               │
  │  ├── blazil-engine  :7878 shard=1 │               │
  │  ├── payments       :50061        │               │
  │  ├── banking        :50062        │               │
  │  ├── trading        :50063        │               │
  │  └── crypto         :50064        │               │
  └──────────────────────────────────┘               │
  ┌──────────────────────────────────┐               │
  │  node-3 (10.10.0.3)              │               │
  │  ├── tigerbeetle-2  :3002         │               │
  │  ├── blazil-engine  :7878 shard=2 │               │
  │  ├── payments       :50071        │               │
  │  ├── banking        :50072        │               │
  │  ├── trading        :50073        │               │
  │  └── crypto         :50074        │               │
  └──────────────────────────────────┘               │
                    └─────────────────────────────────┘
```

## Quick Reference

```bash
# SSH to any node
ssh root@<PUBLIC_IP>

# Tail all logs on a node
docker compose -f /opt/blazil/infra/docker/docker-compose.node-<N>.yml logs -f

# Check TigerBeetle health
docker ps | grep tigerbeetle

# Check engine metrics
curl -s http://localhost:9090/metrics | grep blazil_tps

# Restart a single service
docker compose -f /opt/blazil/infra/docker/docker-compose.node-<N>.yml restart blazil-engine

# View Grafana dashboard
open http://<NODE_1_PUBLIC_IP>:3001  # admin / blazil (change in production)
```
