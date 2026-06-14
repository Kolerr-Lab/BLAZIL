#!/usr/bin/env bash
# CPU cloud benchmark harness for ClarkenAI inference (Aeron IPC path).
#
# Runs repeated inference requests against a local co-located server and also
# verifies the user-facing HTTP surface before reporting success.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
CONFIG_PATH="/tmp/blazil-inference-client.toml"
MODEL_PATH=""
RUNS=20
PROMPT="What is 2 + 2?"
MAX_TOKENS=16
READY_TIMEOUT_SECS=60
AERON_DIR="${BLAZIL_AERON_DIR:-/dev/shm/aeron-inference-hybrid}"
API_KEY="${BLAZIL_INFERENCE_API_KEY:-devkey}"
THREADS=8
INFERENCE_WORKERS=8
HTTP_PORT=8092
TENANT_ID="dashboard"
N_CTX=4096
TEMPERATURE=0.7
OPTIMIZATION_LEVEL=basic
MODEL_DIR=/tmp/blazil-models-client
HYBRID_ENABLED=false
STAGE1_END=25
STAGE2_END=60
TOTAL_LAYERS=80
BITNET_THRESHOLD=0.0
SKIP_BUILD=false
RUN_LABEL="default"
ARTIFACT_ROOT="$ROOT_DIR/docs/runs/clarkenai-cloud-bench"
TIMESTAMP="$(date +%Y-%m-%d_%H-%M-%S)"
ARTIFACT_DIR=""
SERVER_LOG=""
RESULT_LOG=""
SUMMARY_LOG=""
CONFIG_ARTIFACT=""
HEALTH_BODY=""
HTTP_HEADERS=""
HTTP_BODY=""
HTTP_SMOKE_LOG=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --config)
      CONFIG_PATH="$2"; shift 2 ;;
    --runs)
      RUNS="$2"; shift 2 ;;
    --model)
      MODEL_PATH="$2"; shift 2 ;;
    --threads)
      THREADS="$2"; shift 2 ;;
    --workers)
      INFERENCE_WORKERS="$2"; shift 2 ;;
    --http-port)
      HTTP_PORT="$2"; shift 2 ;;
    --tenant-id)
      TENANT_ID="$2"; shift 2 ;;
    --n-ctx)
      N_CTX="$2"; shift 2 ;;
    --temperature)
      TEMPERATURE="$2"; shift 2 ;;
    --optimization-level)
      OPTIMIZATION_LEVEL="$2"; shift 2 ;;
    --model-dir)
      MODEL_DIR="$2"; shift 2 ;;
    --hybrid-enabled)
      HYBRID_ENABLED="$2"; shift 2 ;;
    --stage1-end)
      STAGE1_END="$2"; shift 2 ;;
    --stage2-end)
      STAGE2_END="$2"; shift 2 ;;
    --total-layers)
      TOTAL_LAYERS="$2"; shift 2 ;;
    --bitnet-threshold)
      BITNET_THRESHOLD="$2"; shift 2 ;;
    --skip-build)
      SKIP_BUILD=true; shift 1 ;;
    --label)
      RUN_LABEL="$2"; shift 2 ;;
    --artifact-root)
      ARTIFACT_ROOT="$2"; shift 2 ;;
    --prompt)
      PROMPT="$2"; shift 2 ;;
    --max-tokens)
      MAX_TOKENS="$2"; shift 2 ;;
    --ready-timeout-secs)
      READY_TIMEOUT_SECS="$2"; shift 2 ;;
    --aeron-dir)
      AERON_DIR="$2"; shift 2 ;;
    --api-key)
      API_KEY="$2"; shift 2 ;;
    *)
      echo "Unknown arg: $1" >&2
      exit 1 ;;
  esac
done

