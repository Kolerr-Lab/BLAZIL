// Types for all WebSocket messages broadcast by blazil-bench and ml-bench binaries.

export type BenchStatus = "idle" | "connecting" | "running" | "completed" | "error";

// Fintech benchmark config (VSR consensus)
export interface FintechConfigMessage {
  type: "config";
  shards: number;
  duration_secs: number | null;
  rt_workers: number;
  tb_addr: string;
  capacity_per_shard: number;
  window_per_shard: number;
}

// AI dataloader benchmark config
export interface DataloaderConfigMessage {
  type: "config";
  mode: "dataloader";
  dataset: string;
  batch_size: number;
  workers: number;
  duration_secs: number;
  num_samples: number;
  num_classes: number;
}

// AI inference benchmark config
export interface InferenceConfigMessage {
  type: "config";
  mode: "inference";
  dataset: string;
  model: string;
  batch_size: number;
  workers: number;
  inference_workers: number;
  duration_secs: number;
  num_samples: number;
  num_classes: number;
}

export type ConfigMessage = FintechConfigMessage | DataloaderConfigMessage | InferenceConfigMessage;

// Fintech tick (per-shard TPS)
export interface FintechTickMessage {
  type: "tick";
  t: number;        // seconds since bench start
  shard_id: number;
  tps: number;      // events committed+rejected this second on this shard
  committed_total: number;
  rejected_total: number;
  inflight: number;
  sent_total: number;
  p50_us: number;   // rolling p50 in microseconds
  p99_us: number;   // rolling p99 in microseconds
}

// AI dataloader tick (samples/sec)
export interface DataloaderTickMessage {
  type: "tick";
  t: number;
  mode: "dataloader";
  samples_per_sec: number;
  total_samples: number;
  total_batches: number;
  total_errors: number;
}

// AI inference tick (RPS)
export interface InferenceTickMessage {
  type: "tick";
  t: number;
  mode: "inference";
  rps: number;
  samples_per_sec: number;
  total_samples: number;
  total_predictions: number;
  total_batches: number;
  total_errors: number;
}

export type TickMessage = FintechTickMessage | DataloaderTickMessage | InferenceTickMessage;

export interface EventMessage {
  type: "event";
  t: number;
  kind:
    | "bench_start"
    | "bench_done"
    | "warmup_start"
    | "warmup_done"
    | "node_down"
    | "node_up"
    | "drain"
    | "info";
  message: string;
}

// Fintech summary
export interface FintechSummaryMessage {
  type: "summary";
  total_committed: number;
  total_rejected: number;
  error_rate: number;
  survival_rate: number;
  tps: number;       // overall average TPS
  avg_tps: number;
  max_tps: number;
  min_tps: number;
  consistency: number;  // min_tps/max_tps * 100
  p50_ns: number;
  p99_ns: number;
  p999_ns: number;
  mean_ns: number;
  wall_secs: number;
  shards: number;
}

// AI dataloader summary
export interface DataloaderSummaryMessage {
  type: "summary";
  mode: "dataloader";
  total_samples: number;
  total_batches: number;
  total_errors: number;
  error_rate: number;
  samples_per_sec: number;
  batches_per_sec: number;
  p50_us: number;
  p99_us: number;
  p999_us: number;
  wall_secs: number;
}

// AI inference summary
export interface InferenceSummaryMessage {
  type: "summary";
  mode: "inference";
  total_samples: number;
  total_batches: number;
  total_predictions: number;
  total_errors: number;
  error_rate: number;
  rps: number;
  samples_per_sec: number;
  batches_per_sec: number;
  p50_us: number;
  p99_us: number;
  p999_us: number;
  wall_secs: number;
}

export type SummaryMessage = FintechSummaryMessage | DataloaderSummaryMessage | InferenceSummaryMessage;

export type BenchMessage =
  | ConfigMessage
  | TickMessage
  | EventMessage
  | SummaryMessage;

// Aggregated per-second snapshot (all shards combined).
export interface SecondSnapshot {
  t: number;
  tps: number;             // aggregate across all shards
  per_shard: { shard_id: number; tps: number }[];
  error_rate: number;
  p50_us: number;
  p99_us: number;
  total_committed: number;
  total_rejected: number;
}

// Per-shard live state.
export interface ShardState {
  shard_id: number;
  current_tps: number;
  committed_total: number;
  rejected_total: number;
  inflight: number;
  sent_total: number;
  p50_us: number;
  p99_us: number;
  last_tick_t: number;
  tps_history: number[]; // last 30 values for sparkline
}

// Complete dashboard state.
export interface DashboardState {
  status: BenchStatus;
  config: ConfigMessage | null;
  mode: "fintech" | "dataloader" | "inference";  // Detected from config
  elapsed_secs: number;
  history: SecondSnapshot[];     // per-second, up to 600 entries
  shards: Map<number, ShardState>;  // For fintech only
  events: EventMessage[];
  summary: SummaryMessage | null;
  // Current metrics (latest second)
  current_tps: number;            // TPS for fintech, samples/sec for dataloader, RPS for inference
  peak_tps: number;               // Peak throughput
  total_committed: number;        // Fintech: committed txns
  total_rejected: number;         // Fintech: rejected txns
  total_samples: number;          // AI: total samples processed
  total_predictions: number;      // AI inference: total predictions
  total_errors: number;           // AI: errors
  current_p50_us: number;
  current_p99_us: number;
}
