"use client";

import type { DashboardState } from "@/types/metrics";

interface Props {
  state: DashboardState;
}

function fmtNum(n: number): string {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(2) + "M";
  if (n >= 1_000) return (n / 1_000).toFixed(1) + "K";
  return n.toLocaleString();
}

function fmtLatency(us: number): string {
  if (us === 0) return "—";
  if (us >= 1_000_000) return (us / 1_000_000).toFixed(2) + " s";
  if (us >= 1_000) return (us / 1_000).toFixed(1) + " ms";
  return us.toLocaleString() + " µs";
}

interface BigCardProps {
  label: string;
  value: string;
  sub?: string;
  accent?: string;
  glow?: boolean;
}

function BigCard({ label, value, sub, accent = "var(--accent-green)", glow }: BigCardProps) {
  return (
    <div
      className="card flex-1 min-w-0 flex flex-col justify-between p-5"
      style={glow ? { boxShadow: `0 0 40px rgba(0,229,160,0.1)` } : undefined}
    >
      <div
        className="text-[10px] font-semibold tracking-widest uppercase"
        style={{ color: "var(--text-muted)" }}
      >
        {label}
      </div>
      <div
        className={`font-black text-3xl xl:text-4xl tracking-tight mt-1 tabular-nums ${glow ? "tps-glow" : ""}`}
        style={{ color: accent }}
      >
        {value}
      </div>
      {sub && (
        <div className="text-xs mt-1" style={{ color: "var(--text-muted)" }}>
          {sub}
        </div>
      )}
    </div>
  );
}

interface StatCardProps {
  label: string;
  value: string;
  accent?: string;
}

function StatCard({ label, value, accent = "var(--text)" }: StatCardProps) {
  return (
    <div
      className="card flex flex-col gap-1 p-4"
    >
      <div
        className="text-[10px] font-semibold tracking-widest uppercase"
        style={{ color: "var(--text-muted)" }}
      >
        {label}
      </div>
      <div
        className="font-bold text-xl tabular-nums"
        style={{ color: accent }}
      >
        {value}
      </div>
    </div>
  );
}

export function HeroMetrics({ state }: Props) {
  const { mode, current_tps, peak_tps, total_samples, total_predictions, total_errors, current_p50_us, current_p99_us, summary } = state;

  // AI-specific: dataloader shows samples/sec, inference shows RPS
  const throughputLabel = mode === 'inference' ? 'Request Rate' : 'Throughput';
  const peakLabel = mode === 'inference' ? 'Peak RPS' : 'Peak Samples/s';
  
  // Calculate bandwidth if summary available
  const bandwidth_gb_s = summary && 'bandwidth_gb_s' in summary ? summary.bandwidth_gb_s : 0;

  return (
    <div className="flex flex-col gap-4">
      {/* Top row: big 3 */}
      <div className="flex gap-4">
        <BigCard
          label={throughputLabel}
          value={current_tps > 0 ? fmtNum(current_tps) : "—"}
          sub={current_tps > 0 ? (mode === 'inference' ? 'requests/sec' : 'samples/sec') : "Waiting for data…"}
          glow={current_tps > 0}
        />
        <BigCard
          label={peakLabel}
          value={peak_tps > 0 ? fmtNum(peak_tps) : "—"}
          sub={peak_tps > 0 ? `Best second` : undefined}
          accent="var(--accent-blue)"
        />
        <BigCard
          label="Bandwidth"
          value={bandwidth_gb_s > 0 ? `${bandwidth_gb_s.toFixed(2)}` : "—"}
          sub={bandwidth_gb_s > 0 ? "GB/s (transport)" : "Run complete for bandwidth"}
          accent="var(--accent-purple)"
        />
      </div>

      {/* Bottom row: stats */}
      <div className="grid grid-cols-2 md:grid-cols-4 xl:grid-cols-6 gap-3">
        <StatCard
          label={mode === 'inference' ? 'Predictions' : 'Samples Loaded'}
          value={total_predictions !== undefined ? total_predictions.toLocaleString() : total_samples > 0 ? total_samples.toLocaleString() : "—"}
          accent="var(--accent-green)"
        />
        <StatCard
          label="Errors"
          value={total_errors.toLocaleString()}
          accent={total_errors > 0 ? "var(--accent-red)" : "var(--text-muted)"}
        />
        <StatCard
          label="Error Rate"
          value={total_samples > 0 ? `${((total_errors / total_samples) * 100).toFixed(3)}%` : "—"}
          accent={total_errors > 0 ? "var(--accent-red)" : "var(--accent-green)"}
        />
        <StatCard
          label="p50 Latency"
          value={fmtLatency(current_p50_us)}
          accent="var(--accent-blue)"
        />
        <StatCard
          label="p99 Latency"
          value={fmtLatency(current_p99_us)}
          accent="var(--accent-amber)"
        />
        <StatCard
          label="Consistency"
          value={(summary && 'consistency' in summary) ? `${summary.consistency.toFixed(1)}%` : state.history.length > 10 ? (() => {
            const tps = state.history.map(h => h.tps).filter(v => v > 0);
            const min = Math.min(...tps);
            const max = Math.max(...tps);
            return max > 0 ? `${(min / max * 100).toFixed(1)}%` : "—";
          })() : "—"}
          accent="var(--accent-purple)"
        />
      </div>
    </div>
  );
}
