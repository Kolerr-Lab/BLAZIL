# MAS Technology Risk Management — Compliance Assessment

**Organisation:** Blazil Financial Inc. (Kolerr Lab)
**Framework:** MAS Technology Risk Management Guidelines (2021)
**Generated:** 2026-05-10T04:22:16Z
**Commit:** [`a2cc672`](https://github.com/Kolerr-Lab/BLAZIL/commit/a2cc672fad499d8d208e36e307485d63ab094554)
**Crate:** `blazil-mas-trm` — 54 tests passing

---

## Assessment Summary

| MAS TRM Area | Status | Implementation |
|---|---|---|
| §3 IT Risk Assessment | ✅ PASS | `libs/mas_trm/src/risk.rs` |
| §6 Data Governance & Residency | ✅ PASS | `libs/mas_trm/src/residency.rs` |
| §7 Business Continuity | ✅ PASS | `libs/mas_trm/src/bcp.rs` |
| §9 Audit & Incident Reporting | ✅ PASS | `libs/audit/` (32 tests) |

---

## §3 — IT Risk Assessment Framework

### Risk Matrix (Likelihood × Impact)

| Score | Rating | Treatment required |
|-------|--------|--------------------|
| 1–4 | Low | Accept with monitoring |
| 5–9 | Medium | Mitigate or transfer |
| 10–16 | High | Mandatory treatment; escalate to Risk Committee |
| 17–25 | Critical | Immediate escalation; board notification |

**Implementation:** `RiskScore::new(Likelihood, Impact)` → `RiskScore::rating()` → `RiskRating`

**Acceptability rule (MAS TRM §3):** `RiskAssessment::is_acceptable()` returns `true`
only when residual score ≤ Medium. High or Critical residual requires escalation.

### Risk treatment strategies

| Strategy | `TreatmentStrategy` variant | Applicable when |
|----------|-------------------------------|-----------------|
| Accept | `Accept` | Low/Medium residual only |
| Mitigate | `Mitigate` | Controls reduce likelihood/impact |
| Transfer | `Transfer` | Insurance or SLA outsourcing |
| Avoid | `Avoid` | Activity discontinued |

### Unit test coverage (blazil-mas-trm)

54 tests validated at commit `a2cc672` — ✅ PASS

Key test cases:
- `test_risk_score_minimum_is_low` — 1×1 = 1 → Low
- `test_risk_score_9_is_medium_upper_boundary` — 3×3 = 9 → Medium
- `test_risk_score_10_is_high_lower_boundary` — 2×5 = 10 → High
- `test_risk_assessment_high_residual_is_not_acceptable`
- `test_risk_assessment_medium_residual_is_acceptable`

---

## §6 — Data Governance and Data Residency

### Residency policy (`ResidencyPolicy::mas_compliant()`)

| Data Classification | Permitted Regions |
|--------------------|-------------------|
| SensitivePersonalData | 🇸🇬 Singapore only |
| PersonalData | 🇸🇬 Singapore only |
| Confidential | 🇸🇬 Singapore, 🇺🇸 United States |
| Internal | 🇸🇬 Singapore, 🇺🇸 United States, 🇪🇺 Europe |
| Public | 🇸🇬 Singapore, 🇺🇸 United States, 🇪🇺 Europe |

**Enforcement:** Fail-closed — `ResidencyCheck::Denied` is returned for any unmatched
classification or for `Region::Unknown`. Policy enforced at the application layer
before any data egress path.

### Data retention (MAS Notice 626 + FinCEN 31 CFR §1020.320(d))

| Record class | Retention | Anchor date |
|---|---|---|
| TransactionRecord | 5 years | Transaction date |
| KycRecord | 5 years | End of customer relationship |
| AuditLog | 5 years | Log creation date |
| SarReport | 5 years | **SAR filing date** (not transaction date) |
| SystemLog | 1 year | Log creation date |
| ConsentRecord | 2 years | Consent granted date |

**SAR dual-date design:** `RetentionRecord` carries both `transaction_date` and
`sar_filed_date`. Per FinCEN 31 CFR §1020.320(d), the 5-year clock starts from
the filing date. When `sar_filed_date` is `None`, the system falls back to
`transaction_date` (conservative — produces a longer retention window).

**Test coverage:**
- `test_sar_retention_uses_filed_date_not_transaction_date`
- `test_sar_filed_later_than_transaction_extends_window`
- `test_sar_retention_falls_back_to_transaction_date_when_not_filed`

---

## §7 — Business Continuity Planning

### RTO / RPO thresholds (extracted from `libs/mas_trm/src/bcp.rs`)

| System Criticality | Max RTO | Max RPO | Blazil examples |
|--------------------|---------|---------|-----------------|
| Critical | **4 h** | **4 h** | TigerBeetle ledger, Payment rails |
| High | **8 h** | **8 h** | Risk engine, Aeron IPC |
| Medium | **24 h** | **24 h** | Reporting, Inference |
| Low | **72 h** | **72 h** | Dev tools, Benchmarks |

### BCP API

```rust
BcpTarget::is_mas_compliant()   // rto_hours <= criticality.max_rto_hours()
                                 // && rpo_hours <= criticality.max_rpo_hours()
BcpTarget::compliance_gap()      // Some("RTO Xh exceeds MAS limit Yh") if non-compliant
BcpAssessment::all_compliant()   // true iff all targets are compliant
BcpAssessment::non_compliant()   // Vec<&BcpTarget> of all non-compliant systems
BcpAssessment::is_test_overdue() // true if next_test_due < Utc::now()
```

**Testing schedule:** MAS TRM §7.5 — annual minimum. `is_test_overdue()` enforces this.

### BCP test coverage
- `test_bcp_target_critical_exactly_at_limit_is_compliant`
- `test_bcp_target_critical_rto_exceeded`
- `test_bcp_target_critical_both_exceeded_gap_mentions_both`
- `test_bcp_assessment_all_compliant`
- `test_bcp_assessment_multiple_non_compliant`
- `test_bcp_assessment_is_test_overdue_when_past_due`

---

## §9 — Audit and Incident Management

Implemented in `blazil-audit` (32 unit tests):
- Immutable append-only audit log with SHA-256 integrity hashes
- Concurrent write support (parking_lot RwLock)
- JSON export for SIEM ingestion
- SAR generation pipeline integrated with `blazil-screening`
- 5-year retention enforced via `RetentionClass::AuditLog`

See: [docs/compliance/PHASE5_AUDIT_LOGGING.md](PHASE5_AUDIT_LOGGING.md)

---

## Open Items

- [ ] Sardine / Chainalysis / Elliptic provider contracts pending — stubs in `libs/screening/src/providers/`
- [ ] Penetration test scheduled — target: Q3 2026
- [ ] MAS TRM self-assessment submission — pending pilot customer onboarding

---

*Generated by `scripts/collect-evidence.sh` — do not edit manually.*