ARTIFACT_DIR="$ARTIFACT_ROOT/${TIMESTAMP}_${RUN_LABEL}"
SERVER_LOG="$ARTIFACT_DIR/server.log"
RESULT_LOG="$ARTIFACT_DIR/results.log"
SUMMARY_LOG="$ARTIFACT_DIR/summary.txt"
CONFIG_ARTIFACT="$ARTIFACT_DIR/config.toml"
HEALTH_BODY="$ARTIFACT_DIR/health.txt"
HTTP_HEADERS="$ARTIFACT_DIR/http-chat.headers"
HTTP_BODY="$ARTIFACT_DIR/http-chat.json"
HTTP_SMOKE_LOG="$ARTIFACT_DIR/http-smoke.txt"

mkdir -p "$ARTIFACT_DIR"

if [[ ! -f "$CONFIG_PATH" ]]; then
  if [[ -z "$MODEL_PATH" ]]; then
    echo "Config not found: $CONFIG_PATH (provide --config or --model)" >&2
    exit 1
  fi
  if [[ ! -f "$MODEL_PATH" ]]; then
    echo "Model not found: $MODEL_PATH" >&2
    exit 1
  fi
  CONFIG_PATH="$CONFIG_ARTIFACT"
  cat > "$CONFIG_PATH" <<EOF
channel = "aeron:ipc?term-length=67108864"
aeron_dir = "$AERON_DIR"
model_path = "$MODEL_PATH"
inference_workers = $INFERENCE_WORKERS
device = "cpu"
optimization_level = "$OPTIMIZATION_LEVEL"
http_port = $HTTP_PORT
api_key = "\${BLAZIL_INFERENCE_API_KEY}"
model_dir = "$MODEL_DIR"

[gguf]
n_threads = $THREADS
n_ctx = $N_CTX
temperature = $TEMPERATURE
max_tokens = 2048

[distributed]
enabled = false
node_stage = 1
layer_start = 0
layer_end = 0
prev_stream_id = 0
next_stream_id = 0
assigned_cores = []
enable_spin_poll = true
enable_realtime_priority = false

[hybrid_matrix]
enabled = $HYBRID_ENABLED
stage1_end = $STAGE1_END
stage2_end = $STAGE2_END
total_layers = $TOTAL_LAYERS
bitnet_threshold = $BITNET_THRESHOLD
EOF
  echo "[bench] Generated temporary config: $CONFIG_PATH"
else
  cp "$CONFIG_PATH" "$CONFIG_ARTIFACT"
fi

cd "$ROOT_DIR"

mkdir -p "$MODEL_DIR"

for cmd in curl python3; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "[bench] Missing required command: $cmd" >&2
    exit 1
  fi
done

HTTP_BASE_URL="http://127.0.0.1:${HTTP_PORT}"

wait_for_health() {
  local http_code
  http_code=$(curl -sS -o "$HEALTH_BODY" -w "%{http_code}" "$HTTP_BASE_URL/health" || true)
  [[ "$http_code" == "200" ]] && grep -q '^OK$' "$HEALTH_BODY"
}

