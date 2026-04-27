"use client";

import { useEffect, useRef, useCallback, useState } from "react";
import type {
  BenchMessage,
  DashboardState,
  SecondSnapshot,
  ShardState,
} from "@/types/metrics";

const MAX_HISTORY = 600; // 10 minutes at 1s resolution
const INITIAL_RETRY_DELAY_MS = 5_000; // Start with 5s
const MAX_RETRY_DELAY_MS = 60_000;     // Cap at 60s

function initialState(): DashboardState {
  return {
    mode: "dataloader",  // AI-only: dataloader or inference
    status: "idle",
    config: null,
    elapsed_secs: 0,
    history: [],
    shards: new Map(),  // Unused in AI mode, kept for type compatibility
    events: [],
    summary: null,
    current_tps: 0,     // samples/sec for dataloader, RPS for inference
    peak_tps: 0,
    total_committed: 0, // Unused in AI
    total_rejected: 0,  // Unused in AI
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
  // Exponential backoff: increase delay on each failure, reset on success
  const retryDelayRef = useRef(INITIAL_RETRY_DELAY_MS);

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
        // Detect mode from config: 'mode' field for AI, 'shards' for fintech (reject fintech)
        const mode = 'mode' in msg && msg.mode === 'dataloader' ? 'dataloader' :
                     'mode' in msg && msg.mode === 'inference' ? 'inference' :
                     'dataloader'; // default to dataloader for AI
        setState(() => ({
          ...initialState(),
          mode,
          status: "running",
          config: msg as typeof msg & { type: "config" },
        }));
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
        
        // AI dashboard only handles AI tick messages (dataloader or inference)
        // Reject fintech ticks (which have 'shard_id' instead of 'mode')
        if ('mode' in tick && tick.mode === 'dataloader') {
          const tps = tick.samples_per_sec;
          const snap: SecondSnapshot = {
            t,
            tps,
            per_shard: [],
            error_rate: tick.total_samples > 0 ? tick.total_errors / tick.total_samples * 100 : 0,
            p50_us: 0,
            p99_us: 0,
            total_committed: tick.total_samples,
            total_rejected: tick.total_errors,
          };
          setState((prev) => ({
            ...prev,
            elapsed_secs: t,
            status: "running",
            current_tps: tps,
            peak_tps: Math.max(prev.peak_tps, tps),
            total_samples: tick.total_samples,
            total_errors: tick.total_errors,
            history: [...prev.history.slice(-(MAX_HISTORY - 1)), snap],
          }));
        } else if ('mode' in tick && tick.mode === 'inference') {
          const tps = tick.rps;
          const snap: SecondSnapshot = {
            t,
            tps,
            per_shard: [],
            error_rate: tick.total_samples > 0 ? tick.total_errors / tick.total_samples * 100 : 0,
            p50_us: 0,
            p99_us: 0,
            total_committed: tick.total_predictions,
            total_rejected: tick.total_errors,
          };
          setState((prev) => ({
            ...prev,
            elapsed_secs: t,
            status: "running",
            current_tps: tps,
            peak_tps: Math.max(prev.peak_tps, tps),
            total_samples: tick.total_samples,
            total_predictions: tick.total_predictions,
            total_errors: tick.total_errors,
            history: [...prev.history.slice(-(MAX_HISTORY - 1)), snap],
          }));
        }
        // Silently ignore fintech ticks (shard_id-based)
      }
    },
    []
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

    ws.onopen = () => {
      // Reset backoff on successful connection
      retryDelayRef.current = INITIAL_RETRY_DELAY_MS;
      setState((p) => ({ ...p, status: "connecting" }));
    };
    ws.onmessage = (e) => handleMessage(e.data as string);
    ws.onerror = () => {
      if (!summaryReceivedRef.current) {
        // Exponential backoff: double the delay, cap at max
        retryDelayRef.current = Math.min(retryDelayRef.current * 2, MAX_RETRY_DELAY_MS);
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
    };
  }, []);

  return { state, connect, disconnect, sendCommand };
}
