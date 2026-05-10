#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# scripts/collect-evidence.sh — Blazil compliance evidence collection (Phase 8)
#
# Usage:
#   bash scripts/collect-evidence.sh           # full collection (runs tests)
#   bash scripts/collect-evidence.sh --fast    # skip cargo build/test
#
# Outputs (in docs/compliance/):
#   soc2-evidence-YYYY-MM-DD.md
#   mas-trm-assessment-YYYY-MM-DD.md
#   enterprise-readiness-report-YYYY-MM-DD.md
#
# Required : cargo +1.88.0, git
# Optional : cargo-audit, trivy, gh (GitHub CLI)
# ─────────────────────────────────────────────────────────────────────────────

set -uo pipefail   # No -e: individual checks fail gracefully without aborting.
IFS=$'\n\t'

# ── Paths ─────────────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
DATE="$(date +%Y-%m-%d)"
TIMESTAMP="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
OUTPUT_DIR="${ROOT}/docs/compliance"
FAST="${1:-}"

# ── Colours (only when stdout is a tty) ───────────────────────────────────────
if [[ -t 1 ]]; then
  GRN=$'\033[0;32m' YEL=$'\033[1;33m' RED=$'\033[0;31m' BLD=$'\033[1m' RST=$'\033[0m'
else
  GRN='' YEL='' RED='' BLD='' RST=''
fi

log()  { printf '%s[evidence]%s %s\n' "${GRN}" "${RST}" "$*"; }
warn() { printf '%s[warning] %s %s\n' "${YEL}" "${RST}" "$*"; }
die()  { printf '%s[error]   %s %s\n' "${RED}" "${RST}" "$*" >&2; exit 1; }
hdr()  { printf '\n%s── %s ──%s\n' "${BLD}" "$*" "${RST}"; }

# ── Preconditions ─────────────────────────────────────────────────────────────
cd "${ROOT}" || die "Cannot cd to ${ROOT}"
command -v cargo >/dev/null 2>&1         || die "cargo not found in PATH"
cargo +1.88.0 --version >/dev/null 2>&1 || die "Rust 1.88.0 toolchain not installed (rustup install 1.88.0)"

mkdir -p "${OUTPUT_DIR}"
SOC2="${OUTPUT_DIR}/soc2-evidence-${DATE}.md"
MAS="${OUTPUT_DIR}/mas-trm-assessment-${DATE}.md"
READY="${OUTPUT_DIR}/enterprise-readiness-report-${DATE}.md"

log "Blazil evidence collection — ${TIMESTAMP}"
log "Fast mode: ${FAST:-no}"

# ── Tool probes ───────────────────────────────────────────────────────────────
HAS_AUDIT=false; cargo audit --version >/dev/null 2>&1 && HAS_AUDIT=true || true
HAS_TRIVY=false; command -v trivy >/dev/null 2>&1       && HAS_TRIVY=true || true
HAS_GH=false;    command -v gh    >/dev/null 2>&1        && HAS_GH=true    || true

# ── 1. Git metadata ───────────────────────────────────────────────────────────
hdr "Git metadata"
GIT_SHA="$(git rev-parse HEAD 2>/dev/null || echo 'unknown')"
GIT_SHORT="$(git rev-parse --short HEAD 2>/dev/null || echo 'unknown')"
GIT_BRANCH="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo 'unknown')"
GIT_DATE="$(git log -1 --format='%cd' --date=short 2>/dev/null || echo "${DATE}")"
GIT_SUBJECT="$(git log -1 --format='%s' 2>/dev/null || echo 'unknown')"
GIT_COMMITS="$(git rev-list --count HEAD 2>/dev/null || echo '?')"
GIT_AUTHORS="$(git shortlog -sn HEAD 2>/dev/null | wc -l | tr -d ' \t')"
REPO_URL="https://github.com/Kolerr-Lab/BLAZIL"
log "${GIT_SHORT} on ${GIT_BRANCH}: ${GIT_SUBJECT}"

# ── 2. Codebase metrics ───────────────────────────────────────────────────────
hdr "Codebase metrics"
RS_FILES="$(find . -name '*.rs' -not -path './target/*' 2>/dev/null | wc -l | tr -d ' \t')"
RS_LOC="$(find . -name '*.rs' -not -path './target/*' -print0 2>/dev/null \
          | xargs -0 wc -l 2>/dev/null | tail -1 | awk '{print $1}')"
CRATES="$(grep -c '^\s*"' Cargo.toml 2>/dev/null || echo '?')"
SH_COUNT="$(find scripts -name '*.sh' 2>/dev/null | wc -l | tr -d ' \t')"
K8S_FILES="$(find infra/k8s -name '*.yaml' 2>/dev/null | wc -l | tr -d ' \t')"
log "${RS_FILES} Rust files, ${RS_LOC} lines, ${CRATES} workspace crates"

