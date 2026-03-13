#!/usr/bin/env bash
# scripts/stress.sh — start the demo stack and run the full stress test suite.
# Writes docs/stress-report.md when done.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT="${REPO_ROOT}/docs/stress-report.md"
DURATION="${STRESS_DURATION:-60s}"
TARGET="${STRESS_TARGET:-localhost:50051}"

echo "=== Blazil Stress Test ==="
echo "Starting demo stack..."
docker compose \
  -f "${REPO_ROOT}/infra/docker/docker-compose.demo.yml" \
  up --build -d

# Wait for the payments service to be accepting connections.
echo "Waiting for payments service to be ready..."
for i in $(seq 1 30); do
  if nc -z localhost 50051 2>/dev/null; then
    echo "  payments service ready (attempt ${i})"
    break
  fi
  sleep 2
done

# Extra stabilisation time.
sleep 5

echo ""
echo "Running stress test (duration=${DURATION})..."
cd "${REPO_ROOT}"
go run ./tools/stresstest/. \
  --target="${TARGET}" \
  --duration="${DURATION}" \
  --mode=local \
  --report="${REPORT}"

echo ""
echo "Tearing down demo stack..."
docker compose \
  -f "${REPO_ROOT}/infra/docker/docker-compose.demo.yml" \
  down --remove-orphans

echo ""
echo "Done. Report: ${REPORT}"
