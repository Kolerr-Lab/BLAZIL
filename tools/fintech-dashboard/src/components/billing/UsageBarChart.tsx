"use client";

import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  Tooltip,
  ResponsiveContainer,
  CartesianGrid,
} from "recharts";
import type { UsageWindow } from "@/types/billing";

interface Props {
  windows: UsageWindow[];
}

export function UsageBarChart({ windows }: Props) {
  if (!windows?.length) {
    return (
      <p className="text-sm text-gray-400 py-4 text-center">
        No usage data for this period.
      </p>
    );
  }

  const data = windows.map((w) => ({
    label: new Date(w.window_start).toLocaleTimeString("en-US", {
      hour: "2-digit",
      minute: "2-digit",
      month: "short",
      day: "numeric",
    }),
    tx: w.tx_count,
  }));

  return (
    <ResponsiveContainer width="100%" height={220}>
      <BarChart data={data} margin={{ top: 4, right: 8, left: 0, bottom: 0 }}>
        <CartesianGrid strokeDasharray="3 3" stroke="#374151" />
        <XAxis
          dataKey="label"
          tick={{ fontSize: 10, fill: "#9ca3af" }}
          interval="preserveStartEnd"
        />
        <YAxis
          tick={{ fontSize: 10, fill: "#9ca3af" }}
          tickFormatter={(v: number) =>
            v >= 1_000_000
              ? `${(v / 1_000_000).toFixed(1)}M`
              : v >= 1_000
              ? `${(v / 1_000).toFixed(0)}K`
              : String(v)
          }
        />
        <Tooltip
          contentStyle={{ background: "#1f2937", border: "1px solid #374151", borderRadius: 6 }}
          labelStyle={{ color: "#f9fafb", fontSize: 11 }}
          itemStyle={{ color: "#60a5fa" }}
          formatter={(v: number) => [v.toLocaleString(), "Transactions"]}
        />
        <Bar dataKey="tx" fill="#3b82f6" radius={[3, 3, 0, 0]} />
      </BarChart>
    </ResponsiveContainer>
  );
}
