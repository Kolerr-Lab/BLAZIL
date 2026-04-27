"use client";

import { useEffect, useRef, useCallback, useState } from "react";
import type {
  BenchMessage,
  DashboardState,
  SecondSnapshot,
  ShardState,
} from "@/types/metrics";

const MAX_HISTORY = 600; // 10 minutes at 1s resolution
const SPARKLINE_LEN = 30;

function initialState(): DashboardState {
  return {
    mode: "fintech",
    status: "idle",
    config: null,
    elapsed_secs: 0,
    history: [],
    shards: new Map(),
    events: [],
    summary: null,
    current_tps: 0,
    peak_tps: 0,
    total_committed: 0,
    total_rejected: 0,
    total_samples: 0,
    total_predictions: 0,
    total_errors: 0,
    current_p50_us: 0,
    current_p99_us: 0,
  };
}

export function useBenchWS(wsUrl: string) {
  const [state, setState] = useState<DashboardState>(initialState);
  const wsRef = useRef<WebSocket | null>(null);
  // Track summary received synchronously so onclose never races with setState.
  const summaryReceivedRef = useRef(false);
  // Pending ticks buffer: accumulate shard ticks for current second, then
  // flush to history once we detect the second has rolled over or all shards
  // have reported. We flush on a 1.1s timer to handle any straggling shards.
  const pendingTicksRef = useRef<Map<number, Map<number, { tps: number; committed: number; rejected: number; p50_us: number; p99_us: number; inflight: number; sent: number }>>>(new Map());
  const flushTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const flushSecond = useCallback((t: number) => {
    const shardMap = pendingTicksRef.current.get(t);
    if (!shardMap) return;
    pendingTicksRef.current.delete(t);

    setState((prev) => {
      const aggTps = [...shardMap.values()].reduce((s, v) => s + v.tps, 0);
      const aggCommitted = [...shardMap.values()].reduce((s, v) => s + v.committed, 0);
      const aggRejected = [...shardMap.values()].reduce((s, v) => s + v.rejected, 0);
      const avgP50 = [...shardMap.values()].reduce((s, v) => s + v.p50_us, 0) / (shardMap.size || 1);
      const avgP99 = [...shardMap.values()].reduce((s, v) => s + v.p99_us, 0) / (shardMap.size || 1);
      const errorRate = aggCommitted + aggRejected > 0
        ? (aggRejected / (aggCommitted + aggRejected)) * 100
        : 0;

      const snapshot: SecondSnapshot = {
        t,
        tps: aggTps,
        per_shard: [...shardMap.entries()].map(([id, d]) => ({ shard_id: id, tps: d.tps })),
        error_rate: errorRate,
        p50_us: Math.round(avgP50),
        p99_us: Math.round(avgP99),
        total_committed: aggCommitted,
        total_rejected: aggRejected,
      };

      const newHistory = [...prev.history, snapshot].slice(-MAX_HISTORY);
      const newPeak = Math.max(prev.peak_tps, aggTps);

      // Update per-shard state.
      const newShards = new Map(prev.shards);
      for (const [sid, d] of shardMap.entries()) {
        const existing = newShards.get(sid);
        const history = existing
          ? [...existing.tps_history, d.tps].slice(-SPARKLINE_LEN)
          : [d.tps];
        newShards.set(sid, {
          shard_id: sid,
          current_tps: d.tps,
          committed_total: d.committed,
          rejected_total: d.rejected,
          inflight: d.inflight,
          sent_total: d.sent,
          p50_us: d.p50_us,
          p99_us: d.p99_us,
          last_tick_t: t,
          tps_history: history,
        });
      }

      return {
        ...prev,
        // Auto-transition to "running" if config was missed (connected after bench start).
        // Safe to set even if already "running" — no-op.
        status: prev.status === "connecting" || prev.status === "idle" ? "running" : prev.status,
        history: newHistory,
        shards: newShards,
        current_tps: aggTps,
        peak_tps: newPeak,
        total_committed: [...newShards.values()].reduce((s, v) => s + v.committed_total, 0),
        total_rejected: [...newShards.values()].reduce((s, v) => s + v.rejected_total, 0),
        current_p50_us: snapshot.p50_us,
        current_p99_us: snapshot.p99_us,
        elapsed_secs: t,
      };
    });
  }, []);

  const handleMessage = useCallback(
    (raw: string) => {
      let msg: BenchMessage;
      try {
        msg = JSON.parse(raw) as BenchMessage;
      } catch {
        return;
      }

      if (msg.type === "config") {
        summaryReceivedRef.current = false;
        const mode: DashboardState["mode"] =
          "mode" in msg ? (msg.mode as "dataloader" | "inference") : "fintech";
        setState({ ...initialState(), status: "connecting", config: msg, mode });
        return;
      }

      if (msg.type === "event") {
        setState((p) => ({
          ...p,
          events: [...p.events.slice(-99), msg as typeof msg & { type: "event" }],
        }));
        return;
      }

      if (msg.type === "summary") {
        summaryReceivedRef.current = true;
        setState((p) => ({
          ...p,
          status: "completed",
          summary: msg as typeof msg & { type: "summary" },
        }));
        return;
      }

      if (msg.type === "tick") {
        const tick = msg as typeof msg & { type: "tick" };
        const t = tick.t;
        
        // Handle fintech tick messages (with shard_id)
        if ('shard_id' in tick) {
          if (!pendingTicksRef.current.has(t)) {
            pendingTicksRef.current.set(t, new Map());
          }
          pendingTicksRef.current.get(t)!.set(tick.shard_id, {
            tps: tick.tps,
            committed: tick.committed_total,
            rejected: tick.rejected_total,
            p50_us: tick.p50_us,
            p99_us: tick.p99_us,
            inflight: tick.inflight,
            sent: tick.sent_total,
          });

          // Flush previous second immediately when a new second arrives.
          if (t > 0 && pendingTicksRef.current.has(t - 1)) {
            flushSecond(t - 1);
          }

          // Safety flush: emit current second after 1.2s to handle straggling shards.
          if (flushTimerRef.current) clearTimeout(flushTimerRef.current);
          flushTimerRef.current = setTimeout(() => flushSecond(t), 1200);
        } else {
          // Handle AI tick messages (dataloader or inference) - no p50/p99 in tick messages
          // Use type narrowing with mode field
          if ('mode' in tick && tick.mode === 'dataloader') {
            setState((prev) => {
              const tps = tick.samples_per_sec;
              const snapshot: SecondSnapshot = {
                t,
                tps,
                per_shard: [],
                error_rate: tick.total_errors / Math.max(tick.total_samples, 1) * 100,
                p50_us: 0,
                p99_us: 0,
                total_committed: tick.total_samples,
                total_rejected: tick.total_errors,
              };
              return {
                ...prev,
                elapsed_secs: t,
                status: "running",
                current_tps: tps,
                peak_tps: Math.max(prev.peak_tps, tps),
                total_samples: tick.total_samples,
                total_errors: tick.total_errors,
                history: [...prev.history, snapshot].slice(-MAX_HISTORY),
              };
            });
          } else if ('mode' in tick && tick.mode === 'inference') {
            setState((prev) => {
              const tps = tick.rps;
              const snapshot: SecondSnapshot = {
                t,
                tps,
                per_shard: [],
                error_rate: tick.total_errors / Math.max(tick.total_samples, 1) * 100,
                p50_us: 0,
                p99_us: 0,
                total_committed: tick.total_samples,
                total_rejected: tick.total_errors,
              };
              return {
                ...prev,
                elapsed_secs: t,
                status: "running",
                current_tps: tps,
                peak_tps: Math.max(prev.peak_tps, tps),
                total_samples: tick.total_samples,
                total_predictions: tick.total_predictions,
                total_errors: tick.total_errors,
                history: [...prev.history, snapshot].slice(-MAX_HISTORY),
              };
            });
          }
        }
      }
    },
    [flushSecond]
  );

  const connect = useCallback(() => {
    if (wsRef.current) {
      // Null out handlers BEFORE closing so the old onclose/onerror cannot
      // race with the new connection and overwrite "connecting" → "error".
      const old = wsRef.current;
      old.onopen = null;
      old.onmessage = null;
      old.onerror = null;
      old.onclose = null;
      old.close();
    }
    summaryReceivedRef.current = false;
    setState((p) => ({ ...p, status: "connecting" }));
    const ws = new WebSocket(wsUrl);
    wsRef.current = ws;

    ws.onopen = () => setState((p) => ({ ...p, status: "connecting" }));
    ws.onmessage = (e) => handleMessage(e.data as string);
    ws.onerror = () => {
      if (!summaryReceivedRef.current) {
        setState((p) => ({ ...p, status: "error" }));
      }
    };
    ws.onclose = () => {
      if (!summaryReceivedRef.current) {
        // covers "connecting" (bench not ready yet) AND "running" (bench crashed)
        setState((p) =>
          p.status === "completed" ? p : { ...p, status: "error" }
        );
      }
    };
  }, [wsUrl, handleMessage]);

  const disconnect = useCallback(() => {
    if (wsRef.current) {
      const old = wsRef.current;
      old.onopen = null;
      old.onmessage = null;
      old.onerror = null;
      old.onclose = null;
      old.close();
      wsRef.current = null;
    }
    setState((p) => ({ ...p, status: "idle" }));
  }, []);

  /** Send a control command to the bench process via the WS connection.
   *  The bench WS server forwards it to all scenario subscribers. */
  const sendCommand = useCallback((cmd: Record<string, unknown>) => {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify(cmd));
    }
  }, []);

  useEffect(() => {
    return () => {
      wsRef.current?.close();
      if (flushTimerRef.current) clearTimeout(flushTimerRef.current);
    };
  }, []);

  return { state, connect, disconnect, sendCommand };
}
