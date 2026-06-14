"use client";

import { useState, useEffect, useCallback } from "react";
import { useBenchWS } from "@/hooks/useBenchWS";
import { Header } from "@/components/Header";
import { HeroMetrics } from "@/components/HeroMetrics";
import { TPSChart } from "@/components/TPSChart";
import { LatencyPanel } from "@/components/LatencyPanel";
import { ClusterInfo } from "@/components/ClusterInfo";
import { ChatPane } from "@/components/ChatPane";
import type { EventMessage } from "@/types/metrics";

const DEFAULT_WS_URL = "ws://localhost:9092/ws";

function formatElapsed(secs: number): string {
  const minutes = Math.floor(secs / 60);
  const seconds = secs % 60;
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
}

export default function DashboardPage() {
  const [wsUrl, setWsUrl] = useState(DEFAULT_WS_URL);
  const { state, connect, disconnect, sendCommand } = useBenchWS(wsUrl);

  // Auto-scroll event log to bottom.
  const eventsEndRef = useCallback((el: HTMLDivElement | null) => {
    el?.scrollIntoView({ behavior: "smooth" });
  }, []);

  // Manual connect only - no auto-retry to avoid connection spam

  // Browser title updates with live throughput.
  useEffect(() => {
    if (state.status === "running" && state.current_tps > 0) {
      const throughput =
        state.current_tps >= 1_000_000
          ? `${(state.current_tps / 1_000_000).toFixed(2)}M`
          : `${(state.current_tps / 1_000).toFixed(0)}K`;
      const label = state.mode === 'inference' ? 'RPS' : 'Samples/s';
      document.title = `${throughput} ${label} | Clarken Console`;
    } else {
      document.title = "Clarken Console | Blazil";
    }
  }, [state.current_tps, state.status, state.mode]);

  const boardMode = state.mode === "inference" ? "Inference board" : "Benchmark board";
  const connectionLabel =
    state.status === "running"
      ? "Streaming live"
      : state.status === "connecting"
      ? "Connecting"
      : state.status === "completed"
      ? "Run complete"
      : state.status === "error"
      ? "Attention required"
      : "Standby";

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
        <section className="grid grid-cols-1 xl:grid-cols-5 gap-4 items-start">
          <div
            className="card xl:col-span-3 p-6 flex flex-col gap-4"
            style={{
              background:
                "radial-gradient(circle at top left, rgba(0,255,178,0.12), transparent 36%), radial-gradient(circle at bottom right, rgba(96,165,250,0.12), transparent 34%), var(--bg-card)",
            }}
          >
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <div className="text-[10px] font-semibold uppercase tracking-[0.24em]" style={{ color: "var(--text-muted)" }}>
                  Clarken Console
                </div>
                <h1 className="mt-2 text-3xl md:text-4xl font-black tracking-tight text-white">
                  Live chat workspace
                </h1>
              </div>
              <div
                className="px-3 py-1.5 rounded-full text-xs font-semibold"
                style={{
                  background: "rgba(255,255,255,0.04)",
                  border: "1px solid var(--border)",
                  color: "var(--text-muted)",
                }}
              >
                {connectionLabel}
              </div>
            </div>
            <div className="grid grid-cols-1 md:grid-cols-3 gap-3 text-sm">
              <div className="card p-4">
                <div className="text-[10px] font-semibold uppercase tracking-widest" style={{ color: "var(--text-muted)" }}>
                  Surface
                </div>
                <div className="mt-1 font-bold text-lg" style={{ color: "var(--accent-green)" }}>
                  Clarken Live Console
                </div>
              </div>
              <div className="card p-4">
                <div className="text-[10px] font-semibold uppercase tracking-widest" style={{ color: "var(--text-muted)" }}>
                  Status
                </div>
                <div className="mt-1 font-bold text-lg" style={{ color: state.status === "error" ? "var(--accent-red)" : "var(--accent-blue)" }}>
                  {connectionLabel}
                </div>
                <div className="mt-1 text-xs leading-5" style={{ color: "var(--text-muted)" }}>
                  {state.elapsed_secs > 0 ? formatElapsed(state.elapsed_secs) : "00:00"}
                </div>
              </div>
              <div className="card p-4">
                <div className="text-[10px] font-semibold uppercase tracking-widest" style={{ color: "var(--text-muted)" }}>
                  Board
                </div>
                <div className="mt-1 font-bold text-lg" style={{ color: "var(--accent-purple)" }}>
                  {boardMode}
                </div>
              </div>
            </div>
          </div>

          <div className="xl:col-span-2">
            <ChatPane />
          </div>
        </section>

        <section className="mt-8 flex flex-col gap-4">
          <div>
            <div className="text-[10px] font-semibold uppercase tracking-[0.24em]" style={{ color: "var(--text-muted)" }}>
              Legacy Benchmark Board
            </div>
            <h2 className="mt-1 text-2xl font-black tracking-tight text-white">
              Performance workspace
            </h2>
          </div>

          <HeroMetrics state={state} />

          <ClusterInfo />

          <div className="grid grid-cols-1 xl:grid-cols-2 gap-4 items-start">
            <TPSChart
              history={state.history}
              duration_secs={state.config?.duration_secs ?? null}
            />

            <LatencyPanel state={state} />
          </div>

          <div className="grid grid-cols-1 xl:grid-cols-2 gap-4 items-start">
            <div>
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

            {state.summary && (
              <div>
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
          </div>
        </section>
      </main>
    </div>
  );
}
