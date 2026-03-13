#!/bin/sh
# TigerBeetle entrypoint — supports single-replica (demo) and multi-replica
# (cluster) modes via environment variables.
#
# Single-replica (default — used by docker-compose.demo.yml):
#   No environment overrides needed. Formats and starts with replica=0,
#   replica-count=1, address=0.0.0.0:3000.
#
# Multi-replica (used by docker-compose.cluster.yml):
#   Set TB_REPLICA, TB_REPLICA_COUNT, and TB_ADDRESSES before starting.
#   Example:
#     TB_REPLICA=1
#     TB_REPLICA_COUNT=3
#     TB_ADDRESSES=tigerbeetle-0:3000,tigerbeetle-1:3001,tigerbeetle-2:3002
set -e

TB_REPLICA="${TB_REPLICA:-0}"
TB_REPLICA_COUNT="${TB_REPLICA_COUNT:-1}"
TB_ADDRESSES="${TB_ADDRESSES:-0.0.0.0:3000}"

DATA_FILE="/data/${TB_REPLICA}_0.tigerbeetle"

if [ ! -f "$DATA_FILE" ]; then
  echo "Formatting TigerBeetle data file (replica ${TB_REPLICA}/${TB_REPLICA_COUNT})..."
  /tigerbeetle format \
    --cluster=0 \
    --replica="${TB_REPLICA}" \
    --replica-count="${TB_REPLICA_COUNT}" \
    "$DATA_FILE"
  echo "Format complete."
fi

echo "Starting TigerBeetle (replica ${TB_REPLICA}, addresses: ${TB_ADDRESSES})..."
exec /tigerbeetle start \
  --addresses="${TB_ADDRESSES}" \
  "${DATA_FILE}"
