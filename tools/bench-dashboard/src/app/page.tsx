"use client";

import { useState, useEffect, useCallback } from "react";
import { useBenchWS } from "@/hooks/useBenchWS";
import { Header } from "@/components/Header";
import { HeroMetrics } from "@/components/HeroMetrics";
import { TPSChart } from "@/components/TPSChart";
import { LatencyPanel } from "@/components/LatencyPanel";
import { FailoverPanel } from "@/components/FailoverPanel";
import { ClusterInfo } from "@/components/ClusterInfo";
import type { EventMessage } from "@/types/metrics";

const DEFAULT_WS_URL = "ws://13.229.63.205:9090/ws";

export default function DashboardPage() {
  const [wsUrl, setWsUrl] = useState(DEFAULT_WS_URL);
  const { state, connect, disconnect, sendCommand } = useBenchWS(wsUrl);

  // Auto-scroll event log to bottom.
  const eventsEndRef = useCallback((el: HTMLDivElement | null) => {
    el?.scrollIntoView({ behavior: "smooth" });
  }, []);

  // Auto-connect on mount, then retry every 3s on error/disconnect.
  useEffect(() => {
    connect();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (state.status !== "error" && state.status !== "idle") return;
    const timer = setTimeout(() => connect(), 3_000);
    return () => clearTimeout(timer);
  }, [state.status, connect]);

  // Browser title updates with live TPS.
  useEffect(() => {
    if (state.status === "running" && state.current_tps > 0) {
      const tps =
        state.current_tps >= 1_000_000
          ? `${(state.current_tps / 1_000_000).toFixed(2)}M`
          : `${(state.current_tps / 1_000).toFixed(0)}K`;
      document.title = `${tps} TPS — Blazil Bench`;
    } else {
      document.title = "Blazil Bench Dashboard";
    }
  }, [state.current_tps, state.status]);

  const perShard = [...state.shards.values()].sort((a, b) => a.shard_id - b.shard_id);

  return (
    <div className="min-h-screen flex flex-col" style={{ background: "var(--bg)" }}>
      <Header
        status={state.status}
        config={state.config}
        elapsedSecs={state.elapsed_secs}
        wsUrl={wsUrl}
        onWsUrlChange={setWsUrl}
        onConnect={connect}
        onDisconnect={disconnect}
      />

      <main className="flex-1 px-4 md:px-6 pb-8 pt-5 max-w-[1600px] mx-auto w-full">
        {/* Hero metrics row */}
        <HeroMetrics state={state} />

        {/* Infra spec strip */}
        <div className="mt-3">
          <ClusterInfo />
        </div>

        {/* TPS Chart + Latency side-by-side */}
        <div className="mt-5 grid grid-cols-1 xl:grid-cols-3 gap-4">
          <div className="xl:col-span-2">
            <TPSChart
              history={state.history}
              duration_secs={state.config?.duration_secs ?? null}
            />
          </div>
          <div className="xl:col-span-1">
            <LatencyPanel state={state} />
          </div>
        </div>

        {/* Per-shard cards */}
        {perShard.length > 0 && (
          <div className="mt-5">
            <div className="text-xs font-semibold uppercase tracking-widest mb-3" style={{ color: "var(--text-muted)" }}>
              Per-Shard Breakdown
            </div>
            <div className="grid grid-cols-2 md:grid-cols-4 xl:grid-cols-8 gap-3">
              {perShard.map((s) => {
                const errorRate =
                  s.committed_total + s.rejected_total > 0
                    ? (s.rejected_total / (s.committed_total + s.rejected_total)) * 100
                    : 0;
                const sparkMax = Math.max(...s.tps_history, 1);
                return (
                  <div
                    key={s.shard_id}
                    className="card p-3 flex flex-col gap-2"
                  >
                    <div className="flex items-center justify-between">
                      <div
                        className="text-[10px] font-semibold uppercase tracking-wider"
                        style={{ color: "var(--text-muted)" }}
                      >
                        Shard {s.shard_id}
                      </div>
                      <div
                        className="w-1.5 h-1.5 rounded-full"
                        style={{
                          background:
                            state.elapsed_secs - s.last_tick_t < 3
                              ? "var(--accent-green)"
                              : "var(--text-muted)",
                        }}
                      />
                    </div>
                    <div
                      className="font-black text-lg tabular-nums"
                      style={{ color: "var(--accent-green)" }}
                    >
                      {s.current_tps > 0
                        ? s.current_tps >= 1_000
                          ? `${(s.current_tps / 1_000).toFixed(0)}K`
                          : s.current_tps.toLocaleString()
                        : "—"}
                    </div>

                    {/* Sparkline bars */}
                    <div className="flex items-end gap-px h-6">
                      {s.tps_history.map((v, i) => (
                        <div
                          key={i}
                          className="flex-1 rounded-sm"
                          style={{
                            height: `${Math.max(8, (v / sparkMax) * 100)}%`,
                            background:
                              i === s.tps_history.length - 1
                                ? "var(--accent-green)"
                                : "var(--border-bright)",
                          }}
                        />
                      ))}
                    </div>

                    <div className="flex flex-col gap-0.5">
                      <div className="flex justify-between text-[10px]">
                        <span style={{ color: "var(--text-muted)" }}>in-flight</span>
                        <span className="font-mono" style={{ color: "var(--text)" }}>
                          {s.inflight.toLocaleString()}
                        </span>
                      </div>
                      <div className="flex justify-between text-[10px]">
                        <span style={{ color: "var(--text-muted)" }}>err rate</span>
                        <span
                          className="font-mono"
                          style={{
                            color:
                              errorRate > 0.1
                                ? "var(--accent-red)"
                                : "var(--accent-green)",
                          }}
                        >
                          {errorRate.toFixed(2)}%
                        </span>
                      </div>
                      <div className="flex justify-between text-[10px]">
                        <span style={{ color: "var(--text-muted)" }}>p99</span>
                        <span className="font-mono" style={{ color: "var(--accent-amber)" }}>
                          {s.p99_us > 0
                            ? s.p99_us >= 1_000_000
                              ? `${(s.p99_us / 1_000_000).toFixed(2)}s`
                              : s.p99_us >= 1_000
                              ? `${(s.p99_us / 1_000).toFixed(1)}ms`
                              : `${s.p99_us}µs`
                            : "—"}
                        </span>
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        )}

        {/* VSR Failover Panel */}
        <div className="mt-5">
          <div className="text-xs font-semibold uppercase tracking-widest mb-3" style={{ color: "var(--text-muted)" }}>
            Fault Tolerance
          </div>
          <FailoverPanel state={state} sendCommand={sendCommand} />
        </div>

        {/* Event log */}
        <div className="mt-5">
          <div className="text-xs font-semibold uppercase tracking-widest mb-3" style={{ color: "var(--text-muted)" }}>
            Event Log
          </div>
          <div
            className="card p-3 font-mono text-xs overflow-y-auto"
            style={{ maxHeight: 200, minHeight: 80 }}
          >
            {state.events.length === 0 ? (
              <div style={{ color: "var(--text-dim)" }}>
                No events yet. Connect to a running bench instance.
              </div>
            ) : (
              <>
                {state.events.map((ev: EventMessage, i) => (
                  <div
                    key={i}
                    className="flex gap-3 py-0.5"
                    style={{
                      color:
                        ev.kind === "node_down"
                          ? "var(--accent-red)"
                          : ev.kind === "node_up"
                          ? "var(--accent-green)"
                          : ev.kind === "bench_done"
                          ? "var(--accent-blue)"
                          : "var(--text-muted)",
                    }}
                  >
                    <span style={{ color: "var(--text-dim)" }}>t+{ev.t}s</span>
                    <span>{ev.message}</span>
                  </div>
                ))}
                <div ref={eventsEndRef} />
              </>
            )}
          </div>
        </div>

        {/* Summary panel (post-run) */}
        {state.summary && (
          <div className="mt-5">
            <div className="text-xs font-semibold uppercase tracking-widest mb-3" style={{ color: "var(--text-muted)" }}>
              Run Summary
            </div>
            <div className="card p-5 grid grid-cols-2 md:grid-cols-4 xl:grid-cols-8 gap-4">
              {[
                { label: "Average TPS", value: state.summary.avg_tps.toLocaleString(), accent: "var(--accent-green)" },
                { label: "Peak TPS", value: state.summary.max_tps.toLocaleString(), accent: "var(--accent-green)" },
                { label: "Min TPS", value: state.summary.min_tps.toLocaleString(), accent: "var(--text-muted)" },
                { label: "Consistency", value: `${state.summary.consistency.toFixed(1)}%`, accent: "var(--accent-purple)" },
                { label: "Committed", value: state.summary.total_committed.toLocaleString(), accent: "var(--accent-green)" },
                { label: "Rejected", value: state.summary.total_rejected.toLocaleString(), accent: state.summary.total_rejected > 0 ? "var(--accent-red)" : "var(--text-muted)" },
                { label: "Error Rate", value: `${state.summary.error_rate.toFixed(3)}%`, accent: state.summary.error_rate > 0.01 ? "var(--accent-red)" : "var(--accent-green)" },
                { label: "Survival Rate", value: `${state.summary.survival_rate.toFixed(2)}%`, accent: "var(--accent-green)" },
              ].map(({ label, value, accent }) => (
                <div key={label} className="flex flex-col gap-1">
                  <div className="text-[10px] uppercase font-semibold tracking-widest" style={{ color: "var(--text-muted)" }}>
                    {label}
                  </div>
                  <div className="font-bold text-lg tabular-nums" style={{ color: accent }}>
                    {value}
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}
      </main>
    </div>
  );
}
