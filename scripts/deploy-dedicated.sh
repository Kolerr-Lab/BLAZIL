#!/usr/bin/env bash
# deploy-dedicated.sh — One-command enterprise dedicated-tenant cluster deploy
#
# Generates a per-tenant Ansible inventory and runs the full dedicated.yml
# playbook: base setup → tenant isolation → RDMA detect → TLS → smoke test.
#
# Usage:
#   ./scripts/deploy-dedicated.sh <tenant_id> <node1_ip> <node2_ip> <node3_ip> [OPTIONS]
#
# Arguments:
#   tenant_id    Unique tenant identifier (e.g. "acme-corp", "fintechbank")
#   node1_ip     Public IP of node-1 (will host Prometheus + Grafana)
#   node2_ip     Public IP of node-2
#   node3_ip     Public IP of node-3
#
# Options:
#   --rdma                 Force RDMA transport (skip auto-detect)
#   --ssh-key <path>       SSH private key (default: ~/.ssh/id_ed25519)
#   --tb-private <ip1>,<ip2>,<ip3>
#                          Private IPs for TigerBeetle VSR network.
#                          Defaults to same as public IPs if omitted.
#   --api-key <key>        Tenant API key (32+ hex chars).
#                          Auto-generated via openssl if omitted.
#   --admin-token <token>  Gateway admin token.
#                          Auto-generated via openssl if omitted.
#   --dry-run              Print the ansible-playbook command; do not run it.
#   --tags <tags>          Pass custom Ansible tags (e.g. "smoke,tls").
#   -h, --help             Show this help.
#
# Examples:
#   # Full deploy with RDMA auto-detect:
#   ./scripts/deploy-dedicated.sh acme-corp 1.2.3.4 1.2.3.5 1.2.3.6
#
#   # Force RDMA transport:
#   ./scripts/deploy-dedicated.sh acme-corp 1.2.3.4 1.2.3.5 1.2.3.6 --rdma
#
#   # Use separate private IPs for TigerBeetle VSR:
#   ./scripts/deploy-dedicated.sh acme-corp 1.2.3.4 1.2.3.5 1.2.3.6 \
#     --tb-private 10.0.0.1,10.0.0.2,10.0.0.3
#
#   # Re-run only smoke test:
#   ./scripts/deploy-dedicated.sh acme-corp 1.2.3.4 1.2.3.5 1.2.3.6 --tags smoke

set -euo pipefail

# ── Resolve repository root ───────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
ANSIBLE_DIR="${REPO_ROOT}/infra/ansible"
INVENTORY_DIR="${ANSIBLE_DIR}/inventory"
PLAYBOOK="${ANSIBLE_DIR}/playbooks/dedicated.yml"
TEMPLATE="${INVENTORY_DIR}/dedicated-template"

# ── Colour output ─────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
BLUE='\033[0;34m'; BOLD='\033[1m'; RESET='\033[0m'

info()  { echo -e "${BLUE}[deploy]${RESET} $*"; }
ok()    { echo -e "${GREEN}[  ok  ]${RESET} $*"; }
warn()  { echo -e "${YELLOW}[ warn ]${RESET} $*"; }
die()   { echo -e "${RED}[error ]${RESET} $*" >&2; exit 1; }

# ── Argument parsing ──────────────────────────────────────────────────────────
if [[ $# -lt 4 ]] || [[ "$1" == "-h" ]] || [[ "$1" == "--help" ]]; then
  grep '^#' "${BASH_SOURCE[0]}" | sed 's/^# \?//' | head -40
  exit 0
fi

TENANT_ID="$1"
NODE1_IP="$2"
NODE2_IP="$3"
NODE3_IP="$4"
shift 4

SSH_KEY="${HOME}/.ssh/id_ed25519"
TB_PRIVATE_IPS=""
TENANT_API_KEY=""
ADMIN_TOKEN=""
RDMA_ENABLED="false"
DRY_RUN=false
EXTRA_TAGS=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --rdma)
      RDMA_ENABLED="true"; shift ;;
    --ssh-key)
      SSH_KEY="$2"; shift 2 ;;
    --tb-private)
      TB_PRIVATE_IPS="$2"; shift 2 ;;
    --api-key)
      TENANT_API_KEY="$2"; shift 2 ;;
    --admin-token)
      ADMIN_TOKEN="$2"; shift 2 ;;
    --dry-run)
      DRY_RUN=true; shift ;;
    --tags)
      EXTRA_TAGS="$2"; shift 2 ;;
    *)
      die "Unknown option: $1. Run with --help for usage." ;;
  esac
