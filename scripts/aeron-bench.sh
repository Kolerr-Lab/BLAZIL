#!/usr/bin/env bash
# scripts/aeron-bench.sh
#
# Run the Aeron IPC E2E benchmark.
#
# Requirements:
#   • git submodule update --init --recursive   (Aeron C source)
#   • cmake, g++ installed                       (C library build)
#   • Rust 1.85+
#
# Usage:
#   ./scripts/aeron-bench.sh [EVENTS]
#
# Examples:
#   ./scripts/aeron-bench.sh             # 100 000 events (default)
#   ./scripts/aeron-bench.sh 1000000     # 1 M events

set -euo pipefail

EVENTS="${1:-100000}"
PAYLOAD_SIZE="${PAYLOAD_SIZE:-128}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

echo "=== Blazil Aeron Benchmark ==="
echo "Events  : ${EVENTS}"
echo "Payload : ${PAYLOAD_SIZE} bytes"
echo "Root    : ${WORKSPACE_ROOT}"
echo ""

# Verify submodule is present.
if [[ ! -f "${WORKSPACE_ROOT}/core/aeron-sys/aeron/CMakeLists.txt" ]]; then
  echo "ERROR: Aeron C submodule not found." >&2
  echo "Run: git submodule update --init --recursive" >&2
  exit 1
fi

cd "${WORKSPACE_ROOT}"

# Build (includes building the static C library via build.rs).
echo "Building with --features aeron…"
cargo build --release --features aeron -p blazil-bench

# Run the bench binary.
echo ""
echo "Running Aeron IPC E2E benchmark (${EVENTS} events, ${PAYLOAD_SIZE}B payload)…"
cargo run --release --features aeron --bin blazil-bench \
  -- --scenario aeron --events "${EVENTS}" --payload-size "${PAYLOAD_SIZE}"
