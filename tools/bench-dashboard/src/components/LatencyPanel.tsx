"use client";

import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
} from "recharts";
import type { SummaryMessage, DashboardState } from "@/types/metrics";

interface Props {
  state: DashboardState;
}

function fmtLatency(ns: number): string {
  if (ns === 0) return "—";
  const ms = ns / 1_000_000;
  if (ms >= 1000) return `${(ms / 1000).toFixed(2)}s`;
  return `${ms.toFixed(0)}ms`;
}

function fmtLatencyUs(us: number): string {
  if (us === 0) return "—";
  const ms = us / 1_000;
  if (ms >= 1000) return `${(ms / 1000).toFixed(2)}s`;
  return `${ms.toFixed(0)}ms`;
}

function LatencyRow({
  label,
  value,
  accent,
  bar,
  max,
}: {
  label: string;
  value: string;
  accent: string;
  bar: number;
  max: number;
}) {
  return (
    <div className="flex items-center gap-3">
      <div className="w-12 text-[10px] font-semibold uppercase" style={{ color: "var(--text-muted)" }}>
        {label}
      </div>
      <div className="flex-1 h-1.5 rounded-full" style={{ background: "var(--border)" }}>
        <div
          className="h-full rounded-full transition-all duration-500"
          style={{
            width: max > 0 ? `${Math.min((bar / max) * 100, 100)}%` : "0%",
            background: accent,
          }}
        />
      </div>
      <div className="w-16 text-right font-mono text-xs font-semibold" style={{ color: accent }}>
        {value}
      </div>
    </div>
  );
}

function LiveGauge({
  label,
  valueUs,
  accent,
}: {
  label: string;
  valueUs: number;
  accent: string;
}) {
  return (
    <div
      className="flex flex-col gap-1 p-3 rounded-lg"
      style={{ background: "rgba(255,255,255,0.02)", border: "1px solid var(--border)" }}
    >
      <div className="text-[10px] uppercase font-semibold" style={{ color: "var(--text-muted)" }}>
        {label}
      </div>
      <div className="font-mono font-bold text-lg" style={{ color: accent }}>
        {fmtLatencyUs(valueUs)}
      </div>
    </div>
  );
}

export function LatencyPanel({ state }: Props) {
  const { summary, current_p50_us, current_p99_us } = state;

  // Type guard for fintech summary (has p50_ns) vs AI summary (has p50_us)
  const p50_ns = (summary && 'p50_ns' in summary) ? summary.p50_ns : (summary && 'p50_us' in summary) ? summary.p50_us * 1000 : 0;
  const p99_ns = (summary && 'p99_ns' in summary) ? summary.p99_ns : (summary && 'p99_us' in summary) ? summary.p99_us * 1000 : 0;
  const p999_ns = (summary && 'p999_ns' in summary) ? summary.p999_ns : (summary && 'p999_us' in summary) ? summary.p999_us * 1000 : 0;
  const mean_ns = (summary && 'mean_ns' in summary) ? summary.mean_ns : 0;
  const maxBar = p999_ns;

  // Per-second latency history for sparkbar chart.
  const latData = state.history
    .filter((h) => h.p50_us > 0 || h.p99_us > 0)
    .slice(-60)
    .map((h) => ({
      t: h.t,
      p50: Math.round(h.p50_us / 1_000),
      p99: Math.round(h.p99_us / 1_000),
    }));

  return (
    <div className="card p-5 flex flex-col gap-4">
      <div>
        <div className="font-semibold text-sm text-white">Latency</div>
        <div className="text-xs mt-0.5" style={{ color: "var(--text-muted)" }}>
          End-to-end per transaction (ring buffer → TigerBeetle VSR → ack)
        </div>
      </div>

      {/* Live rolling estimates */}
      <div className="grid grid-cols-2 gap-2">
        <LiveGauge label="p50 (rolling)" valueUs={current_p50_us} accent="var(--accent-blue)" />
        <LiveGauge label="p99 (rolling)" valueUs={current_p99_us} accent="var(--accent-amber)" />
      </div>

      {/* Final percentiles after run */}
      {summary ? (
        <div className="flex flex-col gap-3">
          <div className="text-xs font-semibold" style={{ color: "var(--text-muted)" }}>
            Final Percentiles
          </div>
          <LatencyRow label="mean" value={fmtLatency(mean_ns)} accent="var(--text-muted)" bar={mean_ns} max={maxBar} />
          <LatencyRow label="p50" value={fmtLatency(p50_ns)} accent="var(--accent-blue)" bar={p50_ns} max={maxBar} />
          <LatencyRow label="p99" value={fmtLatency(p99_ns)} accent="var(--accent-amber)" bar={p99_ns} max={maxBar} />
          <LatencyRow label="p99.9" value={fmtLatency(p999_ns)} accent="var(--accent-red)" bar={p999_ns} max={maxBar} />
        </div>
      ) : (
        <div className="text-xs" style={{ color: "var(--text-muted)" }}>
          Final percentiles available after run completes.
        </div>
      )}

      {/* Per-second p50/p99 chart */}
      {latData.length > 2 && (
        <div>
          <div className="text-xs mb-2 font-semibold" style={{ color: "var(--text-muted)" }}>
            p50 / p99 per second (ms)
          </div>
          <div style={{ height: 100 }}>
            <ResponsiveContainer width="100%" height="100%">
              <BarChart data={latData} margin={{ top: 0, right: 0, left: 0, bottom: 0 }} barGap={0}>
                <CartesianGrid strokeDasharray="2 2" stroke="var(--border)" />
                <XAxis dataKey="t" hide />
                <YAxis width={32} tickFormatter={(v: number) => `${v}`} />
                <Tooltip
                  formatter={(v: number, name: string) => [`${v}ms`, name]}
                  contentStyle={{
                    background: "var(--bg-card)",
                    border: "1px solid var(--border-bright)",
                    borderRadius: 8,
                    color: "var(--text)",
                    fontSize: 11,
                  }}
                />
                <Bar dataKey="p50" fill="var(--accent-blue)" opacity={0.7} isAnimationActive={false} />
                <Bar dataKey="p99" fill="var(--accent-amber)" opacity={0.7} isAnimationActive={false} />
              </BarChart>
            </ResponsiveContainer>
          </div>
        </div>
      )}
    </div>
  );
}
