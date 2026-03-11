#!/usr/bin/env bash

set -euo pipefail

echo "🔥 Blazil Benchmark Suite"
echo "========================="
echo ""

# Run custom benchmark suite (main scenarios, must use --release)
echo "Running benchmark scenarios..."
cargo run -p blazil-bench --release -- 2>&1

echo ""

# Run Criterion micro-benchmarks
echo "Running Criterion micro-benchmarks..."
cargo bench -p blazil-bench 2>&1

echo ""
echo "✅ Benchmark complete."
echo "Results saved to: target/criterion/"