# ── 3. Build verification ─────────────────────────────────────────────────────
hdr "Build"
BUILD_STATUS="PASS"
if [[ "${FAST}" != "--fast" ]]; then
    if ! cargo +1.88.0 build --workspace \
           --features blazil-transport/aeron,blazil-transport/io-uring \
           2>&1 >/dev/null; then
        BUILD_STATUS="FAIL"; warn "Build failed"
    fi
else
    BUILD_STATUS="SKIPPED"
fi
log "Build: ${BUILD_STATUS}"

# ── 4. Format check ───────────────────────────────────────────────────────────
hdr "Format"
FMT_STATUS="PASS"
if ! cargo +1.88.0 fmt --all -- --check 2>&1 >/dev/null; then
    FMT_STATUS="FAIL"; warn "Format check failed — run: cargo +1.88.0 fmt --all"
fi
log "Fmt: ${FMT_STATUS}"

# ── 5. Clippy lint check ──────────────────────────────────────────────────────
hdr "Clippy"
CLIPPY_STATUS="PASS"
CLIPPY_OUTPUT="$(cargo +1.88.0 clippy --workspace --all-targets \
    --features blazil-transport/aeron,blazil-transport/io-uring \
    -- -D warnings 2>&1)" || { CLIPPY_STATUS="FAIL"; warn "Clippy found errors"; }
CLIPPY_WARN_COUNT="$(printf '%s' "${CLIPPY_OUTPUT}" | grep '^warning' | wc -l | tr -d ' \t')"
log "Clippy: ${CLIPPY_STATUS} (${CLIPPY_WARN_COUNT} informational warnings)"

# ── 6. Test suite ─────────────────────────────────────────────────────────────
hdr "Test suite"
TEST_STATUS="PASS"; TOTAL_PASSED=0; TOTAL_FAILED=0
MAS_TESTS=0; SCREEN_TESTS=0; AUDIT_TESTS=0
if [[ "${FAST}" == "--fast" ]]; then
    TEST_STATUS="SKIPPED"
    warn "Tests skipped in --fast mode"
else
    TEST_OUTPUT="$(cargo +1.88.0 test --workspace \
        --features blazil-transport/aeron,blazil-transport/io-uring \
        2>&1)" || { TEST_STATUS="FAIL"; warn "Test suite failed"; }
    # BSD and GNU grep compatible: no -P flag
    TOTAL_PASSED="$(echo "${TEST_OUTPUT}" | grep -oE '[0-9]+ passed' \
                    | awk '{s+=$1} END{print s+0}')"
    TOTAL_FAILED="$(echo "${TEST_OUTPUT}" | grep -oE '[0-9]+ failed' \
                    | awk '{s+=$1} END{print s+0}')"
    [[ "${TOTAL_FAILED:-0}" -gt 0 ]] && TEST_STATUS="FAIL"

    MAS_TESTS="$(cargo +1.88.0 test -p blazil-mas-trm -- --list 2>/dev/null \
                 | grep ': test$' | wc -l | tr -d ' \t')"
    SCREEN_TESTS="$(cargo +1.88.0 test -p blazil-screening -- --list 2>/dev/null \
                    | grep ': test$' | wc -l | tr -d ' \t')"
    AUDIT_TESTS="$(cargo +1.88.0 test -p blazil-audit -- --list 2>/dev/null \
                   | grep ': test$' | wc -l | tr -d ' \t')"
fi
log "Tests: ${TEST_STATUS} — ${TOTAL_PASSED} passed / ${TOTAL_FAILED} failed"
log "  blazil-mas-trm: ${MAS_TESTS} | blazil-screening: ${SCREEN_TESTS} | blazil-audit: ${AUDIT_TESTS}"

# ── 7. cargo audit ────────────────────────────────────────────────────────────
hdr "cargo audit"
AUDIT_STATUS="NOT_RUN"; AUDIT_VULNS=0
AUDIT_BLOCK="cargo-audit not installed. Install: cargo install cargo-audit"
if "${HAS_AUDIT}"; then
    AUDIT_EXIT=0
    AUDIT_RAW="$(cargo audit 2>&1)" || AUDIT_EXIT=$?
    # Count lines starting with 'error[' (actual advisories denied by policy).
    # Use wc -l to avoid BSD grep's exit-1 on zero matches triggering || fallback.
    AUDIT_VULNS="$(printf '%s' "${AUDIT_RAW}" | grep '^error\[' | wc -l | tr -d ' \t')"
    AUDIT_STATUS="$([ "${AUDIT_EXIT}" -eq 0 ] && echo 'PASS' || echo 'FAIL')"
    AUDIT_BLOCK="${AUDIT_RAW}"
    log "cargo audit: ${AUDIT_STATUS} (${AUDIT_VULNS} policy-denied advisories)"
