"use client";

import { useEffect, useRef, useMemo } from "react";
import type { DashboardState } from "@/types/metrics";

// ── Types ──────────────────────────────────────────────────────────────────────

type NodeStatus = "healthy" | "down" | "recovering";

interface ClusterNode {
  id: number;           // 0-indexed
  label: string;
  role: "primary" | "replica";
  status: NodeStatus;
  tpsShare: number;     // 0–100, proportion of cluster TPS
}

// ── SVG layout (viewBox 0 0 100 100) ─────────────────────────────────────────

// Triangle: top-center, bottom-left, bottom-right
const POS = [
  { x: 50, y: 14 },
  { x: 11, y: 84 },
  { x: 89, y: 84 },
] as const;

const EDGES: [number, number][] = [
  [0, 1],
  [0, 2],
  [1, 2],
];

// ── Helpers ───────────────────────────────────────────────────────────────────

function parseNodeId(message: string): number | null {
  const m =
    message.match(/\bnode[- _]?(\d+)\b/i) ||
    message.match(/\breplica[- _]?(\d+)\b/i);
  if (!m) return null;
  const n = parseInt(m[1]) - 1; // convert to 0-indexed
  return n >= 0 && n <= 2 ? n : null;
}

function fmtTps(tps: number): string {
  if (tps >= 1_000_000) return `${(tps / 1_000_000).toFixed(2)}M`;
  if (tps >= 1_000) return `${(tps / 1_000).toFixed(0)}K`;
  return tps > 0 ? tps.toLocaleString() : "—";
}

// ── Component ─────────────────────────────────────────────────────────────────

interface Props {
  state: DashboardState;
  /** Send a WS control command to the bench process. */
  sendCommand: (cmd: Record<string, unknown>) => void;
}

