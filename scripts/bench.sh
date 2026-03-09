#!/usr/bin/env bash

set -euo pipefail

# Blazil Benchmark Suite Runner

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Blazil Benchmark Suite"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Color codes
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'

print_status() {
    echo -e "${GREEN}✓${NC} $1"
}

print_info() {
    echo -e "${BLUE}ℹ${NC} $1"
}

# Check if running in release mode
if [[ "${1:-}" == "--release" ]]; then
    RELEASE_FLAG="--release"
    print_info "Running benchmarks in release mode"
else
    RELEASE_FLAG=""
    print_info "Running benchmarks in debug mode (use --release for optimized benchmarks)"
fi
echo ""

# Run Rust benchmarks
print_info "Running Rust benchmarks..."
echo ""
cargo bench --workspace $RELEASE_FLAG

print_status "Benchmark suite complete"
echo ""
echo "Results saved to: target/criterion/"
echo ""
echo "To view HTML reports:"
echo "  open target/criterion/report/index.html"