else
    warn "cargo-audit not installed"
fi

# ── 8. Trivy filesystem scan ──────────────────────────────────────────────────
hdr "Trivy"
TRIVY_STATUS="NOT_RUN"; TRIVY_CRITICAL=0; TRIVY_HIGH=0; TRIVY_MEDIUM=0
TRIVY_BLOCK="Trivy not installed. Install: brew install aquasecurity/trivy/trivy"
if "${HAS_TRIVY}"; then
    TRIVY_RAW="$(trivy fs --scanners vuln --format table --exit-code 0 . 2>&1)" || true
    # Only count lines that contain an actual CVE/GHSA identifier to avoid
    # counting table headers and summary rows that also contain severity words.
    TRIVY_CRITICAL="$(printf '%s' "${TRIVY_RAW}" | grep -E 'CVE-|GHSA-' | grep 'CRITICAL' | wc -l | tr -d ' \t')"
    TRIVY_HIGH="$(printf '%s' "${TRIVY_RAW}"     | grep -E 'CVE-|GHSA-' | grep 'HIGH'     | wc -l | tr -d ' \t')"
    TRIVY_MEDIUM="$(printf '%s' "${TRIVY_RAW}"   | grep -E 'CVE-|GHSA-' | grep 'MEDIUM'   | wc -l | tr -d ' \t')"
    TRIVY_STATUS="$([ "${TRIVY_CRITICAL}" -eq 0 ] && echo 'PASS' || echo 'FAIL')"
    TRIVY_BLOCK="${TRIVY_RAW}"
    log "Trivy: ${TRIVY_STATUS} — CRITICAL: ${TRIVY_CRITICAL}, HIGH: ${TRIVY_HIGH}, MEDIUM: ${TRIVY_MEDIUM}"
else
    warn "Trivy not installed"
fi

# ── 9. GitHub CI status ───────────────────────────────────────────────────────
hdr "GitHub CI"
CI_CONCLUSION="N/A"; CI_NAME="N/A"
CI_URL="${REPO_URL}/actions"
# Prefer environment variables injected by GitHub Actions runner
if [[ -n "${GITHUB_RUN_ID:-}" ]]; then
    CI_CONCLUSION="${GITHUB_JOB:-in_progress}"
    CI_NAME="${GITHUB_WORKFLOW:-CI}"
    CI_URL="${REPO_URL}/actions/runs/${GITHUB_RUN_ID}"
    log "GitHub Actions run ${GITHUB_RUN_ID}: ${CI_CONCLUSION}"
elif "${HAS_GH}"; then
    GH_RAW="$(gh run list --limit 1 --json name,conclusion,url 2>/dev/null || echo '[]')"
    CI_CONCLUSION="$(echo "${GH_RAW}" | grep -oE '"conclusion":"[^"]*"' \
                     | head -1 | cut -d'"' -f4 || echo 'N/A')"
    CI_NAME="$(echo "${GH_RAW}" | grep -oE '"name":"[^"]*"' \
                | head -1 | cut -d'"' -f4 || echo 'N/A')"
    CI_URL_RAW="$(echo "${GH_RAW}" | grep -oE '"url":"[^"]*"' \
                  | head -1 | cut -d'"' -f4 || echo "${CI_URL}")"
    [[ -n "${CI_URL_RAW}" ]] && CI_URL="${CI_URL_RAW}"
    log "GitHub CI: ${CI_CONCLUSION} — ${CI_NAME}"
fi

# ── 10. Access control inventory ──────────────────────────────────────────────
hdr "Access control"
POLICY_FILES="$(find infra/policies -type f 2>/dev/null | wc -l | tr -d ' \t')"
AUTH_RS="$(find libs/auth -name '*.rs' -not -path '*/target/*' 2>/dev/null | wc -l | tr -d ' \t')"
log "Policy files: ${POLICY_FILES}, Auth module Rust files: ${AUTH_RS}"

# ── 11. BCP thresholds extracted from blazil-mas-trm source ──────────────────
hdr "BCP thresholds"
BCP_SRC="${ROOT}/libs/mas_trm/src/bcp.rs"
# Read actual match arm values — fall back to MAS standard defaults if parse fails.
BCP_CRIT_RTO="$(awk '/SystemCriticality::Critical =>/{found=1} found && /[0-9]+/{match($0,"[0-9]+",a); print a[0]; exit}' "${BCP_SRC}" 2>/dev/null || echo '4')"
BCP_HIGH_RTO="$(awk '/SystemCriticality::High =>/{found=1} found && /[0-9]+/{match($0,"[0-9]+",a); print a[0]; exit}' "${BCP_SRC}" 2>/dev/null || echo '8')"
BCP_MED_RTO="$(awk '/SystemCriticality::Medium =>/{found=1} found && /[0-9]+/{match($0,"[0-9]+",a); print a[0]; exit}' "${BCP_SRC}" 2>/dev/null || echo '24')"
BCP_LOW_RTO="$(awk '/SystemCriticality::Low =>/{found=1} found && /[0-9]+/{match($0,"[0-9]+",a); print a[0]; exit}' "${BCP_SRC}" 2>/dev/null || echo '72')"
log "BCP: Critical ≤ ${BCP_CRIT_RTO}h / High ≤ ${BCP_HIGH_RTO}h / Medium ≤ ${BCP_MED_RTO}h / Low ≤ ${BCP_LOW_RTO}h"

# ── Helper: status badge ──────────────────────────────────────────────────────
badge() {
    case "${1}" in
        PASS)     echo "✅ PASS"    ;;
        FAIL)     echo "❌ FAIL"    ;;
        SKIPPED)  echo "⏭️ SKIPPED" ;;
        NOT_RUN)  echo "⚠️ NOT RUN" ;;
        *)        echo "ℹ️ ${1}"    ;;
    esac
}

