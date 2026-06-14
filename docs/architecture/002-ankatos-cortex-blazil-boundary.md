# Ankatos, Cortex v1, And Blazil Boundary

This note defines the target-state boundary after the Clarken chatbot SaaS pivot.

## Product Roles

### Ankatos

Ankatos is the product surface: an economic operating system for robotics.

Its responsibilities are:

- operator workflows
- task orchestration
- policy enforcement
- memory and world state
- fleet and economic coordination
- safety controls and auditability

### Cortex v1

Cortex v1 is the reasoning core that carries the Clarken identity.

Its responsibilities are:

- planning and reasoning
- identity and answer policy
- multimodal understanding where needed
- prompt and context assembly for robotic tasks
- bounded decision support for Ankatos

Runtime profiles currently map to:

- ClarkenAI 7B Spark for local smoke and lightweight lab work
- ClarkenAI 70B Core for cloud-grade reasoning
- ClarkenAI 70B Edge for constrained edge deployment

### Blazil Super Engine

Blazil is the runtime substrate behind Cortex inference.

Its responsibilities are:

- GGUF and ONNX execution
- transport hot path
- Aeron IPC and low-latency serving
- hybrid matrix quantization and kernel execution
- benchmark harnesses and evidence generation

## Boundary Rules

The old Clarken SaaS stack is no longer the product center of gravity.

- Web chat, subscriptions, email, and growth surfaces are legacy support surfaces unless explicitly reused by Ankatos.
- Blazil must remain independently benchable and deployable without the full Clarken SaaS stack.
- Cortex should be callable through a stable network boundary when used by Ankatos across hosts.
- Aeron IPC is an internal optimization for co-located runtimes, not the long-term cross-host contract.

## Deployment Modes

### 1. Bench Host

Use this when validating raw inference capability.

Deploy:

- Blazil inference runtime
- model artifacts
- benchmark scripts
- evidence capture

Do not deploy:

- legacy Clarken SaaS stack
- billing, auth, or conversation storage services

### 2. Cloud Control Plane

Use this when operating Ankatos centrally.

Deploy:

- Ankatos services
- Cortex orchestration layer
- policy, audit, memory, and operator services
- a network client for Blazil runtimes

Preferred contract to Blazil:

- HTTP/SSE for straightforward request-response and streaming
- gRPC when stronger typed contracts or bidirectional control are needed

Avoid:

- hard-coupling product services to Aeron IPC on remote hosts

### 3. Edge Node

Use this when a robot or local industrial node needs local reasoning.

Deploy selectively:

- Cortex v1 Edge profile
- Blazil edge runtime when latency or autonomy requires local inference
- local policy and safety envelope

Optionally rely on cloud Core when:

- latency budget allows it
- bandwidth is stable
- the task does not require hard local autonomy

## Immediate Implications For This Repo

- `services/inference` is the production inference core for Cortex v1.
- `tools/ai-dashboard` is an operator and lab console, not the final Ankatos product UI.
- `docs/runbooks/clarkenai-cloud-bench.md` defines inference-host validation only.
- Future integration work should prefer a stable network contract above Blazil instead of leaking co-located IPC assumptions into the product layer.