#!/usr/bin/env bash
# io-uring-bench.sh — Run the io_uring UDP transport benchmark.
#
# Requirements:
#   - Linux 5.1+ (io_uring support)
#   - Build in --release for meaningful numbers
#
# Usage:
#   ./scripts/io-uring-bench.sh [EVENTS]
#
# EVENTS defaults to 100000.

set -euo pipefail

EVENTS="${1:-100000}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ "$(uname -s)" != "Linux" ]]; then
    echo "❌  io_uring UDP transport requires Linux 5.1+. Current OS: $(uname -s)" >&2
    exit 1
fi

echo "🚀  Running io_uring UDP benchmark (${EVENTS} events)..."
echo "    Kernel: $(uname -r)"
echo "    Features: io-uring"
echo ""

cd "$REPO_ROOT"

cargo run \
    -p blazil-bench \
    --release \
    --features io-uring \
    -- "$EVENTS" 2>&1

echo ""
echo "✅  io_uring UDP benchmark complete."
