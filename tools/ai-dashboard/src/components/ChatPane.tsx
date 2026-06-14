"use client";

import { useEffect, useMemo, useRef, useState } from "react";

type ChatRole = "assistant" | "user" | "error";

interface ChatMessage {
  id: string;
  role: ChatRole;
  text: string;
  latencyUs?: number;
  firstTokenLatencyUs?: number;
  tokensGenerated?: number;
}

interface ChatSettings {
  serverUrl: string;
  apiKey: string;
  tenantId: string;
  maxTokens: number;
}

interface ChatResponse {
  output_text: string;
  latency_us: number;
  first_token_latency_us: number;
  tokens_generated: number;
  error?: string;
}

const STORAGE_KEY = "blazil-ai-chat-settings";
const DEFAULT_SETTINGS: ChatSettings = {
  serverUrl: "http://localhost:8092",
  apiKey: "devkey",
  tenantId: "dashboard",
  maxTokens: 96,
};

function fmtLatency(us?: number): string {
  if (!us) return "-";
  if (us >= 1_000_000) return `${(us / 1_000_000).toFixed(2)}s`;
  if (us >= 1_000) return `${(us / 1_000).toFixed(1)}ms`;
  return `${us}us`;
}

export function ChatPane() {
  const [settings, setSettings] = useState<ChatSettings>(DEFAULT_SETTINGS);
  const [prompt, setPrompt] = useState("");
  const [showOperatorSettings, setShowOperatorSettings] = useState(false);
  const [messages, setMessages] = useState<ChatMessage[]>([
    {
      id: "intro",
      role: "assistant",
      text: "Clarken live console is ready. Submit a prompt here to exercise the active Clarken runtime and compare the answer against the live operating metrics beside it.",
    },
  ]);
  const [isSending, setIsSending] = useState(false);
  const [healthStatus, setHealthStatus] = useState<"idle" | "checking" | "healthy" | "error">("idle");
  const [healthMessage, setHealthMessage] = useState("Not checked");
  const listRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return;
    try {
      const parsed = JSON.parse(raw) as Partial<ChatSettings>;
      setSettings((current) => ({
        ...current,
        ...parsed,
        maxTokens: typeof parsed.maxTokens === "number" ? parsed.maxTokens : current.maxTokens,
      }));
    } catch {
      // Ignore corrupted local storage and keep defaults.
    }
  }, []);

  useEffect(() => {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
  }, [settings]);

  useEffect(() => {
    listRef.current?.scrollTo({ top: listRef.current.scrollHeight, behavior: "smooth" });
  }, [messages, isSending]);

  const latestStats = useMemo(() => {
    const latest = [...messages].reverse().find((message) => message.role === "assistant" && message.latencyUs);
    if (!latest) return null;
    return latest;
  }, [messages]);

  async function pingBackend() {
    setHealthStatus("checking");
    setHealthMessage("Checking /health...");
    try {
      const response = await fetch(`/api/chat?serverUrl=${encodeURIComponent(settings.serverUrl)}`);
      const payload = (await response.json()) as { ok?: boolean; status?: number; error?: string };
      if (!response.ok || !payload.ok) {
        setHealthStatus("error");
        setHealthMessage(payload.error ?? `HTTP ${payload.status ?? response.status}`);
        return;
      }
      setHealthStatus("healthy");
      setHealthMessage(`Healthy (${payload.status})`);
    } catch (error) {
      setHealthStatus("error");
      setHealthMessage(error instanceof Error ? error.message : "Health check failed");
    }
  }

  async function sendPrompt() {
    if (isSending || !prompt.trim()) return;

    const userMessage: ChatMessage = {
      id: `user-${Date.now()}`,
      role: "user",
      text: prompt.trim(),
    };

    setMessages((current) => [...current, userMessage]);
    setPrompt("");
    setIsSending(true);

    try {
      const response = await fetch("/api/chat", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          serverUrl: settings.serverUrl,
          apiKey: settings.apiKey,
          tenantId: settings.tenantId,
          prompt: userMessage.text,
          maxTokens: settings.maxTokens,
        }),
      });

      const payload = (await response.json()) as ChatResponse | { error?: string };
      if (!response.ok || !("output_text" in payload)) {
        setMessages((current) => [
          ...current,
          {
            id: `error-${Date.now()}`,
            role: "error",
            text: payload.error ?? `Chat request failed with status ${response.status}`,
          },
        ]);
        return;
      }

      setMessages((current) => [
        ...current,
        {
          id: `assistant-${Date.now()}`,
          role: "assistant",
          text: payload.output_text,
          latencyUs: payload.latency_us,
          firstTokenLatencyUs: payload.first_token_latency_us,
          tokensGenerated: payload.tokens_generated,
        },
      ]);
    } catch (error) {
      setMessages((current) => [
        ...current,
        {
          id: `error-${Date.now()}`,
          role: "error",
          text: error instanceof Error ? error.message : "Chat request failed",
        },
      ]);
    } finally {
      setIsSending(false);
    }
  }

  function handlePromptKeyDown(event: React.KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) {
      return;
    }

    event.preventDefault();
    void sendPrompt();
  }

  return (
    <div className="card p-5 flex h-full flex-col gap-4">
      <div className="flex items-start justify-between gap-4">
        <div>
          <div className="font-semibold text-sm text-white">Clarken Live Console</div>
          <div className="text-xs mt-0.5" style={{ color: "var(--text-muted)" }}>
            Ask, inspect, and compare live Clarken responses in one focused workspace.
          </div>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={() => setShowOperatorSettings((current) => !current)}
            className="text-xs font-semibold px-3 py-1.5 rounded-lg transition-all"
            style={{
              background: "rgba(255,255,255,0.05)",
              border: "1px solid var(--border)",
              color: "var(--text-muted)",
            }}
          >
            {showOperatorSettings ? "Hide operator settings" : "Operator settings"}
          </button>
          <button
            onClick={pingBackend}
            disabled={healthStatus === "checking"}
            className="text-xs font-semibold px-3 py-1.5 rounded-lg transition-all"
            style={{
              background: "rgba(96,165,250,0.12)",
              border: "1px solid rgba(96,165,250,0.3)",
              color: "var(--accent-blue)",
            }}
          >
            {healthStatus === "checking" ? "Checking..." : "Check service"}
          </button>
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-3 text-xs">
        <div
          className="px-3 py-1.5 rounded-full"
          style={{
            background:
              healthStatus === "healthy"
                ? "rgba(0,229,160,0.10)"
                : healthStatus === "error"
                ? "rgba(248,113,113,0.12)"
                : "rgba(255,255,255,0.04)",
            color:
              healthStatus === "healthy"
                ? "var(--accent-green)"
                : healthStatus === "error"
                ? "var(--accent-red)"
                : "var(--text-muted)",
            border: "1px solid var(--border)",
          }}
        >
          {healthMessage}
        </div>
        <div style={{ color: "var(--text-muted)" }}>Ready for live prompts</div>
      </div>

      {showOperatorSettings && (
        <div className="card p-4 grid grid-cols-1 gap-3 md:grid-cols-2 text-xs">
          <label className="flex flex-col gap-1">
            <span style={{ color: "var(--text-muted)" }}>Service URL</span>
            <input
              value={settings.serverUrl}
              onChange={(event) => setSettings((current) => ({ ...current, serverUrl: event.target.value }))}
              className="rounded-lg px-3 py-2 outline-none"
              style={{ background: "var(--bg)", border: "1px solid var(--border)", color: "var(--text)" }}
              placeholder="http://host:8092"
            />
          </label>
          <label className="flex flex-col gap-1">
            <span style={{ color: "var(--text-muted)" }}>Workspace ID</span>
            <input
              value={settings.tenantId}
              onChange={(event) => setSettings((current) => ({ ...current, tenantId: event.target.value }))}
              className="rounded-lg px-3 py-2 outline-none"
              style={{ background: "var(--bg)", border: "1px solid var(--border)", color: "var(--text)" }}
              placeholder="dashboard"
            />
          </label>
          <label className="flex flex-col gap-1 md:col-span-2">
            <span style={{ color: "var(--text-muted)" }}>Access token</span>
            <input
              type="password"
              value={settings.apiKey}
              onChange={(event) => setSettings((current) => ({ ...current, apiKey: event.target.value }))}
              className="rounded-lg px-3 py-2 outline-none"
              style={{ background: "var(--bg)", border: "1px solid var(--border)", color: "var(--text)" }}
              placeholder="devkey"
            />
          </label>
          <label className="flex flex-col gap-1 md:max-w-[180px]">
            <span style={{ color: "var(--text-muted)" }}>Response length</span>
            <input
              type="number"
              min={1}
              max={2048}
              value={settings.maxTokens}
              onChange={(event) =>
                setSettings((current) => ({
                  ...current,
                  maxTokens: Math.max(1, Number(event.target.value) || 1),
                }))
              }
              className="rounded-lg px-3 py-2 outline-none"
              style={{ background: "var(--bg)", border: "1px solid var(--border)", color: "var(--text)" }}
            />
          </label>
        </div>
      )}

      <div
        ref={listRef}
        className="flex min-h-[360px] flex-1 flex-col gap-3 overflow-y-auto rounded-xl p-3"
        style={{ background: "rgba(255,255,255,0.02)", border: "1px solid var(--border)" }}
      >
        {messages.map((message) => (
          <div
            key={message.id}
            className={`max-w-[92%] rounded-2xl px-4 py-3 ${message.role === "user" ? "self-end" : "self-start"}`}
            style={{
              background:
                message.role === "user"
                  ? "linear-gradient(135deg, rgba(0,229,160,0.16), rgba(96,165,250,0.16))"
                  : message.role === "error"
                  ? "rgba(248,113,113,0.10)"
                  : "rgba(22,29,43,0.92)",
              border:
                message.role === "user"
                  ? "1px solid rgba(0,229,160,0.16)"
                  : message.role === "error"
                  ? "1px solid rgba(248,113,113,0.25)"
                  : "1px solid var(--border)",
            }}
          >
            <div className="mb-1 text-[10px] font-semibold uppercase tracking-widest" style={{ color: "var(--text-muted)" }}>
              {message.role === "assistant" ? "Clarken" : message.role === "user" ? "Prompt" : "Error"}
            </div>
            <div className="whitespace-pre-wrap text-sm leading-6" style={{ color: "var(--text)" }}>
              {message.text}
            </div>
            {message.role === "assistant" && message.latencyUs !== undefined && (
              <div className="mt-3 grid grid-cols-3 gap-2 text-[11px]">
                <div className="rounded-lg px-2 py-1.5" style={{ background: "rgba(96,165,250,0.08)" }}>
                  <div style={{ color: "var(--text-muted)" }}>Total</div>
                  <div className="font-semibold" style={{ color: "var(--accent-blue)" }}>{fmtLatency(message.latencyUs)}</div>
                </div>
                <div className="rounded-lg px-2 py-1.5" style={{ background: "rgba(0,229,160,0.08)" }}>
                  <div style={{ color: "var(--text-muted)" }}>First token</div>
                  <div className="font-semibold" style={{ color: "var(--accent-green)" }}>{fmtLatency(message.firstTokenLatencyUs)}</div>
                </div>
                <div className="rounded-lg px-2 py-1.5" style={{ background: "rgba(251,191,36,0.08)" }}>
                  <div style={{ color: "var(--text-muted)" }}>Tokens</div>
                  <div className="font-semibold" style={{ color: "var(--accent-amber)" }}>{message.tokensGenerated ?? 0}</div>
                </div>
              </div>
            )}
          </div>
        ))}
        {isSending && (
          <div
            className="self-start rounded-2xl px-4 py-3"
            style={{ background: "rgba(22,29,43,0.92)", border: "1px solid var(--border)" }}
          >
            <div className="mb-1 text-[10px] font-semibold uppercase tracking-widest" style={{ color: "var(--text-muted)" }}>
              Clarken
            </div>
            <div className="text-sm" style={{ color: "var(--text-muted)" }}>Generating response...</div>
          </div>
        )}
      </div>

      {latestStats && (
        <div className="grid grid-cols-3 gap-3 text-xs">
          <div className="card p-3">
            <div style={{ color: "var(--text-muted)" }}>Latest total latency</div>
            <div className="mt-1 font-semibold" style={{ color: "var(--accent-blue)" }}>{fmtLatency(latestStats.latencyUs)}</div>
          </div>
          <div className="card p-3">
            <div style={{ color: "var(--text-muted)" }}>Latest first token</div>
            <div className="mt-1 font-semibold" style={{ color: "var(--accent-green)" }}>{fmtLatency(latestStats.firstTokenLatencyUs)}</div>
          </div>
          <div className="card p-3">
            <div style={{ color: "var(--text-muted)" }}>Latest token count</div>
            <div className="mt-1 font-semibold" style={{ color: "var(--accent-amber)" }}>{latestStats.tokensGenerated ?? 0}</div>
          </div>
        </div>
      )}

      <div className="flex flex-col gap-3">
        <textarea
          value={prompt}
          onChange={(event) => setPrompt(event.target.value)}
          onKeyDown={handlePromptKeyDown}
          rows={4}
          className="rounded-xl px-4 py-3 outline-none resize-none"
          style={{ background: "var(--bg)", border: "1px solid var(--border)", color: "var(--text)" }}
          placeholder="Ask Clarken about credit risk, liquidity, execution latency, or test prompts for the active model..."
        />
        <div className="flex items-center justify-between gap-3">
          <div className="text-xs" style={{ color: "var(--text-muted)" }}>
            Press Enter to send, Shift+Enter for a new line. Use this space for live Clarken conversations and quick answer checks.
          </div>
          <button
            onClick={sendPrompt}
            disabled={isSending || !prompt.trim()}
            className="text-xs font-semibold px-4 py-2 rounded-lg transition-all"
            style={{
              background: isSending || !prompt.trim() ? "rgba(255,255,255,0.06)" : "rgba(0,229,160,0.14)",
              border: isSending || !prompt.trim() ? "1px solid var(--border)" : "1px solid rgba(0,229,160,0.35)",
              color: isSending || !prompt.trim() ? "var(--text-dim)" : "var(--accent-green)",
            }}
          >
            {isSending ? "Sending..." : "Send prompt"}
          </button>
        </div>
      </div>
    </div>
  );
}