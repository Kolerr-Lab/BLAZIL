# Backup and Restore Runbook

## TigerBeetle Backup

TigerBeetle VSR maintains its own replication — losing one node doesn't require
a manual backup restore. Manual backups are needed for:
- Disaster recovery (all 3 nodes lost simultaneously)
- Migrating to a new cluster
- Pre-upgrade snapshots

### Backup Procedure

TigerBeetle's data file is a single flat file at `/data/0_0.tigerbeetle` (path
set by the entrypoint script). Backup = copy this file while TB is not running,
OR use TB's built-in `--backup` flag (TB 0.17+).

```bash
# Option A: Cold backup (stop TB first — guarantees consistency)
# WARNING: brief downtime. VSR tolerates 1 node down — rotate through nodes.

# Step 1: Stop TB on the node being backed up
ssh root@$NODE_IP "docker stop blazil-tigerbeetle-<N>"

# Step 2: Copy data file to backup location
ssh root@$NODE_IP "docker run --rm \
  -v blazil-tb-data-<N>:/data:ro \
  -v /backup:/backup \
  alpine sh -c 'cp /data/0_0.tigerbeetle /backup/tb-$(date +%Y%m%d-%H%M%S).tigerbeetle'"

# Step 3: Upload to DO Spaces (optional, recommended for DR)
# doctl compute cdn flush ...
# OR: rclone copy /backup/ spaces:blazil-backups/

# Step 4: Restart TB
ssh root@$NODE_IP "docker start blazil-tigerbeetle-<N>"

# Step 5: Verify VSR re-join
sleep 30
ssh root@$NODE_IP "docker logs blazil-tigerbeetle-<N> --tail=20"
```

### Automated Daily Backup Script

```bash
#!/bin/bash
# Run as cron: 0 2 * * * /opt/blazil/scripts/backup.sh >> /var/log/blazil-backup.log 2>&1

set -e
BACKUP_DIR="/backup/blazil-$(date +%Y%m%d)"
mkdir -p "$BACKUP_DIR"

# Stop TB, copy, restart (takes ~5s, within VSR tolerance)
docker stop blazil-tigerbeetle-0
docker run --rm \
  -v blazil-tb-data-0:/data:ro \
  -v "$BACKUP_DIR":/backup \
  alpine cp /data/0_0.tigerbeetle /backup/tb-replica-0.tigerbeetle
docker start blazil-tigerbeetle-0

echo "Backup complete: $BACKUP_DIR"
ls -lh "$BACKUP_DIR"
```

---

## Restore Procedure

### Restore single node from backup

```bash
# 1. Stop the node's TigerBeetle
ssh root@$NODE_IP "docker stop blazil-tigerbeetle-<N>"

# 2. Replace data file
ssh root@$NODE_IP "docker run --rm \
  -v blazil-tb-data-<N>:/data \
  -v /backup/<BACKUP_FILE>:/backup/tb.tigerbeetle:ro \
  alpine cp /backup/tb.tigerbeetle /data/0_0.tigerbeetle"

# 3. Restart TigerBeetle — VSR will sync from other replicas automatically
ssh root@$NODE_IP "docker start blazil-tigerbeetle-<N>"

# 4. Wait for sync and verify
sleep 60
ssh root@$NODE_IP "docker logs blazil-tigerbeetle-<N> --tail=30"
```

### Full cluster restore (disaster recovery)

> Use only when ALL 3 nodes are lost. This replays from the backup point.
> Transactions since the last backup are unrecoverable unless you have WAL logs.

```bash
# 1. Provision fresh cluster (terraform apply)
# 2. Copy backup file to all 3 nodes (use any one replica's backup — VSR will replicate)
for NODE in $NODE1_IP $NODE2_IP $NODE3_IP; do
  scp /backup/tb-replica-0.tigerbeetle root@$NODE:/tmp/
  ssh root@$NODE "docker run --rm \
    -v blazil-tb-data-0:/data \
    -v /tmp/tb-replica-0.tigerbeetle:/backup/tb.tigerbeetle:ro \
    alpine cp /backup/tb.tigerbeetle /data/0_0.tigerbeetle"
done

# 3. Start cluster (TB will detect shared state and form quorum)
cd infra/ansible
ansible-playbook -i inventory/production playbooks/site.yml --tags start
```

---

## Backup Retention Policy

| Type | Retention | Storage |
|---|---|---|
| Daily cold backup | 7 days | Local `/backup/` |
| Weekly snapshot | 30 days | DO Spaces |
| Pre-upgrade backup | Indefinite | DO Spaces (tagged by version) |

---

## Verify Backup Integrity

```bash
# TigerBeetle can verify a data file without starting the full cluster
docker run --rm \
  -v /backup/tb-YYYYMMDD.tigerbeetle:/data/0_0.tigerbeetle \
  ghcr.io/tigerbeetle/tigerbeetle:0.16.72 \
  inspect /data/0_0.tigerbeetle
```
