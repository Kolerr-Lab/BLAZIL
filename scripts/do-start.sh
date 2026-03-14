#!/bin/bash
# do-start.sh — Start a Blazil node on DigitalOcean
#
# Run after ALL 3 nodes have been provisioned with do-setup.sh.
# Pass the private VPC IPs of all 3 nodes.
#
# Each node runs ONE TigerBeetle replica + its own Blazil services:
#   node-1 → docker-compose.node-1.yml  (TB replica 0, shard 0, prometheus+grafana)
#   node-2 → docker-compose.node-2.yml  (TB replica 1, shard 1)
#   node-3 → docker-compose.node-3.yml  (TB replica 2, shard 2)
#
# Usage (on node-1):
#   BLAZIL_NODE_ID=node-1 BLAZIL_SHARD_ID=0 \
#     ./scripts/do-start.sh 10.0.0.1 10.0.0.2 10.0.0.3
#
# The script reads BLAZIL_NODE_ID / BLAZIL_SHARD_ID from .env.node if present.
set -e

INSTALL_DIR="$(cd "$(dirname "$0")/.." && pwd)"

# Load node-local env if it exists
if [ -f "$INSTALL_DIR/.env.node" ]; then
  # shellcheck disable=SC1091
  source "$INSTALL_DIR/.env.node"
fi

NODE_ID=${BLAZIL_NODE_ID:-"node-1"}
SHARD_ID=${BLAZIL_SHARD_ID:-"0"}
TB_NODE1=${1:-"10.0.0.1"}   # private IP node-1
TB_NODE2=${2:-"10.0.0.2"}   # private IP node-2
TB_NODE3=${3:-"10.0.0.3"}   # private IP node-3

# TigerBeetle VSR: each replica listens on its own port (3000/3001/3002)
TB_ADDRESSES="${TB_NODE1}:3000,${TB_NODE2}:3001,${TB_NODE3}:3002"
BLAZIL_NODES="node-1:${TB_NODE1}:7878,node-2:${TB_NODE2}:7878,node-3:${TB_NODE3}:7878"
LOCAL_IP=$(hostname -I | awk '{print $1}')

# Select per-node compose file — each node runs exactly ONE TB replica
case "$NODE_ID" in
  node-1) COMPOSE_FILE="infra/docker/docker-compose.node-1.yml" ;;
  node-2) COMPOSE_FILE="infra/docker/docker-compose.node-2.yml" ;;
  node-3) COMPOSE_FILE="infra/docker/docker-compose.node-3.yml" ;;
  *)
    echo "ERROR: Unknown BLAZIL_NODE_ID '$NODE_ID'. Expected node-1, node-2, or node-3." >&2
    exit 1
    ;;
esac

echo "═══════════════════════════════════════════════"
echo " Blazil Node Start: $NODE_ID (shard $SHARD_ID)"
echo "═══════════════════════════════════════════════"
echo " Compose file: $COMPOSE_FILE"
echo " TB cluster:   $TB_ADDRESSES"
echo " Blazil nodes: $BLAZIL_NODES"
echo " Local IP:     $LOCAL_IP"
echo "═══════════════════════════════════════════════"

cd "$INSTALL_DIR"

TB_ADDRESSES="$TB_ADDRESSES" \
BLAZIL_NODES="$BLAZIL_NODES" \
BLAZIL_NODE_ID="$NODE_ID" \
BLAZIL_SHARD_ID="$SHARD_ID" \
BLAZIL_AUTH_REQUIRED=false \
docker compose \
  -f "$COMPOSE_FILE" \
  up --build -d

echo ""
echo "✅ Node $NODE_ID started"
echo "   Engine:   ${LOCAL_IP}:7878"
echo "   Metrics:  http://${LOCAL_IP}:9090  (Prometheus)"
if [ "$NODE_ID" = "node-1" ]; then
  echo "   Grafana:  http://${LOCAL_IP}:3000  (admin / blazil)"
fi
echo ""
echo "To tail logs:  docker compose -f $COMPOSE_FILE logs -f"
