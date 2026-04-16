"use client";

import Image from "next/image";
import { clsx } from "clsx";
import type { BenchStatus, ConfigMessage } from "@/types/metrics";

interface Props {
  status: BenchStatus;
  config: ConfigMessage | null;
  elapsedSecs: number;
  wsUrl: string;
  onWsUrlChange: (u: string) => void;
  onConnect: () => void;
  onDisconnect: () => void;
}

const STATUS_LABEL: Record<BenchStatus, string> = {
  idle: '"Why big iron when you can Blazil-Beetle stack?"',
  connecting: "CONNECTING",
  running: "RUNNING",
  completed: "COMPLETED",
  error: "ERROR",
};

const STATUS_COLOR: Record<BenchStatus, string> = {
  idle: "text-[var(--text-muted)]",
  connecting: "text-[var(--accent-amber)]",
  running: "text-[var(--accent-green)]",
  completed: "text-[var(--accent-blue)]",
  error: "text-[var(--accent-red)]",
};

function fmtTime(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
}

export function Header({
  status,
  config,
  elapsedSecs,
  wsUrl,
  onWsUrlChange,
  onConnect,
  onDisconnect,
}: Props) {
  const isRunning = status === "running" || status === "connecting";

  return (
    <header
      className="sticky top-0 z-50 flex items-center justify-between gap-4 px-6 py-3"
      style={{
        background: "rgba(8,11,18,0.92)",
        backdropFilter: "blur(16px)",
        borderBottom: "1px solid var(--border)",
      }}
    >
      {/* Left: Brand */}
      <div className="flex items-center gap-3 min-w-0">
        <div
          className="flex items-center justify-center w-8 h-8 rounded-lg overflow-hidden"
          style={{ background: "#000", border: "1px solid var(--border-bright)" }}
        >
          <Image
            src="/blazil-icon.jpg"
            alt="Blazil"
            width={32}
            height={32}
            className="object-cover"
            priority
          />
        </div>
        <div>
          <div className="font-bold text-sm tracking-wide text-white">
            BLAZIL
          </div>
          <div className="text-[10px] text-[var(--text-muted)] -mt-0.5">
            BENCH DASHBOARD
          </div>
        </div>

        {config && (
          <div
            className="hidden md:flex items-center gap-2 ml-4 px-3 py-1 rounded-full text-xs"
            style={{
              background: "rgba(0,229,160,0.08)",
              border: "1px solid rgba(0,229,160,0.2)",
              color: "var(--accent-green)",
            }}
          >
            <span>{config.shards} shards</span>
            <span style={{ color: "var(--border-bright)" }}>·</span>
            <span>
              {config.duration_secs ? `${config.duration_secs}s` : "event mode"}
            </span>
            <span style={{ color: "var(--border-bright)" }}>·</span>
            <span className="truncate max-w-[160px]" title={config.tb_addr}>
              {config.tb_addr.split(",")[0]}
              {config.tb_addr.includes(",") ? "…" : ""}
            </span>
          </div>
        )}
      </div>

      {/* Center: Status + Timer */}
      <div className="flex items-center gap-3">
        <div className="flex items-center gap-2">
          {status === "running" && (
            <div className="relative flex items-center justify-center w-3 h-3">
              <div
                className="absolute w-3 h-3 rounded-full pulse-ring"
                style={{ background: "var(--accent-green)", opacity: 0.4 }}
              />
              <div
                className="w-2 h-2 rounded-full"
                style={{ background: "var(--accent-green)" }}
              />
            </div>
          )}
          <span className={clsx("text-xs font-semibold tracking-widest", STATUS_COLOR[status])}>
            {STATUS_LABEL[status]}
          </span>
        </div>
        {elapsedSecs > 0 && (
          <div
            className="font-mono text-sm font-bold"
            style={{ color: "var(--accent-green)" }}
          >
            {fmtTime(elapsedSecs)}
          </div>
        )}
      </div>

      {/* Right: Connection */}
      <div className="flex items-center gap-2">
        <input
          type="text"
          value={wsUrl}
          onChange={(e) => onWsUrlChange(e.target.value)}
          disabled={isRunning}
          className="hidden md:block text-xs font-mono rounded-lg px-3 py-1.5 w-56 outline-none"
          style={{
            background: "var(--bg-card)",
            border: "1px solid var(--border)",
            color: "var(--text)",
            opacity: isRunning ? 0.6 : 1,
          }}
          placeholder="ws://host:9090/ws"
        />
        {isRunning ? (
          <button
            onClick={onDisconnect}
            className="text-xs font-semibold px-4 py-1.5 rounded-lg transition-all"
            style={{
              background: "rgba(239,68,68,0.15)",
              border: "1px solid rgba(239,68,68,0.3)",
              color: "var(--accent-red)",
            }}
          >
            Disconnect
          </button>
        ) : (
          <button
            onClick={onConnect}
            className="text-xs font-semibold px-4 py-1.5 rounded-lg transition-all"
            style={{
              background: "rgba(0,229,160,0.12)",
              border: "1px solid rgba(0,229,160,0.3)",
              color: "var(--accent-green)",
            }}
          >
            Connect
          </button>
        )}
      </div>
    </header>
  );
}
