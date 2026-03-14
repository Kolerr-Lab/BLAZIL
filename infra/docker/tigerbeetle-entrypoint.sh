#!/bin/sh
# TigerBeetle entrypoint — supports single-replica (demo) and multi-replica
# (cluster) modes via environment variables.
#
# Single-replica (default — used by docker-compose.demo.yml):
#   No environment overrides needed. Formats and starts with replica=0,
#   replica-count=1, address=0.0.0.0:3000.
#
# Multi-replica (used by docker-compose.node-N.yml on DigitalOcean):
#   Set TB_REPLICA, TB_REPLICA_COUNT, and TB_ADDRESSES before starting.
#   TB_ADDRESSES should contain the REAL IPs of all replicas so that
#   cross-node communication works.
#   Example (node-1, whose private IP is 10.104.0.4):
#     TB_REPLICA=0
#     TB_REPLICA_COUNT=3
#     TB_ADDRESSES=10.104.0.4:3000,10.104.0.3:3001,10.104.0.2:3002
#
#   This script automatically replaces the local replica's IP with
#   "0.0.0.0:<port>" so TigerBeetle can bind() inside the Docker container
#   (the container's network interface doesn't have the host's private IP;
#   Docker's port publishing maps the container port to the host IP).
set -e

TB_REPLICA="${TB_REPLICA:-0}"
TB_REPLICA_COUNT="${TB_REPLICA_COUNT:-1}"
TB_ADDRESSES="${TB_ADDRESSES:-0.0.0.0:3000}"

DATA_FILE="/data/${TB_REPLICA}_0.tigerbeetle"

# ── Replace local address with 0.0.0.0:<port> ────────────────────────────────
# TigerBeetle bind()s to the address at index TB_REPLICA in the list.
# Inside Docker that address is the HOST's private IP, which doesn't exist
# on the container's loopback/bridge interface. We substitute 0.0.0.0 so
# TB binds to all container interfaces; Docker -p <hostport>:<containerport>
# then makes it reachable from the other nodes at the real host IP.
LOCAL_ADDRESSES=$(printf '%s' "$TB_ADDRESSES" | awk -v replica="$TB_REPLICA" '
{
  n = split($0, addrs, ",")
  for (i = 1; i <= n; i++) {
    if (i == replica + 1) {
      # keep the port, replace the host part with 0.0.0.0
      split(addrs[i], parts, ":")
      addrs[i] = "0.0.0.0:" parts[length(parts)]
    }
  }
  out = addrs[1]
  for (i = 2; i <= n; i++) out = out "," addrs[i]
  print out
}')

if [ ! -f "$DATA_FILE" ]; then
  echo "Formatting TigerBeetle data file (replica ${TB_REPLICA}/${TB_REPLICA_COUNT})..."
  /tigerbeetle format \
    --cluster=0 \
    --replica="${TB_REPLICA}" \
    --replica-count="${TB_REPLICA_COUNT}" \
    "$DATA_FILE"
  echo "Format complete."
fi

echo "Starting TigerBeetle (replica ${TB_REPLICA}, addresses: ${LOCAL_ADDRESSES})..."
exec /tigerbeetle start \
  --addresses="${LOCAL_ADDRESSES}" \
  "${DATA_FILE}"
