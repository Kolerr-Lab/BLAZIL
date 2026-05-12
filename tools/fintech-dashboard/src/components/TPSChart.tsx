"use client";

import {
  AreaChart,
  Area,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
  ReferenceLine,
} from "recharts";
import type { SecondSnapshot } from "@/types/metrics";

interface Props {
  history: SecondSnapshot[];
  duration_secs: number | null;
  mode: "fintech" | "dataloader" | "inference";  // Add mode prop
}

function formatRate(v: number): string {
  if (v >= 1_000_000) return `${(v / 1_000_000).toFixed(1)}M`;
  if (v >= 1_000) return `${(v / 1_000).toFixed(0)}K`;
  return `${v}`;
}

interface TooltipPayloadEntry {
  value: number;
  dataKey: string;
}

function CustomTooltip(props: {
  active?: boolean;
  payload?: any[];
  label?: number;
  mode: "fintech" | "dataloader" | "inference";
}) {
  const { active, payload, label, mode } = props;
  if (!active || !payload?.length) return null;
  const rate = payload[0]?.value ?? 0;
  const rateLabel = mode === "fintech" ? "TPS" 
    : mode === "dataloader" ? "samples/sec"
    : "RPS";
  const showComparison = mode === "fintech" && rate > 0;
  
  return (
    <div
      className="px-3 py-2 text-xs rounded-lg font-mono"
      style={{
        background: "var(--bg-card)",
        border: "1px solid var(--border-bright)",
        color: "var(--text)",
      }}
    >
      <div style={{ color: "var(--text-muted)" }}>t+{label}s</div>
      <div style={{ color: "var(--accent-green)", fontSize: 14, fontWeight: 700 }}>
        {rate.toLocaleString()} {rateLabel}
      </div>
      {showComparison && (
        <div style={{ color: "var(--text-muted)" }}>
          {(rate / 24_000).toFixed(1)}× Visa
        </div>
      )}
    </div>
  );
}

export function TPSChart({ history, duration_secs, mode }: Props) {
  const data = history.map((h) => ({ t: h.t, rate: h.rate }));
  const maxRate = Math.max(...data.map((d) => d.rate), 1);
  // Nice Y-axis ceiling
  const yMax = Math.ceil((maxRate * 1.15) / 100_000) * 100_000 || 100_000;

  const rateLabel = mode === "fintech" ? "TPS"
    : mode === "dataloader" ? "Samples/sec"
    : "RPS";
  const subtitle = mode === "fintech" ? "Per-second, all shards combined"
    : mode === "dataloader" ? "Dataset streaming throughput"
    : "Inference requests per second";

  return (
    <div className="card p-5 flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <div>
          <div className="font-semibold text-sm text-white">
            Throughput — Aggregate {rateLabel}
          </div>
          <div className="text-xs mt-0.5" style={{ color: "var(--text-muted)" }}>
            {subtitle}
          </div>
        </div>
        {data.length > 0 && (
          <div
            className="text-xs font-mono px-3 py-1 rounded-full"
            style={{
              background: "rgba(0,229,160,0.08)",
              color: "var(--accent-green)",
              border: "1px solid rgba(0,229,160,0.15)",
            }}
          >
            {data.length}s recorded
          </div>
        )}
      </div>

      <div style={{ height: 240 }}>
        {data.length === 0 ? (
          <div className="h-full flex items-center justify-center">
            <div className="text-sm" style={{ color: "var(--text-muted)" }}>
              Waiting for bench data…
            </div>
          </div>
        ) : (
          <ResponsiveContainer width="100%" height="100%">
            <AreaChart data={data} margin={{ top: 4, right: 8, left: 0, bottom: 0 }}>
              <defs>
                <linearGradient id="tps-gradient" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor="var(--accent-green)" stopOpacity={0.3} />
                  <stop offset="100%" stopColor="var(--accent-green)" stopOpacity={0.01} />
                </linearGradient>
              </defs>
              <CartesianGrid strokeDasharray="3 3" stroke="var(--border)" />
              <XAxis
                dataKey="t"
                tickFormatter={(v: number) => `${v}s`}
                interval="preserveStartEnd"
                minTickGap={60}
              />
              <YAxis
                tickFormatter={formatRate}
                domain={[0, yMax]}
                width={48}
              />
              <Tooltip content={(props) => <CustomTooltip {...props} mode={mode} />} />
              {/* Visa peak reference line (fintech only) */}
              {mode === "fintech" && (
                <ReferenceLine
                  y={24_000}
                  stroke="rgba(239,68,68,0.4)"
                  strokeDasharray="4 4"
                  label={{
                    value: "Visa peak",
                    position: "insideTopRight",
                    fontSize: 10,
                    fill: "rgba(239,68,68,0.7)",
                  }}
                />
              )}
              {/* 1M TPS reference */}
              {yMax >= 800_000 && (
                <ReferenceLine
                  y={1_000_000}
                  stroke="rgba(168,85,247,0.4)"
                  strokeDasharray="4 4"
                  label={{
                    value: "1M TPS",
                    position: "insideTopRight",
                    fontSize: 10,
                    fill: "rgba(168,85,247,0.7)",
                  }}
                />
              )}
              {duration_secs && (
                <ReferenceLine
                  x={duration_secs}
                  stroke="rgba(59,130,246,0.4)"
                  strokeDasharray="4 4"
                />
              )}
              <Area
                type="monotone"
                dataKey="tps"
                stroke="var(--accent-green)"
                strokeWidth={2}
                fill="url(#tps-gradient)"
                dot={false}
                activeDot={{ r: 4, stroke: "var(--accent-green)", strokeWidth: 2, fill: "var(--bg)" }}
                isAnimationActive={false}
              />
            </AreaChart>
          </ResponsiveContainer>
        )}
      </div>
    </div>
  );
}