export function FailoverPanel({ state, sendCommand }: Props) {
  const logRef = useRef<HTMLDivElement>(null);

  // Auto-scroll log
  useEffect(() => {
    logRef.current?.scrollTo({ top: logRef.current.scrollHeight, behavior: "smooth" });
  }, [state.events]);

  // ── Derive live cluster nodes from WS events + shard TPS ─────────────────
  const nodes = useMemo<ClusterNode[]>(() => {
    const totalShards = state.config?.shards ?? 0;
    const nodeCount = 3;

    // Authoritative node status from WS events
    const nodeStatus = new Map<number, NodeStatus>();
    for (const ev of state.events) {
      if (ev.kind === "node_down") {
        const id = parseNodeId(ev.message);
        if (id !== null) nodeStatus.set(id, "down");
      } else if (ev.kind === "node_up") {
        const id = parseNodeId(ev.message);
        if (id !== null) nodeStatus.set(id, "healthy");
      } else if (ev.kind === "info" && ev.message.toLowerCase().includes("rejoining")) {
        const id = parseNodeId(ev.message);
        if (id !== null) nodeStatus.set(id, "recovering");
      }
    }

    // Compute TPS per node — distribute shards evenly across 3 nodes
    const nodeTps = [0, 0, 0];
    for (const shard of state.shards.values()) {
      const nodeId =
        totalShards > 0
          ? Math.min(nodeCount - 1, Math.floor((shard.shard_id / totalShards) * nodeCount))
          : shard.shard_id % nodeCount;
      nodeTps[nodeId] += shard.current_tps;
    }
    const maxNodeTps = Math.max(...nodeTps, 1);

    return ([0, 1, 2] as const).map((id) => ({
      id,
      label: `Node ${id + 1}`,
      role: (id === 0 ? "primary" : "replica") as "primary" | "replica",
      status: nodeStatus.get(id) ?? "healthy",
      tpsShare: Math.round((nodeTps[id] / maxNodeTps) * 100),
    }));
  }, [state.events, state.shards, state.config]);

  // ── Consensus badge ────────────────────────────────────────────────────────
  const downCount = nodes.filter((n) => n.status === "down").length;
  const recoveringCount = nodes.filter((n) => n.status === "recovering").length;

  let consensusLabel: string;
  let consensusColor: string;
  if (downCount === 0 && recoveringCount === 0) {
    consensusLabel = "3-of-3 QUORUM";
    consensusColor = "var(--accent-green)";
  } else if (downCount === 0 && recoveringCount > 0) {
    consensusLabel = "REJOINING";
    consensusColor = "var(--accent-amber)";
  } else if (downCount === 1) {
    consensusLabel = "2-of-3 QUORUM";
    consensusColor = "var(--accent-amber)";
  } else {
    consensusLabel = "QUORUM LOST";
    consensusColor = "var(--accent-red)";
  }

  // Bench is connected when we have an active WS session
  const isConnected = state.status === "running" || state.status === "connecting";

  // Send kill / restart commands to the bench process
  const killNode = (nodeId: number) =>
    sendCommand({ cmd: "kill_node", node_id: nodeId + 1 }); // 1-indexed
  const restartNode = (nodeId: number) =>
    sendCommand({ cmd: "restart_node", node_id: nodeId + 1 });

  // Only show failover-relevant events in the log
  const failoverEvents = state.events.filter(
    (e) => e.kind === "node_down" || e.kind === "node_up" || e.kind === "info"
  );

  // ── Render ─────────────────────────────────────────────────────────────────
  return (
    <div className="card p-4">
      {/* ── Header ── */}
      <div className="flex items-center gap-3 mb-4">
        <span
          className="text-xs font-semibold uppercase tracking-widest"
          style={{ color: "var(--text-muted)" }}
        >
          VSR Cluster
        </span>
        <span
          className="text-[10px] font-bold px-2 py-0.5 rounded-full"
          style={{
            background: `color-mix(in srgb, ${consensusColor} 14%, transparent)`,
            color: consensusColor,
            border: `1px solid color-mix(in srgb, ${consensusColor} 28%, transparent)`,
            transition: "all 0.4s",
          }}
        >
          {consensusLabel}
        </span>
        {!isConnected && (
          <span className="text-[10px] ml-auto" style={{ color: "var(--text-dim)" }}>
            Connect to a running{" "}
            <span style={{ color: "var(--accent-amber)" }}>--scenario vsr-failover</span>{" "}
            bench to enable kill controls
          </span>
        )}
      </div>

      {/* ── Body: SVG + cards ── */}
      <div className="flex gap-4 items-start">
        {/* ── SVG cluster diagram ── */}
        <div className="flex-shrink-0">
          <svg
            viewBox="-5 0 110 105"
            width={170}
            height={155}
            style={{ overflow: "visible" }}
          >
            {/* ── Edges ── */}
            {EDGES.map(([a, b]) => {
              const alive = nodes[a].status !== "down" && nodes[b].status !== "down";
              const pa = POS[a];
              const pb = POS[b];
              const pathD = `M ${pa.x} ${pa.y} L ${pb.x} ${pb.y}`;
              const dur1 = `${1.6 + (a + b) * 0.35}s`;
              const dur2 = `${2.1 + (a + b) * 0.25}s`;
              return (
                <g key={`edge-${a}-${b}`}>
                  {/* Base line */}
                  <line
                    x1={pa.x} y1={pa.y}
                    x2={pb.x} y2={pb.y}
                    stroke={alive ? "var(--border-bright)" : "var(--border)"}
                    strokeWidth={alive ? "0.9" : "0.4"}
                    strokeDasharray={alive ? "0" : "2.5 2.5"}
                    style={{ transition: "all 0.6s" }}
                  />
                  {/* Forward pulse dot */}
                  {alive && (
                    <circle r="1.6" fill="var(--accent-green)" opacity="0.85">
                      <animateMotion dur={dur1} repeatCount="indefinite" path={pathD} />
                    </circle>
                  )}
                  {/* Reverse pulse dot */}
                  {alive && (
                    <circle r="1.1" fill="var(--accent-blue)" opacity="0.6">
                      <animateMotion
                        dur={dur2}
                        repeatCount="indefinite"
                        path={`M ${pb.x} ${pb.y} L ${pa.x} ${pa.y}`}
                      />
                    </circle>
                  )}
                </g>
              );
            })}

            {/* ── Nodes ── */}
            {nodes.map((node) => {
              const pos = POS[node.id];
              const isDown = node.status === "down";
              const isRecovering = node.status === "recovering";

              const fill = isDown
                ? "#180404"
                : isRecovering
                ? "#181200"
                : "var(--bg-card)";
              const stroke = isDown
                ? "var(--accent-red)"
                : isRecovering
                ? "var(--accent-amber)"
                : "var(--accent-green)";
              const ringStroke = isDown
                ? "#ef4444"
                : isRecovering
                ? "#f59e0b"
                : "#00e5a0";

              return (
                <g key={node.id}>
                  {/* Expand ring animation */}
                  <circle cx={pos.x} cy={pos.y} r="7.5" fill="none" stroke={ringStroke} strokeWidth="0.6">
                    {isDown ? (
                      <>
                        <animate attributeName="r" from="7.5" to="18" dur="0.9s" repeatCount="indefinite" />
                        <animate attributeName="opacity" from="0.7" to="0" dur="0.9s" repeatCount="indefinite" />
                      </>
                    ) : isRecovering ? (
                      <>
                        <animate attributeName="r" from="7.5" to="13" dur="1.3s" repeatCount="indefinite" />
                        <animate attributeName="opacity" from="0.6" to="0" dur="1.3s" repeatCount="indefinite" />
                      </>
                    ) : (
                      <>
                        <animate attributeName="r" from="7.5" to="11.5" dur="2.4s" repeatCount="indefinite" />
                        <animate attributeName="opacity" from="0.35" to="0" dur="2.4s" repeatCount="indefinite" />
                      </>
                    )}
                  </circle>

                  {/* Node body */}
                  <circle
                    cx={pos.x} cy={pos.y} r="7.5"
                    fill={fill}
                    stroke={stroke}
                    strokeWidth="1"
                    style={{
                      filter: isDown
                        ? "none"
                        : `drop-shadow(0 0 5px ${isRecovering ? "rgba(245,158,11,0.35)" : "rgba(0,229,160,0.25)"})`,
                      transition: "all 0.5s",
                    }}
                  />

                  {/* Status icon inside node */}
                  {isDown ? (
                    <text
                      x={pos.x} y={pos.y + 1.5}
                      textAnchor="middle"
                      dominantBaseline="middle"
                      fontSize="6.5"
                      fill="var(--accent-red)"
                      fontWeight="bold"
                    >
                      ✕
                    </text>
                  ) : isRecovering ? (
                    <text
                      x={pos.x} y={pos.y + 1.5}
                      textAnchor="middle"
                      dominantBaseline="middle"
                      fontSize="6"
                      fill="var(--accent-amber)"
                    >
                      ↻
                    </text>
                  ) : (
                    <text
                      x={pos.x} y={pos.y + 1.5}
                      textAnchor="middle"
                      dominantBaseline="middle"
                      fontSize="5.5"
                      fill="var(--accent-green)"
                    >
                      ✓
                    </text>
                  )}

                  {/* Node label */}
                  <text
                    x={pos.x} y={pos.y + 14}
                    textAnchor="middle"
                    fontSize="4.5"
                    fontFamily="Inter, sans-serif"
                    fontWeight="600"
                    fill={isDown ? "var(--text-dim)" : "var(--text)"}
                  >
                    {node.label}
                  </text>
                  <text
                    x={pos.x} y={pos.y + 19.5}
                    textAnchor="middle"
                    fontSize="3.2"
                    fontFamily="Inter, sans-serif"
                    fill="var(--text-dim)"
                  >
                    {node.role.toUpperCase()}
                  </text>
                </g>
              );
            })}
          </svg>
        </div>

        {/* ── Right: node cards + event log ── */}
        <div className="flex-1 flex flex-col gap-2 min-w-0">
          {/* Node status cards */}
          <div className="grid grid-cols-3 gap-2">
            {nodes.map((node) => {
              const isDown = node.status === "down";
              const isRecov = node.status === "recovering";
              const color = isDown
                ? "var(--accent-red)"
                : isRecov
                ? "var(--accent-amber)"
                : "var(--accent-green)";
              const statusText = isDown ? "DOWN" : isRecov ? "REJOINING" : "HEALTHY";

              // Per-node TPS estimate: when a node is down, surviving nodes
              // show boosted numbers to demonstrate fault tolerance.
              const aliveCount = nodes.filter((n) => n.status !== "down").length || 1;
              const perNodeTps = isDown
                ? 0
                : Math.round(state.current_tps / aliveCount);

              return (
                <div
                  key={node.id}
                  className="rounded-lg p-2.5 flex flex-col gap-1.5"
                  style={{
                    background: `color-mix(in srgb, ${color} 8%, var(--bg-card))`,
                    border: `1px solid color-mix(in srgb, ${color} 25%, transparent)`,
                    transition: "all 0.45s",
                  }}
                >
                  <div className="flex items-center justify-between gap-1">
                    <span
                      className="text-[11px] font-bold"
                      style={{ color: "var(--text)" }}
                    >
                      {node.label}
                    </span>
                    <span
                      className="text-[9px] font-bold"
                      style={{ color }}
                    >
                      {statusText}
                    </span>
                  </div>
                  <div className="text-[9px]" style={{ color: "var(--text-dim)" }}>
                    {node.role}
                  </div>

                  {/* TPS bar */}
                  <div>
                    <div
                      className="h-1 rounded-full w-full"
                      style={{ background: "var(--border)" }}
                    >
                      <div
                        className="h-1 rounded-full"
                        style={{
                          width: isDown ? "0%" : `${node.tpsShare || 33}%`,
                          background: color,
                          transition: "width 1.2s ease",
                        }}
                      />
                    </div>
                    <div
                      className="text-[9px] font-mono mt-0.5"
                      style={{ color: isDown ? "var(--text-dim)" : "var(--text-muted)" }}
                    >
                      {isDown ? "—" : fmtTps(perNodeTps)}
                    </div>
                  </div>

                  {/* Kill / Restart buttons — only when bench is connected */}
                  {isConnected && (
                    <div className="flex gap-1 mt-0.5">
                      {!isDown && (
                        <button
                          onClick={() => killNode(node.id)}
                          title={`Send kill_node command to bench — runs your configured --kill-cmd-${node.id + 1}`}
                          className="flex-1 text-[9px] font-bold py-0.5 rounded"
                          style={{
                            background: "color-mix(in srgb, var(--accent-red) 12%, transparent)",
                            color: "var(--accent-red)",
                            border: "1px solid color-mix(in srgb, var(--accent-red) 22%, transparent)",
                            cursor: "pointer",
                          }}
                        >
                          KILL
                        </button>
                      )}
                      {isDown && (
                        <button
                          onClick={() => restartNode(node.id)}
                          title={`Send restart_node command to bench — runs your configured --restart-cmd-${node.id + 1}`}
                          className="flex-1 text-[9px] font-bold py-0.5 rounded"
                          style={{
                            background: "color-mix(in srgb, var(--accent-green) 12%, transparent)",
                            color: "var(--accent-green)",
                            border: "1px solid color-mix(in srgb, var(--accent-green) 22%, transparent)",
                            cursor: "pointer",
                          }}
                        >
                          RESTART
                        </button>
                      )}
                    </div>
                  )}
                </div>
              );
            })}
          </div>

          {/* Failover event log */}
          <div
            ref={logRef}
            className="rounded-lg p-2.5 font-mono text-[10px] overflow-y-auto"
            style={{
              background: "var(--bg)",
              border: "1px solid var(--border)",
              minHeight: 64,
              maxHeight: 110,
            }}
          >
            {failoverEvents.length === 0 ? (
              <span style={{ color: "var(--text-dim)" }}>
                No failover events yet.{" "}
                {isConnected ? (
                  <>Press <span style={{ color: "var(--accent-red)" }}>KILL</span> on a node to trigger a real VSR failover.</>
                ) : (
                  <>Connect to a running <span style={{ color: "var(--accent-amber)" }}>--scenario vsr-failover</span> bench.</>
                )}
              </span>
            ) : (
              failoverEvents.slice(-40).map((e, i) => (
                <div key={i} className="flex gap-2 py-px">
                  <span style={{ color: "var(--text-dim)" }}>t+{e.t}s</span>
                  <span
                    style={{
                      color:
                        e.kind === "node_down"
                          ? "var(--accent-red)"
                          : e.kind === "node_up"
                          ? "var(--accent-green)"
                          : "var(--text-muted)",
                    }}
                  >
                    {e.message}
                  </span>
                </div>
              ))
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