done

# ── Validation ────────────────────────────────────────────────────────────────
[[ -n "$TENANT_ID" ]]  || die "tenant_id must not be empty"
[[ -n "$NODE1_IP"  ]]  || die "node1_ip must not be empty"
[[ -n "$NODE2_IP"  ]]  || die "node2_ip must not be empty"
[[ -n "$NODE3_IP"  ]]  || die "node3_ip must not be empty"

# Validate tenant_id: alphanumeric + hyphens only (used in filenames + TLS CN)
[[ "$TENANT_ID" =~ ^[a-zA-Z0-9][a-zA-Z0-9-]{1,62}[a-zA-Z0-9]$ ]] \
  || die "tenant_id must be 3-64 chars, alphanumeric + hyphens (got: '$TENANT_ID')"

# Validate IPs are plausibly IPv4
for ip in "$NODE1_IP" "$NODE2_IP" "$NODE3_IP"; do
  [[ "$ip" =~ ^[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}$ ]] \
    || die "Not a valid IPv4 address: $ip"
done

[[ -f "$SSH_KEY" ]] || die "SSH key not found: $SSH_KEY"

command -v ansible-playbook >/dev/null 2>&1 \
  || die "ansible-playbook not found — install with: pip install ansible"

# ── Default private IPs to public IPs (single-NIC nodes) ─────────────────────
if [[ -z "$TB_PRIVATE_IPS" ]]; then
  TB_PRIVATE_IPS="${NODE1_IP},${NODE2_IP},${NODE3_IP}"
  warn "No --tb-private IPs given — using public IPs for TigerBeetle VSR network."
  warn "For production deployments, use a dedicated private VPC network."
fi

