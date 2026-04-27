# Blazil Inference Service

Production-grade ML inference server using Aeron IPC transport for ultra-low latency.

## Architecture

```text
Client → Aeron:IPC (stream 2001) → InferenceServer
  → ONNX Model (Tract) → Result
  → Aeron:IPC (stream 2002) → Client
```

**Key Features:**
- **Aeron IPC Transport**: Shared-memory IPC, zero-copy, sub-microsecond latency
- **MessagePack Protocol**: Binary serialization, 30-50% smaller than JSON
- **Tract Engine**: Pure Rust ONNX inference, production-stable
- **Embedded C Driver**: In-process Aeron Media Driver, no external binaries
- **TransportServer Trait**: Consistent with Blazil fintech services

## Quick Start

### Build

```bash
# Requires Aeron C library (submodule)
git submodule update --init --recursive

# Build with Aeron feature
cargo build --release -p blazil-inference-service --features aeron
```

### Run

```bash
# Using config file
./target/release/inference-server --config config.toml

# Using CLI arguments
./target/release/inference-server \
  --model squeezenet1.1.onnx \
  --workers 8 \
  --optimization basic

# With environment variable logging
RUST_LOG=info ./target/release/inference-server --model model.onnx
```

## Configuration

Create `config.toml` (see [config.example.toml](config.example.toml)):

```toml
channel = "aeron:ipc?term-length=67108864"
aeron_dir = "/dev/shm/aeron-inference"
model_path = "/path/to/model.onnx"
inference_workers = 8
device = "cpu"
optimization_level = "basic"
enable_metrics = true
metrics_port = 9091
```

### Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `channel` | String | `aeron:ipc?term-length=67108864` | Aeron IPC channel URI |
| `aeron_dir` | String | `/dev/shm/aeron-inference` (Linux) | Aeron shared memory directory |
| `model_path` | PathBuf | - | Path to ONNX model file (required) |
| `inference_workers` | usize | `num_cpus` | Number of inference threads |
| `device` | String | `"cpu"` | Device: `cpu`, `cuda`, `tensorrt` |
| `optimization_level` | String | `"basic"` | Optimization: `disable`, `basic`, `extended`, `all` |
| `enable_metrics` | bool | `true` | Enable Prometheus metrics |
| `metrics_port` | u16 | `9091` | Metrics HTTP server port |

## Protocol

### InferenceRequest (MessagePack)

```rust
struct InferenceRequest {
    request_id: String,        // Correlation ID
    input_data: Vec<u8>,       // Raw image/tensor bytes
    input_shape: Vec<u32>,     // [H, W, C] or [B, C, H, W]
    model_version: String,     // Model version (future use)
}
```

### InferenceResponse (MessagePack)

```rust
struct InferenceResponse {
    request_id: String,        // Matches request
    class_id: Option<u32>,     // Predicted class (classification)
    probabilities: Vec<f32>,   // Class probabilities
    raw_output: Vec<f32>,      // Raw model output (if requested)
    confidence: f32,           // Max probability
    latency_us: u64,           // End-to-end latency (microseconds)
    error: String,             // Empty = success, non-empty = error
}
```

### Stream IDs

- **Requests** (client → server): Stream ID `2001`
- **Responses** (server → client): Stream ID `2002`

## Metrics

Prometheus metrics available at `http://localhost:9091/metrics`:

| Metric | Type | Description |
|--------|------|-------------|
| `inference_requests_total` | Counter | Total requests received |
| `inference_requests_success_total` | Counter | Successful inferences |
| `inference_requests_failed_total` | Counter | Failed inferences |
| `inference_predictions_total` | Counter | Total predictions generated |
| `inference_request_latency_microseconds` | Histogram | Request latency (µs) |
| `inference_active_requests` | Gauge | Currently active requests |
| `inference_aeron_offer_failures_total` | Counter | Aeron backpressure events |

Health check: `http://localhost:9091/health`

## Performance

**Target Performance:**
- **Latency**: P99 < 10ms (single inference, CPU)
- **Throughput**: 10K+ inferences/sec (batch mode)
- **Transport Overhead**: < 100µs (Aeron IPC)

**Tuning:**
- `inference_workers`: Match CPU cores for CPU inference
- `aeron_dir`: Use `/dev/shm` on Linux (tmpfs, zero page-fault)
- `term-length`: Larger buffer prevents backpressure (default 64 MB)
- `optimization_level`: `extended` or `all` for production

## Client Example (Rust)

```rust
use blazil_transport::aeron::{AeronContext, AeronPublication, AeronSubscription};
use blazil_inference_service::protocol::{
    InferenceRequest, InferenceResponse,
    serialize_request, deserialize_response,
    INFERENCE_REQ_STREAM_ID, INFERENCE_RSP_STREAM_ID,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let channel = "aeron:ipc?term-length=67108864";
    let aeron_dir = "/dev/shm/aeron-inference";

    // Create Aeron context
    let ctx = AeronContext::new(aeron_dir)?;

    // Create publication (send requests)
    let pub_ = AeronPublication::new(&ctx, channel, INFERENCE_REQ_STREAM_ID)?;
    pub_.wait_for_connected(Duration::from_secs(5))?;

    // Create subscription (receive responses)
    let sub = AeronSubscription::new(&ctx, channel, INFERENCE_RSP_STREAM_ID)?;
    sub.wait_for_available_image(Duration::from_secs(5))?;

    // Send inference request
    let req = InferenceRequest {
        request_id: "req-001".to_string(),
        input_data: vec![0u8; 224 * 224 * 3], // 224x224 RGB image
        input_shape: vec![224, 224, 3],
        model_version: "v1".to_string(),
    };

    let req_bytes = serialize_request(&req)?;
    pub_.offer(&req_bytes)?;

    // Receive response
    sub.poll_fragments(1, |buffer, _header| {
        let resp: InferenceResponse = deserialize_response(buffer).unwrap();
        println!("Class: {:?}, Confidence: {:.2}", resp.class_id, resp.confidence);
    });

    Ok(())
}
```

## Integration with Blazil Services

This inference service follows the same architecture as Blazil's banking/trading services:

- **Transport**: Aeron IPC (shared with fintech line)
- **Protocol**: MessagePack (consistent binary serialization)
- **Pattern**: TransportServer trait (uniform interface)
- **Deployment**: Standalone binary, Kubernetes-ready

**Future Integration:**
- Banking: Fraud detection via inference pipeline
- Trading: Risk scoring for order validation
- Crypto: Anomaly detection for wallet transactions

## Development

### Run Tests

```bash
cargo test -p blazil-inference-service
```

### Run Clippy

```bash
cargo clippy -p blazil-inference-service --all-targets -- -D warnings
```

### Benchmarking

Use `ml-bench` tool with inference mode:

```bash
cargo run --release --bin ml-bench -- \
  --mode inference \
  --model squeezenet1.1.onnx \
  --inference-workers 8
```

## License

BSL-1.1 - See [LICENSE](../../LICENSE)