run_http_smoke() {
  local payload
  local prompt_json
  prompt_json=$(printf '%s' "$PROMPT" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')
  payload=$(printf '{"request_id":"http-smoke-001","prompt":%s,"max_tokens":%s}' "$prompt_json" "$MAX_TOKENS")

  local http_code
  http_code=$(curl -sS -D "$HTTP_HEADERS" -o "$HTTP_BODY" -w "%{http_code}" \
    -X POST "$HTTP_BASE_URL/v1/chat" \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer $API_KEY" \
    -H "X-Tenant-ID: $TENANT_ID" \
    --data "$payload")

  if [[ "$http_code" != "200" ]]; then
    echo "[bench] HTTP smoke failed with status $http_code" >&2
    cat "$HTTP_BODY" >&2 || true
    return 1
  fi

  python3 - "$HTTP_BODY" <<'PY' > "$HTTP_SMOKE_LOG"
import json
import sys
from pathlib import Path

body = json.loads(Path(sys.argv[1]).read_text())
output_text = body.get("output_text", "")
if not output_text.strip():
    raise SystemExit("empty output_text in HTTP smoke response")
print(f"request_id={body.get('request_id', '')}")
print(f"latency_us={body.get('latency_us', 0)}")
print(f"first_token_latency_us={body.get('first_token_latency_us', 0)}")
print(f"tokens_generated={body.get('tokens_generated', 0)}")
print(f"output_chars={len(output_text)}")
PY
}

echo "[bench] artifact_dir=$ARTIFACT_DIR"
echo "[bench] config_path=$CONFIG_PATH"
echo "[bench] threads=$THREADS workers=$INFERENCE_WORKERS hybrid_enabled=$HYBRID_ENABLED label=$RUN_LABEL opt=$OPTIMIZATION_LEVEL ready_timeout=${READY_TIMEOUT_SECS}s"

if [[ "$SKIP_BUILD" != true ]]; then
  echo "[bench] Building release binaries (inference-server + test-inference)..."
  cargo build --release -p blazil-inference-service -p test-inference >/dev/null
else
  echo "[bench] Skipping build (requested)"
fi

echo "[bench] Cleaning previous processes (safe no-op if none)..."
pkill -f 'target/release/inference-server' >/dev/null 2>&1 || true

echo "[bench] Starting inference server..."
BLAZIL_INFERENCE_API_KEY="$API_KEY" RUST_LOG=info \
  target/release/inference-server --config "$CONFIG_PATH" \
  >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!

cleanup() {
  if kill -0 "$SERVER_PID" >/dev/null 2>&1; then
    kill "$SERVER_PID" >/dev/null 2>&1 || true
    wait "$SERVER_PID" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

READY=0
for _ in $(seq 1 "$READY_TIMEOUT_SECS"); do
  if wait_for_health; then
    READY=1
    break
  fi
  if ! kill -0 "$SERVER_PID" >/dev/null 2>&1; then
    echo "[bench] Server exited early. Last log lines:" >&2
    tail -n 80 "$SERVER_LOG" >&2 || true
    exit 1
  fi
  sleep 1
done

if [[ "$READY" -ne 1 ]]; then
  echo "[bench] Server did not become ready in time. Last log lines:" >&2
  tail -n 120 "$SERVER_LOG" >&2 || true
  exit 1
fi

echo "[bench] Server ready. Verifying HTTP surface..."
if ! run_http_smoke; then
  echo "[bench] HTTP smoke failed. Server log tail:" >&2
  tail -n 120 "$SERVER_LOG" >&2 || true
  exit 1
fi

echo "[bench] HTTP smoke passed. Running $RUNS Aeron request(s)..."
: > "$RESULT_LOG"

for i in $(seq 1 "$RUNS"); do
  RID="bench-$(printf '%03d' "$i")"
  OUT=""
  if ! OUT=$(TEST_INFERENCE_AERON_DIR="$AERON_DIR" \
             TEST_INFERENCE_REQUEST_ID="$RID" \
             TEST_INFERENCE_PROMPT="$PROMPT" \
             TEST_INFERENCE_MAX_TOKENS="$MAX_TOKENS" \
             target/release/test-inference 2>&1); then
    echo "$OUT" >> "$RESULT_LOG"
    echo "[bench] Request $i failed: test-inference exited non-zero" >&2
    echo "$OUT" >&2
    exit 1
  fi

  echo "$OUT" >> "$RESULT_LOG"
  LINE=$(echo "$OUT" | grep 'BENCH_RESULT' | tail -n 1 || true)
  if [[ -z "$LINE" ]]; then
    echo "[bench] Request $i failed to produce BENCH_RESULT" >&2
    echo "$OUT" >&2
    exit 1
  fi

  LAT=$(echo "$LINE" | awk '{for (i=1; i<=NF; i++) if ($i ~ /^latency_ms=/) {sub("latency_ms=","",$i); print $i}}')
  TOK=$(echo "$LINE" | awk '{for (i=1; i<=NF; i++) if ($i ~ /^tokens=/) {sub("tokens=","",$i); print $i}}')
  TPS=$(echo "$LINE" | awk '{for (i=1; i<=NF; i++) if ($i ~ /^tokens_per_sec=/) {sub("tokens_per_sec=","",$i); print $i}}')

  printf '[bench] run=%s latency_ms=%s tokens=%s tokens_per_sec=%s\n' "$i" "$LAT" "$TOK" "$TPS"
done

TMP_LAT="$(mktemp)"
TMP_TPS="$(mktemp)"
grep 'BENCH_RESULT' "$RESULT_LOG" \
  | awk '{for (i=1; i<=NF; i++) if ($i ~ /^latency_ms=/) {sub("latency_ms=","",$i); print $i}}' \
  | sort -n > "$TMP_LAT"
grep 'BENCH_RESULT' "$RESULT_LOG" \
  | awk '{for (i=1; i<=NF; i++) if ($i ~ /^tokens_per_sec=/) {sub("tokens_per_sec=","",$i); print $i}}' \
  | sort -n > "$TMP_TPS"

COUNT=$(wc -l < "$TMP_LAT" | tr -d ' ')
if [[ "$COUNT" -eq 0 ]]; then
  echo "[bench] No latency samples captured." >&2
  exit 1
fi

pctl() {
  local p="$1"
  local idx
  idx=$(awk -v n="$COUNT" -v p="$p" 'BEGIN {v=int((p*n)+0.999999); if (v<1) v=1; if (v>n) v=n; print v}')
  sed -n "${idx}p" "$TMP_LAT"
}

P50=$(pctl 0.50)
P95=$(pctl 0.95)
P99=$(pctl 0.99)
MEAN=$(awk '{s+=$1} END {if (NR==0) print 0; else printf "%.2f", s/NR}' "$TMP_LAT")

tps_pctl() {
  local p="$1"
  local idx
  idx=$(awk -v n="$COUNT" -v p="$p" 'BEGIN {v=int((p*n)+0.999999); if (v<1) v=1; if (v>n) v=n; print v}')
  sed -n "${idx}p" "$TMP_TPS"
}

TPS_MEAN=$(awk '{s+=$1} END {if (NR==0) print 0; else printf "%.4f", s/NR}' "$TMP_TPS")
TPS_P50=$(tps_pctl 0.50)
TPS_P95=$(tps_pctl 0.95)
TPS_P99=$(tps_pctl 0.99)

rm -f "$TMP_LAT"
rm -f "$TMP_TPS"

{
  echo "===== ClarkenAI Cloud Bench Summary ====="
  echo "timestamp=$TIMESTAMP"
  echo "host=$(hostname)"
  echo "git_commit=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
  echo "http_base_url=$HTTP_BASE_URL"
  echo "label=$RUN_LABEL"
  echo "samples=$COUNT"
  echo "tenant_id=$TENANT_ID"
  echo "prompt=$PROMPT"
  echo "max_tokens=$MAX_TOKENS"
  echo "hybrid_enabled=$HYBRID_ENABLED"
  echo "stage1_end=$STAGE1_END"
  echo "stage2_end=$STAGE2_END"
  echo "total_layers=$TOTAL_LAYERS"
  echo "bitnet_threshold=$BITNET_THRESHOLD"
  echo "threads=$THREADS"
  echo "workers=$INFERENCE_WORKERS"
  echo "ready_timeout_secs=$READY_TIMEOUT_SECS"
  echo "optimization_level=$OPTIMIZATION_LEVEL"
  echo "model_dir=$MODEL_DIR"
  echo "health_body_file=$HEALTH_BODY"
  echo "http_smoke_headers=$HTTP_HEADERS"
  echo "http_smoke_body=$HTTP_BODY"
  echo "http_smoke_summary=$HTTP_SMOKE_LOG"
  echo "latency_ms_mean=$MEAN"
  echo "latency_ms_p50=$P50"
  echo "latency_ms_p95=$P95"
  echo "latency_ms_p99=$P99"
  echo "tokens_per_sec_mean=$TPS_MEAN"
  echo "tokens_per_sec_p50=$TPS_P50"
  echo "tokens_per_sec_p95=$TPS_P95"
  echo "tokens_per_sec_p99=$TPS_P99"
  echo "target_ms_per_token=100"
  echo "config_path=$CONFIG_PATH"
  echo "raw_result_log=$RESULT_LOG"
  echo "server_log=$SERVER_LOG"
} | tee "$SUMMARY_LOG"

echo ""
echo "[bench] summary log: $SUMMARY_LOG"
