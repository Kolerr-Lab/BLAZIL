# ClarkenAI 70B Cloud Bench Runbook

This is the canonical workflow for ClarkenAI CPU cloud benchmarking.

It replaces the older ONNX/ml-bench AWS benchmark documents and scripts. Do not use the retired `ai-aws-*`, `ai-benchmark.sh`, or `gen-ai-report.sh` path for Clarken validation.

## Scope

- Target runtime: `blazil-inference-service`
- Target launcher: `scripts/clarkenai-70b-bench.sh`
- Target harness: `scripts/clarkenai-cloud-bench.sh`
- Target setup: `scripts/clarkenai-aws-setup.sh`
- Target model fetch: `scripts/get-clarkenai-70b-model.sh`
- Target preset: `services/inference/inference-chat-70b-ready.toml`

## Preconditions

- Ubuntu 24.04 LTS or similar Linux host
- CPU-only instance sized for 70B-class GGUF testing
- Repo present on the host
- Enough free disk for the selected model profile
- A non-dev API key chosen before launch

## 1. Host Setup

```bash
cd /opt/blazil
sudo ./scripts/clarkenai-aws-setup.sh --repo-root /opt/blazil
```

What this must leave behind:

- Rust toolchain installed
- `hf` or `huggingface-cli` available
- tuned sysctl and file-descriptor limits applied
- `target/release/inference-server` built
- `target/release/test-inference` built
- model cache directory present
- Aeron runtime directory present

## 2. Fetch Model

```bash
cd /opt/blazil
./scripts/get-clarkenai-70b-model.sh --profile qwen72-q4km --model-dir /opt/clarkenai/models
```

Then export the runtime env:

```bash
export CLARKENAI_MODEL_DIR=/opt/clarkenai/models
export CLARKENAI_MODEL_PATH=/opt/clarkenai/models/Qwen2.5-72B-Instruct-Q4_K_M.gguf
export BLAZIL_AERON_DIR=/dev/shm/aeron-inference-hybrid
export BLAZIL_INFERENCE_API_KEY='<replace-with-a-real-secret>'
```

## 3. Optional Preset Boot

This verifies the env-driven preset directly.

```bash
cd /opt/blazil/services/inference
cargo run --release -- --config inference-chat-70b-ready.toml
```

## 4. Run Canonical Bench

```bash
cd /opt/blazil
./scripts/clarkenai-70b-bench.sh \
  --model "$CLARKENAI_MODEL_PATH" \
  --runs 10 \
  --max-tokens 32 \
  --label aws-i4i4xl-70b-core
```

What the harness verifies before reporting success:

- server process stays alive
- `GET /health` returns `200 OK`
- `POST /v1/chat` returns a non-empty `output_text`
- Aeron IPC requests return `BENCH_RESULT` samples
- summary, config, health, HTTP, and server logs are all persisted under the artifact directory

## 5. Artifact Standard

Each real run must create an artifact directory under one of these roots:

- `docs/runs/clarkenai-70b/`
- `docs/runs/clarkenai-cloud-bench/`

Minimum files expected from a valid run:

- `config.toml`
- `summary.txt`
- `server.log`
- `results.log`
- `health.txt`
- `http-chat.headers`
- `http-chat.json`
- `http-smoke.txt`

The `summary.txt` should include at least:

- timestamp
- host
- git commit
- label
- sample count
- prompt and max tokens
- hybrid split parameters
- latency percentiles
- tokens/sec percentiles
- artifact file paths for health and HTTP evidence

## 6. Readiness Gates

Only call a cloud run credible when all of these are true:

- setup completed without manual package rescue
- model fetch completed without missing CLI tooling
- preset boot works with env-driven values
- health probe passes
- HTTP smoke chat passes
- Aeron bench records more than a single sample for the target run
- artifacts are committed or copied off the instance intact

## 7. Retired Path

The following path is no longer authoritative for Clarken cloud validation:

- `docs/AI_AWS_QUICKSTART.md`
- `docs/AI_BASELINES.md`
- `docs/BENCHMARK_RESULTS_WORKFLOW.md`
- `scripts/ai-aws-setup.sh`
- `scripts/ai-benchmark.sh`
- `scripts/gen-ai-report.sh`
- `scripts/commit-bench-results.sh`

If you find an old reference to the retired path, remove it instead of following it.