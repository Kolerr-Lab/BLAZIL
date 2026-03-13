#!/bin/bash
# cluster.sh — Start the Blazil 3-node cluster on a single machine.
#
# Simulates a real multi-node deployment using Docker Compose networks.
# Each node runs a dedicated blazil-engine (shard owner) and blazil-payments
# connected to a shared 3-replica TigerBeetle cluster.
#
# Requirements:
#   - Docker with Compose v2 installed
#   - Ports 3000-3002, 7878-7880, 50051, 50061, 50071, 9090, 9095-9097, 3010 free
#
# Usage:
#   ./scripts/cluster.sh
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
COMPOSE_FILE="${REPO_ROOT}/infra/docker/docker-compose.cluster.yml"

echo "🔥 Blazil Cluster — 3-node setup"
echo "================================="
echo ""
echo "  TigerBeetle cluster: 3 replicas (VSR consensus)"
echo "  Blazil nodes: 3 (engine + payments per node)"
echo ""

docker compose \
  -f "${COMPOSE_FILE}" \
  up --build -d

echo ""
echo "Waiting for cluster to stabilise..."
sleep 10

echo ""
echo "✅ Blazil Cluster is running!"
echo ""
echo "  Node 1 (shard 0): localhost:50051"
echo "  Node 2 (shard 1): localhost:50061"
echo "  Node 3 (shard 2): localhost:50071"
echo ""
echo "  Grafana:    http://localhost:3010  (admin/blazil)"
echo "  Prometheus: http://localhost:9090"
echo ""
echo "  TigerBeetle cluster:"
echo "    Replica 0: localhost:3000"
echo "    Replica 1: localhost:3001"
echo "    Replica 2: localhost:3002"
echo ""
echo "  Engine metrics:"
echo "    Node 1: http://localhost:9095/metrics"
echo "    Node 2: http://localhost:9096/metrics"
echo "    Node 3: http://localhost:9097/metrics"
echo ""
echo "Press Ctrl+C to stop tailing logs. Cluster continues running."
echo "To stop: docker compose -f infra/docker/docker-compose.cluster.yml down"
echo ""

docker compose \
  -f "${COMPOSE_FILE}" \
  logs -f
