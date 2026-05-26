# Runbook: Backup and Restore

**Severity:** Standard (backup) / SEV-1 (restore)  
**Estimated time:** Backup: 10 min automated; Restore: 45–90 min  
**Requires:** `kubectl` with prod context, Vault admin access, S3/Spaces access

---

## Backup strategy

| Data | Mechanism | Frequency | Retention |
|------|-----------|-----------|-----------|
| TigerBeetle data files | Kubernetes CronJob → S3 | Every 6 hours | 30 days |
| Vault secrets | Vault snapshot | Daily | 90 days |
| Kubernetes manifests | Git (this repo) | Every commit | Forever |
| Terraform state | DO Spaces backend | Every apply | 30 versions |
| Prometheus metrics | Persistent Volume (30Gi) | N/A — time-series only | 15 days |

---

## 1. Manual TigerBeetle backup

```bash
# Scale down engine to stop writes
kubectl -n blazil scale deployment blazil-engine --replicas=0

# Trigger snapshot on TigerBeetle
kubectl -n blazil exec -it deploy/blazil-tigerbeetle -- \
  ./tigerbeetle backup --output=/data/backup-$(date +%Y%m%d-%H%M%S).snap

# Copy snapshot to S3
kubectl -n blazil exec deploy/blazil-tigerbeetle -- \
  aws s3 cp /data/backup-$(date +%Y%m%d-%H%M%S).snap \
  s3://blazil-backups/tigerbeetle/ --sse aws:kms

# Restore engine
kubectl -n blazil scale deployment blazil-engine --replicas=2
```

---

## 2. Vault backup

```bash
# Create Vault snapshot (requires admin policy)
vault operator raft snapshot save /tmp/vault-$(date +%Y%m%d).snap

# Upload to S3
aws s3 cp /tmp/vault-$(date +%Y%m%d).snap \
  s3://blazil-backups/vault/ --sse aws:kms

# Verify integrity
vault operator raft snapshot inspect /tmp/vault-$(date +%Y%m%d).snap
```

---

## 3. Restore TigerBeetle from snapshot

> This is a destructive operation. Obtain incident commander approval before proceeding.

```bash
# 1. Scale down all services to stop writes
kubectl -n blazil scale deployment --all --replicas=0

# 2. Download the snapshot
aws s3 cp s3://blazil-backups/tigerbeetle/<SNAPSHOT_FILE> /tmp/restore.snap

# 3. Copy snapshot into the TigerBeetle pod
kubectl -n blazil cp /tmp/restore.snap \
  $(kubectl -n blazil get pod -l app=blazil-tigerbeetle -o name | head -1):/data/restore.snap

# 4. Restore
kubectl -n blazil exec -it deploy/blazil-tigerbeetle -- \
  ./tigerbeetle restore --input=/data/restore.snap

# 5. Verify account count matches expected
kubectl -n blazil exec -it deploy/blazil-tigerbeetle -- \
  ./tigerbeetle query-accounts --limit=1

# 6. Re-enable services
kubectl -n blazil scale deployment blazil-engine --replicas=2
kubectl -n blazil scale deployment blazil-payments --replicas=3
# ... repeat for other services
```

---

## 4. Restore Vault from snapshot

```bash
# Seal Vault first
vault operator seal

# Restore
vault operator raft snapshot restore /tmp/vault-<DATE>.snap

# Unseal (requires 3 of 5 unseal keys)
vault operator unseal <KEY_1>
vault operator unseal <KEY_2>
vault operator unseal <KEY_3>

# Verify
vault status
vault kv list secret/blazil/
```

---

## 5. Verify restore integrity

```bash
# Run double-entry invariant check
kubectl -n blazil exec -it deploy/blazil-engine -- \
  curl -s http://localhost:9090/debug/verify-ledger

# Expected output: {"status":"ok","accounts_checked":<N>,"errors":0}
```

---

## Post-restore checklist

- [ ] Double-entry invariant check passes with 0 errors
- [ ] All services return healthy on `/healthz`
- [ ] Grafana shows normal transaction rates
- [ ] Incident ticket updated with restore timestamp and snapshot used
- [ ] Affected customers notified if data loss window applies
