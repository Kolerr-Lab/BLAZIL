#!/usr/bin/env bash
# scripts/shard-bench.sh — Shard scaling benchmark runner.
#
# Runs the sharded pipeline benchmark at 1, 2, 4 and 8 shards and prints
# the Criterion throughput/time summary for each configuration.
#
# Usage:
#   bash scripts/shard-bench.sh          # from repo root
#   chmod +x scripts/shard-bench.sh && scripts/shard-bench.sh
#
# Requirements:
#   - Linux or macOS
#   - Rust toolchain in PATH
#   - Run from the repository root
set -e

# Aeron IPC tuning: 64 MB term buffer and 64 KB MTU for maximum throughput.
# These env vars are read by the embedded Aeron driver at startup.
export AERON_TERM_BUFFER_LENGTH=134217728  # 128 MB — power-of-two, 2× the proven 64 MB baseline,
                                            # safe on macOS (file-backed mmap under /tmp/aeron-*).
                                            # 256 MB caused mmap-init hang on macOS Ventura+.
export AERON_IPC_MTU_LENGTH=65536          # 64 KB

echo "=== Shard scaling benchmark ==="
for N in 1 2 4 8; do
  echo "--- $N shard(s) ---"
  BLAZIL_SHARD_COUNT=$N cargo bench \
    --bench sharded_pipeline_scenario 2>&1 | grep -E "TPS|thrpt|time"
done