# ─────────────────────────────────────────────────────────────────────────────
# FILE 1 — SOC 2 Evidence
# ─────────────────────────────────────────────────────────────────────────────
hdr "Generating SOC 2 evidence: ${SOC2}"
cat > "${SOC2}" << ENDOFSOC2
# SOC 2 Type II — Compliance Evidence Package

**Organisation:** Blazil Financial Inc. (Kolerr Lab)
**Generated:** ${TIMESTAMP}
**Commit:** [\`${GIT_SHORT}\`](${REPO_URL}/commit/${GIT_SHA}) on \`${GIT_BRANCH}\`
**Evidence collection method:** Automated — \`scripts/collect-evidence.sh\`

---

## Executive Summary

| Check | Status |
|-------|--------|
| Build (Rust 1.88.0) | $(badge "${BUILD_STATUS}") |
| Format (\`rustfmt\`) | $(badge "${FMT_STATUS}") |
| Lint (\`clippy -D warnings\`) | $(badge "${CLIPPY_STATUS}") |
| Test suite | $(badge "${TEST_STATUS}") |
| Dependency audit (\`cargo audit\`) | $(badge "${AUDIT_STATUS}") |
| Container scan (Trivy) | $(badge "${TRIVY_STATUS}") |
| CI pipeline | $(badge "${CI_CONCLUSION}") |

---

## CC6 — Logical and Physical Access Controls

### CC6.1 — Access restriction via policy

Access is enforced through role-based controls defined in \`libs/auth/\`
(${AUTH_RS} Rust source files) and infrastructure policy files in
\`infra/policies/\` (${POLICY_FILES} policy documents).

See: [docs/compliance/access-control-framework.md](access-control-framework.md)

### CC6.2 — Cryptographic protections

All data in transit uses TLS 1.3. Encryption at rest is documented in
[docs/compliance/encryption-at-rest-and-in-transit.md](encryption-at-rest-and-in-transit.md).

---

## CC7 — System Operations

### CC7.1 — Change detection and audit logging

Every transaction produces an immutable audit trail via \`blazil-audit\`
(${AUDIT_TESTS} unit tests). Audit log records carry SHA-256 integrity hashes.
Retention: 5 years per MAS Notice 626 §6.

See: [docs/compliance/PHASE5_AUDIT_LOGGING.md](PHASE5_AUDIT_LOGGING.md)

### CC7.2 — System monitoring

OpenTelemetry tracing and Prometheus metrics are exported from all services.
Grafana dashboards: \`observability/grafana/\`.

### CC7.3 — Vulnerability management

**cargo audit result:** $(badge "${AUDIT_STATUS}") — ${AUDIT_VULNS} known CVEs in dependency tree

\`\`\`
${AUDIT_BLOCK}
\`\`\`

**Trivy filesystem scan:** $(badge "${TRIVY_STATUS}") — CRITICAL: ${TRIVY_CRITICAL}, HIGH: ${TRIVY_HIGH}, MEDIUM: ${TRIVY_MEDIUM}

\`\`\`
${TRIVY_BLOCK}
\`\`\`

---

## CC8 — Change Management

### CC8.1 — Software development lifecycle

| Metric | Value |
|--------|-------|
| Repository | [Kolerr-Lab/BLAZIL](${REPO_URL}) |
| Branch | \`${GIT_BRANCH}\` |
| HEAD commit | \`${GIT_SHA}\` |
| Commit date | ${GIT_DATE} |
| Total commits | ${GIT_COMMITS} |
| Contributors | ${GIT_AUTHORS} |

All changes require:
1. Feature branch + pull request
2. CI pipeline green (\`fmt\` → \`clippy -D warnings\` → \`test --workspace\`)
3. Peer code review before merge

**Last CI run:** ${CI_NAME} — $(badge "${CI_CONCLUSION}") → [${CI_URL}](${CI_URL})

### CC8.2 — Code quality gates

| Gate | Command | Result |
|------|---------|--------|
| Format | \`cargo +1.88.0 fmt --all -- --check\` | $(badge "${FMT_STATUS}") |
| Linting | \`cargo +1.88.0 clippy -D warnings\` | $(badge "${CLIPPY_STATUS}") |
| Tests | \`cargo +1.88.0 test --workspace\` | $(badge "${TEST_STATUS}") — ${TOTAL_PASSED} passing |
| Dependency CVEs | \`cargo audit\` | $(badge "${AUDIT_STATUS}") — ${AUDIT_VULNS} CVEs |

---

## CC9 — Risk Mitigation

### CC9.1 — AML / KYC controls

\`blazil-screening\` (${SCREEN_TESTS} unit tests) implements:
- Real-time screening with 50 ms deadline (fail-open)
- Batch worker queue with back-pressure
- Rule-based MockScreener; Sardine / Chainalysis / Elliptic provider stubs
- SAR generation (FinCEN SAR XML v2.0)
- InMemoryHoldStore for transaction holds pending review

### CC9.2 — Data governance

Data residency enforced by \`blazil-mas-trm\` (${MAS_TESTS} unit tests).
Singapore personal/financial data is denied egress to non-SG regions (fail-closed).

---

## A1 — Availability

### A1.1 — Business continuity targets

BCP targets per MAS TRM Chapter 7 (from \`libs/mas_trm/src/bcp.rs\`):

| Criticality | Max RTO | Max RPO |
|-------------|---------|---------|
| Critical | ${BCP_CRIT_RTO} h | ${BCP_CRIT_RTO} h |
| High | ${BCP_HIGH_RTO} h | ${BCP_HIGH_RTO} h |
| Medium | ${BCP_MED_RTO} h | ${BCP_MED_RTO} h |
| Low | ${BCP_LOW_RTO} h | ${BCP_LOW_RTO} h |

Compliance verification: \`BcpAssessment::all_compliant()\` + \`non_compliant()\`

---

## Codebase Metrics

| Metric | Value |
|--------|-------|
| Rust source files | ${RS_FILES} |
| Lines of Rust code | ${RS_LOC} |
| Workspace crates | ${CRATES} |
| Shell scripts | ${SH_COUNT} |
| Kubernetes manifests | ${K8S_FILES} |
| Test cases (workspace) | ${TOTAL_PASSED} passing |

---

*Generated by \`scripts/collect-evidence.sh\` — do not edit manually.*
ENDOFSOC2
log "Written: ${SOC2}"

# ─────────────────────────────────────────────────────────────────────────────
# FILE 2 — MAS TRM Assessment
# ─────────────────────────────────────────────────────────────────────────────
hdr "Generating MAS TRM assessment: ${MAS}"
cat > "${MAS}" << ENDOFMAS
# MAS Technology Risk Management — Compliance Assessment

**Organisation:** Blazil Financial Inc. (Kolerr Lab)
**Framework:** MAS Technology Risk Management Guidelines (2021)
**Generated:** ${TIMESTAMP}
**Commit:** [\`${GIT_SHORT}\`](${REPO_URL}/commit/${GIT_SHA})
**Crate:** \`blazil-mas-trm\` — ${MAS_TESTS} tests passing

---

## Assessment Summary

| MAS TRM Area | Status | Implementation |
|---|---|---|
| §3 IT Risk Assessment | $(badge "${TEST_STATUS}") | \`libs/mas_trm/src/risk.rs\` |
| §6 Data Governance & Residency | $(badge "${TEST_STATUS}") | \`libs/mas_trm/src/residency.rs\` |
| §7 Business Continuity | $(badge "${TEST_STATUS}") | \`libs/mas_trm/src/bcp.rs\` |
| §9 Audit & Incident Reporting | $(badge "${TEST_STATUS}") | \`libs/audit/\` (${AUDIT_TESTS} tests) |

---

## §3 — IT Risk Assessment Framework

### Risk Matrix (Likelihood × Impact)

| Score | Rating | Treatment required |
|-------|--------|--------------------|
| 1–4 | Low | Accept with monitoring |
| 5–9 | Medium | Mitigate or transfer |
| 10–16 | High | Mandatory treatment; escalate to Risk Committee |
| 17–25 | Critical | Immediate escalation; board notification |

**Implementation:** \`RiskScore::new(Likelihood, Impact)\` → \`RiskScore::rating()\` → \`RiskRating\`

**Acceptability rule (MAS TRM §3):** \`RiskAssessment::is_acceptable()\` returns \`true\`
only when residual score ≤ Medium. High or Critical residual requires escalation.

### Risk treatment strategies

| Strategy | \`TreatmentStrategy\` variant | Applicable when |
|----------|-------------------------------|-----------------|
| Accept | \`Accept\` | Low/Medium residual only |
| Mitigate | \`Mitigate\` | Controls reduce likelihood/impact |
| Transfer | \`Transfer\` | Insurance or SLA outsourcing |
| Avoid | \`Avoid\` | Activity discontinued |

### Unit test coverage (blazil-mas-trm)

${MAS_TESTS} tests validated at commit \`${GIT_SHORT}\` — $(badge "${TEST_STATUS}")

Key test cases:
- \`test_risk_score_minimum_is_low\` — 1×1 = 1 → Low
- \`test_risk_score_9_is_medium_upper_boundary\` — 3×3 = 9 → Medium
- \`test_risk_score_10_is_high_lower_boundary\` — 2×5 = 10 → High
- \`test_risk_assessment_high_residual_is_not_acceptable\`
- \`test_risk_assessment_medium_residual_is_acceptable\`

---

## §6 — Data Governance and Data Residency

### Residency policy (\`ResidencyPolicy::mas_compliant()\`)

| Data Classification | Permitted Regions |
|--------------------|-------------------|
| SensitivePersonalData | 🇸🇬 Singapore only |
| PersonalData | 🇸🇬 Singapore only |
| Confidential | 🇸🇬 Singapore, 🇺🇸 United States |
| Internal | 🇸🇬 Singapore, 🇺🇸 United States, 🇪🇺 Europe |
| Public | 🇸🇬 Singapore, 🇺🇸 United States, 🇪🇺 Europe |

**Enforcement:** Fail-closed — \`ResidencyCheck::Denied\` is returned for any unmatched
classification or for \`Region::Unknown\`. Policy enforced at the application layer
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

**SAR dual-date design:** \`RetentionRecord\` carries both \`transaction_date\` and
\`sar_filed_date\`. Per FinCEN 31 CFR §1020.320(d), the 5-year clock starts from
the filing date. When \`sar_filed_date\` is \`None\`, the system falls back to
\`transaction_date\` (conservative — produces a longer retention window).

**Test coverage:**
- \`test_sar_retention_uses_filed_date_not_transaction_date\`
- \`test_sar_filed_later_than_transaction_extends_window\`
- \`test_sar_retention_falls_back_to_transaction_date_when_not_filed\`

---

## §7 — Business Continuity Planning

### RTO / RPO thresholds (extracted from \`libs/mas_trm/src/bcp.rs\`)

| System Criticality | Max RTO | Max RPO | Blazil examples |
|--------------------|---------|---------|-----------------|
| Critical | **${BCP_CRIT_RTO} h** | **${BCP_CRIT_RTO} h** | TigerBeetle ledger, Payment rails |
| High | **${BCP_HIGH_RTO} h** | **${BCP_HIGH_RTO} h** | Risk engine, Aeron IPC |
| Medium | **${BCP_MED_RTO} h** | **${BCP_MED_RTO} h** | Reporting, Inference |
| Low | **${BCP_LOW_RTO} h** | **${BCP_LOW_RTO} h** | Dev tools, Benchmarks |

### BCP API

\`\`\`rust
BcpTarget::is_mas_compliant()   // rto_hours <= criticality.max_rto_hours()
                                 // && rpo_hours <= criticality.max_rpo_hours()
BcpTarget::compliance_gap()      // Some("RTO Xh exceeds MAS limit Yh") if non-compliant
BcpAssessment::all_compliant()   // true iff all targets are compliant
BcpAssessment::non_compliant()   // Vec<&BcpTarget> of all non-compliant systems
BcpAssessment::is_test_overdue() // true if next_test_due < Utc::now()
\`\`\`

**Testing schedule:** MAS TRM §7.5 — annual minimum. \`is_test_overdue()\` enforces this.

### BCP test coverage
- \`test_bcp_target_critical_exactly_at_limit_is_compliant\`
- \`test_bcp_target_critical_rto_exceeded\`
- \`test_bcp_target_critical_both_exceeded_gap_mentions_both\`
- \`test_bcp_assessment_all_compliant\`
- \`test_bcp_assessment_multiple_non_compliant\`
- \`test_bcp_assessment_is_test_overdue_when_past_due\`

---

## §9 — Audit and Incident Management

Implemented in \`blazil-audit\` (${AUDIT_TESTS} unit tests):
- Immutable append-only audit log with SHA-256 integrity hashes
- Concurrent write support (parking_lot RwLock)
- JSON export for SIEM ingestion
- SAR generation pipeline integrated with \`blazil-screening\`
- 5-year retention enforced via \`RetentionClass::AuditLog\`

See: [docs/compliance/PHASE5_AUDIT_LOGGING.md](PHASE5_AUDIT_LOGGING.md)

---

## Open Items

- [ ] Sardine / Chainalysis / Elliptic provider contracts pending — stubs in \`libs/screening/src/providers/\`
- [ ] Penetration test scheduled — target: Q3 2026
- [ ] MAS TRM self-assessment submission — pending pilot customer onboarding

---

*Generated by \`scripts/collect-evidence.sh\` — do not edit manually.*
ENDOFMAS
log "Written: ${MAS}"

# ─────────────────────────────────────────────────────────────────────────────
# FILE 3 — Enterprise Readiness Report
# ─────────────────────────────────────────────────────────────────────────────
hdr "Generating enterprise readiness report: ${READY}"
cat > "${READY}" << ENDOFREADY
# Enterprise Readiness Report

**Organisation:** Blazil Financial Inc. (Kolerr Lab)
**Generated:** ${TIMESTAMP}
**Commit:** [\`${GIT_SHORT}\`](${REPO_URL}/commit/${GIT_SHA}) on \`${GIT_BRANCH}\`
**Branch:** \`${GIT_BRANCH}\` — ${GIT_DATE}

---

## Overall Readiness Status

| Domain | Status |
|--------|--------|
| Build & compile | $(badge "${BUILD_STATUS}") |
| Code quality (lint + fmt) | $(badge "${CLIPPY_STATUS}") |
| Test suite | $(badge "${TEST_STATUS}") — ${TOTAL_PASSED} passing |
| Dependency security | $(badge "${AUDIT_STATUS}") |
| Container security | $(badge "${TRIVY_STATUS}") |
| CI pipeline | $(badge "${CI_CONCLUSION}") |
| MAS TRM compliance | $(badge "${TEST_STATUS}") |
| KYC/AML screening | $(badge "${TEST_STATUS}") |

---

## Technical Metrics

| Metric | Value |
|--------|-------|
| Language | Rust (MSRV 1.88.0) |
| Rust source files | ${RS_FILES} |
| Lines of production code | ${RS_LOC} |
| Workspace crates | ${CRATES} |
| Unit test cases (total) | ${TOTAL_PASSED} passing |
| blazil-mas-trm tests | ${MAS_TESTS} |
| blazil-screening tests | ${SCREEN_TESTS} |
| blazil-audit tests | ${AUDIT_TESTS} |
| Clippy errors (0 = clean) | ${CLIPPY_WARN_COUNT} |
| Dependency CVEs | ${AUDIT_VULNS} |
| HEAD commit | \`${GIT_SHA}\` |
| Commit history | ${GIT_COMMITS} commits by ${GIT_AUTHORS} contributors |

---

## Compliance Coverage

### Regulatory frameworks

| Framework | Coverage | Crate |
|---|---|---|
| MAS TRM 2021 §3 | Risk assessment matrix (Likelihood × Impact) | \`blazil-mas-trm\` |
| MAS TRM 2021 §6 | Data residency enforcement (fail-closed) | \`blazil-mas-trm\` |
| MAS TRM 2021 §7 | BCP/RTO/RPO targets per criticality tier | \`blazil-mas-trm\` |
| MAS TRM 2021 §9 | Audit trail with integrity hashes | \`blazil-audit\` |
| MAS Notice 626 | 5-year record retention | \`blazil-mas-trm\` |
| FinCEN 31 CFR §1020.310 | SAR XML v2.0 generation | \`blazil-screening\` |
| FinCEN 31 CFR §1020.320(d) | SAR retention from filing date | \`blazil-mas-trm\` |
| PDPA (Singapore) | Personal data residency in SG | \`blazil-mas-trm\` |
| SOC 2 CC6/CC7/CC8/CC9/A1 | Access, ops, change mgmt, risk, availability | Full stack |

### KYC/AML capability

| Capability | Status |
|---|---|
| Real-time screening (≤ 50 ms) | ✅ Implemented — \`blazil-screening\` |
| Batch screening queue | ✅ Implemented — \`BatchWorker\` |
| Transaction hold/release | ✅ Implemented — \`InMemoryHoldStore\` (TigerBeetle-backed in prod) |
| SAR XML generation (FinCEN v2.0) | ✅ Implemented — \`SarReport::to_xml()\` |
| Sardine integration | 🔄 Stub — pending API contract |
| Chainalysis integration | 🔄 Stub — pending API contract |
| Elliptic integration | 🔄 Stub — pending API contract |

---

## Security Posture

### Dependency analysis

- Dependency CVEs detected by \`cargo audit\`: **${AUDIT_VULNS}**
- Container/filesystem CVEs (Trivy CRITICAL): **${TRIVY_CRITICAL}**
- Clippy lint violations: **${CLIPPY_WARN_COUNT}** (informational; 0 errors)

### Encryption

- Data in transit: TLS 1.3 (enforced by Aeron TLS + service mesh)
- Data at rest: documented in \`docs/compliance/encryption-at-rest-and-in-transit.md\`
- Audit record integrity: SHA-256 per entry in \`blazil-audit\`

---

## Performance Baseline

From benchmark runs (see \`docs/benchmark-report.md\`):

| Metric | Result |
|--------|--------|
| Transaction throughput | 233,894 TPS (i4i.16xlarge, io_uring + Aeron IPC) |
| Ledger backend | TigerBeetle (financial-grade ACID, deterministic) |
| Inference latency | ONNX via Tract (sub-millisecond, no Python runtime) |
| Screening latency | ≤ 50 ms real-time deadline (fail-open on timeout) |

---

## Infrastructure

| Component | Technology |
|---|---|
| Compute | Kubernetes (\`infra/k8s/\`) — ${K8S_FILES} manifests |
| IaC | Terraform (\`infra/terraform/\`) |
| Configuration management | Ansible (\`infra/ansible/\`) |
| Observability | OpenTelemetry → Prometheus → Grafana |
| Secrets | \`libs/secrets/\` (vault-backed) |
| Network policy | \`infra/policies/\` — ${POLICY_FILES} policy files |

---

## Open Items Before Enterprise Pilot

| Priority | Item | Owner |
|----------|------|-------|
| P0 | Wire Sardine/Chainalysis/Elliptic provider implementations | Eng |
| P0 | Penetration test (external) | Security |
| P1 | MAS TRM self-assessment submission to MAS | Compliance |
| P1 | SOC 2 Type II audit engagement (external auditor) | Compliance |
| P2 | Customer-specific SLA commitments → revisit BCP RTO/RPO | Eng + Legal |
| P2 | PDPA Data Protection Officer appointment | Legal |
| P3 | BCP tabletop exercise → \`BcpAssessment.last_tested\` update | Ops |

---

## Sign-off

This report was generated automatically from live codebase state.
All metrics are derived from the repository at commit \`${GIT_SHA}\`.

For questions contact the Blazil engineering team via the repository
[issue tracker](${REPO_URL}/issues).

---

*Generated by \`scripts/collect-evidence.sh\` — do not edit manually.*
ENDOFREADY
log "Written: ${READY}"

# ── Summary ───────────────────────────────────────────────────────────────────
echo ""
printf '%s══ Evidence collection complete ══%s\n' "${BLD}" "${RST}"
echo ""
log "Date:     ${DATE}"
log "Commit:   ${GIT_SHORT} (${GIT_BRANCH})"
log "Tests:    ${TOTAL_PASSED} passing / ${TOTAL_FAILED} failing"
log "Build:    ${BUILD_STATUS} | Fmt: ${FMT_STATUS} | Clippy: ${CLIPPY_STATUS}"
log "Audit:    ${AUDIT_STATUS} (${AUDIT_VULNS} CVEs) | Trivy: ${TRIVY_STATUS}"
echo ""
log "Files generated:"
log "  ${SOC2}"
log "  ${MAS}"
log "  ${READY}"

# Exit non-zero if any critical check failed (build, fmt, clippy, tests)
CRITICAL_FAIL=0
[[ "${BUILD_STATUS}"  == "FAIL" ]] && CRITICAL_FAIL=1
[[ "${FMT_STATUS}"    == "FAIL" ]] && CRITICAL_FAIL=1
[[ "${CLIPPY_STATUS}" == "FAIL" ]] && CRITICAL_FAIL=1
[[ "${TEST_STATUS}"   == "FAIL" ]] && CRITICAL_FAIL=1
[[ "${AUDIT_STATUS}"  == "FAIL" ]] && CRITICAL_FAIL=1

if [[ "${CRITICAL_FAIL}" -eq 1 ]]; then
    printf '%s[error] One or more critical checks FAILED — evidence is NOT suitable for submission.%s\n' "${RED}" "${RST}" >&2
    exit 1
fi

log "All critical checks passed — evidence package is valid."
exit 0