# ── Build TigerBeetle address string ─────────────────────────────────────────
IFS=',' read -r -a PRIV_IPS <<< "$TB_PRIVATE_IPS"
[[ ${#PRIV_IPS[@]} -eq 3 ]] \
  || die "--tb-private must be exactly 3 comma-separated IPs (got: $TB_PRIVATE_IPS)"

TB_ADDRESSES="${PRIV_IPS[0]}:3000,${PRIV_IPS[1]}:3001,${PRIV_IPS[2]}:3002"
BLAZIL_NODES="node-1:${PRIV_IPS[0]}:7878,node-2:${PRIV_IPS[1]}:7878,node-3:${PRIV_IPS[2]}:7878"

# ── Auto-generate secrets if not provided ────────────────────────────────────
if [[ -z "$TENANT_API_KEY" ]]; then
  TENANT_API_KEY="$(openssl rand -hex 32)"
  warn "tenant_api_key not provided — generated: ${TENANT_API_KEY}"
  warn "Save this key — it will NOT be shown again after deploy."
fi

if [[ -z "$ADMIN_TOKEN" ]]; then
  ADMIN_TOKEN="$(openssl rand -hex 24)"
  warn "admin_token not provided — generated: ${ADMIN_TOKEN}"
  warn "Save this token — it will NOT be shown again after deploy."
fi

[[ ${#TENANT_API_KEY} -ge 32 ]] \
  || die "tenant_api_key must be at least 32 characters for security"

# ── Generate per-tenant inventory ────────────────────────────────────────────
INVENTORY_OUT="${INVENTORY_DIR}/dedicated-${TENANT_ID}"

if [[ -f "$INVENTORY_OUT" ]]; then
  warn "Inventory already exists: ${INVENTORY_OUT} — overwriting."
fi

info "Generating inventory: ${INVENTORY_OUT}"
sed \
  -e "s|{{ TENANT_ID }}|${TENANT_ID}|g" \
  -e "s|{{ NODE_1_PUBLIC_IP }}|${NODE1_IP}|g" \
  -e "s|{{ NODE_2_PUBLIC_IP }}|${NODE2_IP}|g" \
  -e "s|{{ NODE_3_PUBLIC_IP }}|${NODE3_IP}|g" \
  -e "s|{{ SSH_KEY_PATH }}|${SSH_KEY}|g" \
  -e "s|{{ TB_ADDRESSES }}|${TB_ADDRESSES}|g" \
  -e "s|{{ BLAZIL_NODES }}|${BLAZIL_NODES}|g" \
  "${TEMPLATE}" > "${INVENTORY_OUT}"

ok "Inventory written to ${INVENTORY_OUT}"

# ── Build ansible-playbook command ───────────────────────────────────────────
ANSIBLE_CMD=(
  ansible-playbook
  -i "${INVENTORY_OUT}"
  "${PLAYBOOK}"
  -e "tenant_id=${TENANT_ID}"
  -e "tenant_api_key=${TENANT_API_KEY}"
  -e "admin_token=${ADMIN_TOKEN}"
  -e "tb_addresses=${TB_ADDRESSES}"
  -e "blazil_nodes=${BLAZIL_NODES}"
  -e "rdma_enabled=${RDMA_ENABLED}"
)

[[ -n "$EXTRA_TAGS" ]] && ANSIBLE_CMD+=(--tags "$EXTRA_TAGS")

# ── Execute ───────────────────────────────────────────────────────────────────
echo
echo -e "${BOLD}${BLUE}======================================================${RESET}"
echo -e "${BOLD}${BLUE}  Blazil dedicated deploy: ${TENANT_ID}${RESET}"
echo -e "${BOLD}${BLUE}======================================================${RESET}"
echo -e "  Nodes:     ${NODE1_IP}  ${NODE2_IP}  ${NODE3_IP}"
echo -e "  RDMA:      ${RDMA_ENABLED}"
echo -e "  Inventory: ${INVENTORY_OUT}"
echo -e "  Playbook:  ${PLAYBOOK}"
echo

DEPLOY_LOG="${REPO_ROOT}/logs/deploy_${TENANT_ID}_$(date +%Y%m%dT%H%M%S).log"
mkdir -p "${REPO_ROOT}/logs"

if $DRY_RUN; then
  warn "DRY RUN — command that would be executed:"
  echo "  ${ANSIBLE_CMD[*]}"
  exit 0
fi

info "Starting deploy — logging to ${DEPLOY_LOG}"
info "This will take 3–8 minutes for a fresh cluster."
echo

# Run ansible-playbook and tee output to log file.
# Use `script -q` pattern to preserve terminal colour codes in the log.
set +e
"${ANSIBLE_CMD[@]}" 2>&1 | tee "${DEPLOY_LOG}"
ANSIBLE_EXIT="${PIPESTATUS[0]}"
set -e

echo
if [[ "$ANSIBLE_EXIT" -ne 0 ]]; then
  die "Deploy FAILED (exit ${ANSIBLE_EXIT}). Full log: ${DEPLOY_LOG}"
fi

ok "Deploy SUCCEEDED for tenant: ${TENANT_ID}"
echo
echo -e "${BOLD}Post-deploy summary:${RESET}"
echo -e "  Engine:       ${NODE1_IP}:7878"
echo -e "  Grafana:      http://${NODE1_IP}:3001"
echo -e "  Metrics:      http://${NODE1_IP}:9090/metrics"
echo -e "  TLS CN:       ${TENANT_ID}.blazil.internal"
echo -e "  Tenant ID:    ${TENANT_ID}"
echo -e "  API key:      ${TENANT_API_KEY}"
echo -e "  Admin token:  ${ADMIN_TOKEN}"
echo -e "  Deploy log:   ${DEPLOY_LOG}"
echo
warn "Store the API key and admin token in your secrets manager now."
warn "These values are NOT recoverable from the server after this session."
