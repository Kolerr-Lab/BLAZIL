#!/bin/bash
# commit-bench-results.sh — Automated workflow to commit AI benchmark results
#
# Usage:
#   ./scripts/commit-bench-results.sh <log-file>
#
# Example:
#   ./scripts/commit-bench-results.sh docs/benchmark-screenshots/ai-bench-2026-05-12_18-30-00.log
#
# This script:
#   1. Generates markdown report from raw log
#   2. Stages both log + report for commit
#   3. Creates descriptive commit message with key metrics
#   4. Optionally pushes to origin/main
set -e

if [ -z "$1" ]; then
  echo "Usage: $0 <log-file>"
  echo "Example: $0 docs/benchmark-screenshots/ai-bench-2026-05-12_18-30-00.log"
  exit 1
fi

LOG_FILE="$1"
BLAZIL_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$BLAZIL_ROOT"

if [ ! -f "$LOG_FILE" ]; then
  echo "Error: Log file not found: $LOG_FILE"
  exit 1
fi

echo "╔══════════════════════════════════════════════════════════╗"
echo "║   BLAZIL AI BENCHMARK — COMMIT RESULTS WORKFLOW         ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo ""

# Step 1: Generate markdown report
echo "▶ Step 1: Generating markdown report..."
./scripts/gen-ai-report.sh "$LOG_FILE"
echo ""

# Extract timestamp from filename
BASENAME=$(basename "$LOG_FILE" .log)
TIMESTAMP="${BASENAME#ai-bench-}"
REPORT_FILE="docs/runs/${TIMESTAMP}_ai-inference-aws-i4i4xl.md"

if [ ! -f "$REPORT_FILE" ]; then
  echo "Error: Report generation failed, file not found: $REPORT_FILE"
  exit 1
fi

echo "✅ Report generated: $REPORT_FILE"
echo ""

# Step 2: Extract key metrics for commit message
echo "▶ Step 2: Extracting key metrics..."
DATALOADER_THROUGHPUT=$(grep -oP 'Dataloader.*?Throughput.*?\*\*\K[\d,]+(?= samples/sec)' "$REPORT_FILE" | head -1 || echo "N/A")
SQUEEZENET_RPS=$(grep -oP 'SqueezeNet.*?Throughput.*?\*\*\K[\d,]+(?= inferences/sec)' "$REPORT_FILE" | head -1 || echo "N/A")
RESNET_RPS=$(grep -oP 'ResNet.*?Throughput.*?\*\*\K[\d,]+(?= inferences/sec)' "$REPORT_FILE" | head -1 || echo "N/A")
SLA_STATUS=$(grep -oP 'SLA Compliance.*?\*\*\K(PASS|FAIL)' "$REPORT_FILE" | head -1 || echo "N/A")

echo "  Dataloader: $DATALOADER_THROUGHPUT samples/sec"
echo "  SqueezeNet: $SQUEEZENET_RPS inferences/sec"
echo "  ResNet-50: $RESNET_RPS inferences/sec"
echo "  SLA: $SLA_STATUS"
echo ""

# Step 3: Stage files
echo "▶ Step 3: Staging files for commit..."
git add "$LOG_FILE"
git add "$REPORT_FILE"
echo "✅ Staged:"
git status --short | grep -E "($(basename "$LOG_FILE")|$(basename "$REPORT_FILE"))"
echo ""

# Step 4: Create commit message
echo "▶ Step 4: Creating commit message..."
COMMIT_MSG=$(cat << EOF
results(ai): AWS i4i.4xlarge benchmark ($TIMESTAMP)

Dataloader: ${DATALOADER_THROUGHPUT} samples/sec
SqueezeNet 1.1: ${SQUEEZENET_RPS} inferences/sec
ResNet-50: ${RESNET_RPS} inferences/sec
SLA compliance: ${SLA_STATUS}

Hardware: AWS i4i.4xlarge (16 vCPU, 128 GB RAM, NVMe)
Duration: 35 minutes (3 phases)
Commit: $(git rev-parse --short HEAD)

Files:
- Raw log: $(basename "$LOG_FILE")
- Report: $(basename "$REPORT_FILE")
EOF
)

echo "$COMMIT_MSG"
echo ""

# Step 5: Confirm commit
echo "▶ Step 5: Ready to commit. Proceed?"
read -p "   [y/N] " -n 1 -r
echo ""

if [[ ! $REPLY =~ ^[Yy]$ ]]; then
  echo "❌ Commit cancelled. Files remain staged."
  echo ""
  echo "To commit manually:"
  echo "  git commit -m 'results(ai): Add AWS benchmark results'"
  echo "  git push origin main"
  exit 0
fi

# Commit
git commit -m "$COMMIT_MSG"
echo "✅ Committed successfully"
echo ""

# Step 6: Push to remote
echo "▶ Step 6: Push to origin/main?"
read -p "   [y/N] " -n 1 -r
echo ""

if [[ $REPLY =~ ^[Yy]$ ]]; then
  git push origin main
  echo "✅ Pushed to origin/main"
  echo ""
  echo "🎉 Benchmark results published!"
  echo ""
  echo "View online:"
  echo "  - Report: https://github.com/Kolerr-Lab/BLAZIL/blob/main/$(basename "$REPORT_FILE")"
  echo "  - Raw log: https://github.com/Kolerr-Lab/BLAZIL/blob/main/$(basename "$LOG_FILE")"
else
  echo "⏳ Commit created locally. Push manually when ready:"
  echo "  git push origin main"
fi

echo ""
echo "╔══════════════════════════════════════════════════════════╗"
echo "║   WORKFLOW COMPLETE                                      ║"
echo "╚══════════════════════════════════════════════════════════╝"
