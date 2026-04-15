// Types for all WebSocket messages broadcast by the blazil-bench binary.

export type BenchStatus = "idle" | "connecting" | "running" | "completed" | "error";

export interface ConfigMessage {
  type: "config";
  shards: number;
  duration_secs: number | null;
  rt_workers: number;
  tb_addr: string;
  capacity_per_shard: number;
  window_per_shard: number;
}

export interface TickMessage {
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

export interface SummaryMessage {
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
  elapsed_secs: number;
  history: SecondSnapshot[];     // per-second, up to 600 entries
  shards: Map<number, ShardState>;
  events: EventMessage[];
  summary: SummaryMessage | null;
  // Live aggregates across all shards for the current second.
  current_tps: number;
  peak_tps: number;
  total_committed: number;
  total_rejected: number;
  current_p50_us: number;
  current_p99_us: number;
}
