"use client";

import { useState, useEffect, useCallback } from "react";
import { useBenchWS } from "@/hooks/useBenchWS";
import { Header } from "@/components/Header";
import { HeroMetrics } from "@/components/HeroMetrics";
import { TPSChart } from "@/components/TPSChart";
import { LatencyPanel } from "@/components/LatencyPanel";
import type { EventMessage } from "@/types/metrics";

// ═══════════════════════════════════════════════════════════════
// BLAZIL AI INFERENCE BENCHMARK DASHBOARD
// ═══════════════════════════════════════════════════════════════
// Port: 3333 (different from fintech:3331)
// WebSocket: ws://localhost:9092/ws (AI only, NOT fintech 9090)
// Benchmark duration: 120s default
// ═══════════════════════════════════════════════════════════════
const DEFAULT_WS_URL = "ws://localhost:9092/ws";

export default function DashboardPage() {
  const [wsUrl, setWsUrl] = useState(DEFAULT_WS_URL);
  const { state, connect, disconnect, sendCommand } = useBenchWS(wsUrl);

  // Auto-scroll event log to bottom.
  const eventsEndRef = useCallback((el: HTMLDivElement | null) => {
    el?.scrollIntoView({ behavior: "smooth" });
  }, []);

  // Auto-connect on mount
  useEffect(() => {
    connect();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // Exponential backoff retry: delay doubles on each failure (5s → 10s → 20s → 40s → 60s max)
  useEffect(() => {
    if (state.status !== "error" && state.status !== "idle") return;
    // Retry delay is managed in useBenchWS hook with exponential backoff
    const timer = setTimeout(() => connect(), 5_000);
    return () => clearTimeout(timer);
  }, [state.status, connect]);

  // Browser title updates with live throughput.
  useEffect(() => {
    if (state.status === "running" && state.current_tps > 0) {
      const throughput =
        state.current_tps >= 1_000_000
          ? `${(state.current_tps / 1_000_000).toFixed(2)}M`
          : `${(state.current_tps / 1_000).toFixed(0)}K`;
      const label = state.mode === 'inference' ? 'RPS' : 'Samples/s';
      document.title = `${throughput} ${label} — Blazil AI`;
    } else {
      document.title = "Blazil AI Dashboard";
    }
  }, [state.current_tps, state.status, state.mode]);

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

        {/* Throughput Chart + Latency side-by-side */}
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
                        ev.kind === "fault_inject"
                          ? "var(--accent-red)"
                          : ev.kind === "fault_recover"
                          ? "var(--accent-green)"
                          : ev.kind === "node_down"
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
              {(() => {
                /* Fintech mode summary */
                if ('avg_tps' in state.summary) {
                  const s = state.summary;
                  return [
                    { label: "Average TPS", value: s.avg_tps.toLocaleString(), accent: "var(--accent-green)" },
                    { label: "Peak TPS", value: s.max_tps.toLocaleString(), accent: "var(--accent-green)" },
                    { label: "Min TPS", value: s.min_tps.toLocaleString(), accent: "var(--text-muted)" },
                    { label: "Consistency", value: `${s.consistency.toFixed(1)}%`, accent: "var(--accent-purple)" },
                    { label: "Committed", value: s.total_committed.toLocaleString(), accent: "var(--accent-green)" },
                    { label: "Rejected", value: s.total_rejected.toLocaleString(), accent: s.total_rejected > 0 ? "var(--accent-red)" : "var(--text-muted)" },
                    { label: "Error Rate", value: `${s.error_rate.toFixed(3)}%`, accent: s.error_rate > 0.01 ? "var(--accent-red)" : "var(--accent-green)" },
                    { label: "Survival Rate", value: `${s.survival_rate.toFixed(2)}%`, accent: "var(--accent-green)" },
                  ];
                }
                /* Inference mode summary */
                if ('rps' in state.summary) {
                  const s = state.summary;
                  return [
                    { label: "RPS", value: s.rps.toFixed(0), accent: "var(--accent-green)" },
                    { label: "Bandwidth", value: `${s.bandwidth_gb_s.toFixed(2)} GB/s`, accent: "var(--accent-blue)" },
                    { label: "Total Data", value: `${s.total_gb.toFixed(1)} GB`, accent: "var(--text-muted)" },
                    { label: "Total Predictions", value: s.total_predictions.toLocaleString(), accent: "var(--accent-green)" },
                    { label: "Total Samples", value: s.total_samples.toLocaleString(), accent: "var(--text)" },
                    { label: "Errors", value: s.total_errors.toLocaleString(), accent: s.total_errors > 0 ? "var(--accent-red)" : "var(--accent-green)" },
                    { label: "Error Rate", value: `${s.error_rate.toFixed(3)}%`, accent: s.error_rate > 0.01 ? "var(--accent-red)" : "var(--accent-green)" },
                    { label: "P99 Latency", value: `${(s.p99_us / 1000).toFixed(2)}ms`, accent: "var(--accent-amber)" },
                  ];
                }
                /* Dataloader mode summary */
                const s = state.summary;
                return [
                  { label: "Samples/sec", value: s.samples_per_sec.toFixed(0), accent: "var(--accent-green)" },
                  { label: "Bandwidth", value: `${s.bandwidth_gb_s.toFixed(2)} GB/s`, accent: "var(--accent-blue)" },
                  { label: "Total Data", value: `${s.total_gb.toFixed(1)} GB`, accent: "var(--text-muted)" },
                  { label: "Total Samples", value: s.total_samples.toLocaleString(), accent: "var(--accent-green)" },
                  { label: "Total Batches", value: s.total_batches.toLocaleString(), accent: "var(--text)" },
                  { label: "Errors", value: s.total_errors.toLocaleString(), accent: s.total_errors > 0 ? "var(--accent-red)" : "var(--accent-green)" },
                  { label: "Error Rate", value: `${s.error_rate.toFixed(3)}%`, accent: s.error_rate > 0.01 ? "var(--accent-red)" : "var(--accent-green)" },
                  { label: "P99 Latency", value: `${(s.p99_us / 1000).toFixed(2)}ms`, accent: "var(--accent-amber)" },
                ];
              })().map(({ label, value, accent }) => (
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
