# Runbook: Data Corruption Response

**Severity:** SEV-1 (financial data integrity)  
**Estimated time:** Variable — do not rush  
**Requires:** `kubectl` with prod context, TigerBeetle admin access, Vault admin access, incident commander

> **STOP.** Before taking any action, page the on-call incident commander and a second engineer. No data modifications should be made by a single person. All actions must be logged with timestamps.

---

## 1. Detect and scope

### 1a. Identify the symptom

Common triggers:

| Symptom | Likely cause |
|---------|-------------|
| Balance check fails double-entry invariant | Partially applied transaction |
| Duplicate `transfer_id` in TigerBeetle | Idempotency key collision or clock skew |
| Account balance < 0 when overdraft not permitted | Missing debit-side constraint |
| `pending_transfer_id` references non-existent transfer | Orphan 2PC Phase 1 with crashed coordinator |

### 1b. Identify affected accounts

```bash
# Pull the TigerBeetle audit log for the last 1 hour
kubectl -n blazil exec -it deploy/blazil-tigerbeetle -- \
  ./tigerbeetle query-transfers --account-id=<ACCOUNT_ID> --limit=1000
```

Record every affected `account_id` and `transfer_id` in the incident ticket.

### 1c. Pause ingress (stop new transactions)

```bash
# Scale down the engine to prevent new writes
kubectl -n blazil scale deployment blazil-engine --replicas=0

# Confirm no active connections
kubectl -n blazil get pods -l app=blazil-engine
```

Do **not** scale down TigerBeetle — it must remain available for the investigation and repair.

---

## 2. Preserve evidence

```bash
# Export affected account state
kubectl -n blazil exec -it deploy/blazil-tigerbeetle -- \
  ./tigerbeetle export-accounts --ids=<COMMA_SEPARATED_IDS> \
  > /tmp/account-snapshot-$(date +%s).json

# Export audit ledger
kubectl -n blazil exec -it deploy/blazil-ledger -- \
  curl -s http://localhost:9090/metrics > /tmp/ledger-metrics-$(date +%s).txt

# Capture current engine pod logs
kubectl -n blazil logs deploy/blazil-engine --since=2h > /tmp/engine-$(date +%s).log
```

Upload all artefacts to the incident storage bucket before proceeding.

---

## 3. Reconcile

### 3a. Void orphan pending transfers

TigerBeetle expires pending transfers automatically after the configured timeout (30 s default). If the transfer has already expired, no action is needed. If it is still pending:

```bash
# Submit a void request via the payments service debug endpoint
curl -X POST http://payments-svc:8080/debug/void-pending \
  -H 'Content-Type: application/json' \
  -d '{"transfer_id":"<UUID>"}'
```

### 3b. Manual balance correction

Manual corrections require a compensating double-entry transfer. Under **no circumstances** should raw account balances be patched directly in TigerBeetle — the ledger is append-only.

```bash
# Use the reconciliation CLI (built from tools/loadgen)
./tools/loadgen/reconcile \
  --debit-account=<ACCOUNT_ID> \
  --credit-account=<SUSPENSE_ACCOUNT_ID> \
  --amount=<AMOUNT_IN_MINOR_UNITS> \
  --code=9999 \
  --reason="corruption-remediation-INCIDENT-<ID>"
```

All reconciliation transfers must use code `9999` (reserved for adjustments) so they are excluded from normal reporting.

---

## 4. Restore service

```bash
# Re-enable the engine
kubectl -n blazil scale deployment blazil-engine --replicas=2

# Watch startup
kubectl -n blazil rollout status deployment/blazil-engine --timeout=120s

# Confirm health
kubectl -n blazil exec -it deploy/blazil-engine -- \
  curl -s http://localhost:9090/healthz
```

---

## 5. Post-incident

- [ ] Root cause identified and documented in the incident ticket
- [ ] All affected account holders notified within SLA (4 h for financial data incidents)
- [ ] Compensating entries verified against double-entry invariant
- [ ] Post-mortem scheduled within 5 business days
- [ ] Any code fix deployed via normal blue-green process
- [ ] Monitoring alert added to detect the same class of corruption early
