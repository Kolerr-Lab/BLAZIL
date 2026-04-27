(globalThis["TURBOPACK"] || (globalThis["TURBOPACK"] = [])).push([typeof document === "object" ? document.currentScript : undefined,
"[project]/src/hooks/useBenchWS.ts [app-client] (ecmascript)", ((__turbopack_context__) => {
"use strict";

__turbopack_context__.s([
    "useBenchWS",
    ()=>useBenchWS
]);
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$index$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/next/dist/compiled/react/index.js [app-client] (ecmascript)");
var _s = __turbopack_context__.k.signature();
"use client";
;
const MAX_HISTORY = 600; // 10 minutes at 1s resolution
const INITIAL_RETRY_DELAY_MS = 5_000; // Start with 5s
const MAX_RETRY_DELAY_MS = 60_000; // Cap at 60s
function initialState() {
    return {
        mode: "dataloader",
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
        current_p99_us: 0
    };
}
function useBenchWS(wsUrl) {
    _s();
    const [state, setState] = (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$index$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["useState"])(initialState);
    const wsRef = (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$index$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["useRef"])(null);
    // Track summary received synchronously so onclose never races with setState.
    const summaryReceivedRef = (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$index$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["useRef"])(false);
    // Exponential backoff: increase delay on each failure, reset on success
    const retryDelayRef = (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$index$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["useRef"])(INITIAL_RETRY_DELAY_MS);
    const handleMessage = (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$index$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["useCallback"])({
        "useBenchWS.useCallback[handleMessage]": (raw)=>{
            let msg;
            try {
                msg = JSON.parse(raw);
            } catch  {
                return;
            }
            if (msg.type === "config") {
                summaryReceivedRef.current = false;
                // Detect mode from config: 'mode' field for AI, 'shards' for fintech (reject fintech)
                const mode = 'mode' in msg && msg.mode === 'dataloader' ? 'dataloader' : 'mode' in msg && msg.mode === 'inference' ? 'inference' : 'dataloader'; // default to dataloader for AI
                setState({
                    "useBenchWS.useCallback[handleMessage]": ()=>({
                            ...initialState(),
                            mode,
                            status: "running",
                            config: msg
                        })
                }["useBenchWS.useCallback[handleMessage]"]);
                return;
            }
            if (msg.type === "event") {
                setState({
                    "useBenchWS.useCallback[handleMessage]": (p)=>({
                            ...p,
                            events: [
                                ...p.events.slice(-99),
                                msg
                            ]
                        })
                }["useBenchWS.useCallback[handleMessage]"]);
                return;
            }
            if (msg.type === "summary") {
                summaryReceivedRef.current = true;
                setState({
                    "useBenchWS.useCallback[handleMessage]": (p)=>({
                            ...p,
                            status: "completed",
                            summary: msg
                        })
                }["useBenchWS.useCallback[handleMessage]"]);
                return;
            }
            if (msg.type === "tick") {
                const tick = msg;
                const t = tick.t;
                // AI dashboard only handles AI tick messages (dataloader or inference)
                // Reject fintech ticks (which have 'shard_id' instead of 'mode')
                if ('mode' in tick && tick.mode === 'dataloader') {
                    const tps = tick.samples_per_sec;
                    const snap = {
                        t,
                        tps,
                        per_shard: [],
                        error_rate: tick.total_samples > 0 ? tick.total_errors / tick.total_samples * 100 : 0,
                        p50_us: 0,
                        p99_us: 0,
                        total_committed: tick.total_samples,
                        total_rejected: tick.total_errors
                    };
                    setState({
                        "useBenchWS.useCallback[handleMessage]": (prev)=>({
                                ...prev,
                                elapsed_secs: t,
                                status: "running",
                                current_tps: tps,
                                peak_tps: Math.max(prev.peak_tps, tps),
                                total_samples: tick.total_samples,
                                total_errors: tick.total_errors,
                                history: [
                                    ...prev.history.slice(-(MAX_HISTORY - 1)),
                                    snap
                                ]
                            })
                    }["useBenchWS.useCallback[handleMessage]"]);
                } else if ('mode' in tick && tick.mode === 'inference') {
                    const tps = tick.rps;
                    const snap = {
                        t,
                        tps,
                        per_shard: [],
                        error_rate: tick.total_samples > 0 ? tick.total_errors / tick.total_samples * 100 : 0,
                        p50_us: 0,
                        p99_us: 0,
                        total_committed: tick.total_predictions,
                        total_rejected: tick.total_errors
                    };
                    setState({
                        "useBenchWS.useCallback[handleMessage]": (prev)=>({
                                ...prev,
                                elapsed_secs: t,
                                status: "running",
                                current_tps: tps,
                                peak_tps: Math.max(prev.peak_tps, tps),
                                total_samples: tick.total_samples,
                                total_predictions: tick.total_predictions,
                                total_errors: tick.total_errors,
                                history: [
                                    ...prev.history.slice(-(MAX_HISTORY - 1)),
                                    snap
                                ]
                            })
                    }["useBenchWS.useCallback[handleMessage]"]);
                }
            // Silently ignore fintech ticks (shard_id-based)
            }
        }
    }["useBenchWS.useCallback[handleMessage]"], []);
    const connect = (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$index$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["useCallback"])({
        "useBenchWS.useCallback[connect]": ()=>{
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
            setState({
                "useBenchWS.useCallback[connect]": (p)=>({
                        ...p,
                        status: "connecting"
                    })
            }["useBenchWS.useCallback[connect]"]);
            const ws = new WebSocket(wsUrl);
            wsRef.current = ws;
            ws.onopen = ({
                "useBenchWS.useCallback[connect]": ()=>{
                    // Reset backoff on successful connection
                    retryDelayRef.current = INITIAL_RETRY_DELAY_MS;
                    setState({
                        "useBenchWS.useCallback[connect]": (p)=>({
                                ...p,
                                status: "connecting"
                            })
                    }["useBenchWS.useCallback[connect]"]);
                }
            })["useBenchWS.useCallback[connect]"];
            ws.onmessage = ({
                "useBenchWS.useCallback[connect]": (e)=>handleMessage(e.data)
            })["useBenchWS.useCallback[connect]"];
            ws.onerror = ({
                "useBenchWS.useCallback[connect]": ()=>{
                    if (!summaryReceivedRef.current) {
                        // Exponential backoff: double the delay, cap at max
                        retryDelayRef.current = Math.min(retryDelayRef.current * 2, MAX_RETRY_DELAY_MS);
                        setState({
                            "useBenchWS.useCallback[connect]": (p)=>({
                                    ...p,
                                    status: "error"
                                })
                        }["useBenchWS.useCallback[connect]"]);
                    }
                }
            })["useBenchWS.useCallback[connect]"];
            ws.onclose = ({
                "useBenchWS.useCallback[connect]": ()=>{
                    if (!summaryReceivedRef.current) {
                        // covers "connecting" (bench not ready yet) AND "running" (bench crashed)
                        setState({
                            "useBenchWS.useCallback[connect]": (p)=>p.status === "completed" ? p : {
                                    ...p,
                                    status: "error"
                                }
                        }["useBenchWS.useCallback[connect]"]);
                    }
                }
            })["useBenchWS.useCallback[connect]"];
        }
    }["useBenchWS.useCallback[connect]"], [
        wsUrl,
        handleMessage
    ]);
    const disconnect = (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$index$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["useCallback"])({
        "useBenchWS.useCallback[disconnect]": ()=>{
            if (wsRef.current) {
                const old = wsRef.current;
                old.onopen = null;
                old.onmessage = null;
                old.onerror = null;
                old.onclose = null;
                old.close();
                wsRef.current = null;
            }
            setState({
                "useBenchWS.useCallback[disconnect]": (p)=>({
                        ...p,
                        status: "idle"
                    })
            }["useBenchWS.useCallback[disconnect]"]);
        }
    }["useBenchWS.useCallback[disconnect]"], []);
    /** Send a control command to the bench process via the WS connection.
   *  The bench WS server forwards it to all scenario subscribers. */ const sendCommand = (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$index$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["useCallback"])({
        "useBenchWS.useCallback[sendCommand]": (cmd)=>{
            if (wsRef.current?.readyState === WebSocket.OPEN) {
                wsRef.current.send(JSON.stringify(cmd));
            }
        }
    }["useBenchWS.useCallback[sendCommand]"], []);
    (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$index$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["useEffect"])({
        "useBenchWS.useEffect": ()=>{
            return ({
                "useBenchWS.useEffect": ()=>{
                    wsRef.current?.close();
                }
            })["useBenchWS.useEffect"];
        }
    }["useBenchWS.useEffect"], []);
    return {
        state,
        connect,
        disconnect,
        sendCommand
    };
}
_s(useBenchWS, "9LFHbqf7cERc3242gQlQmM0k6ZE=");
if (typeof globalThis.$RefreshHelpers$ === 'object' && globalThis.$RefreshHelpers !== null) {
    __turbopack_context__.k.registerExports(__turbopack_context__.m, globalThis.$RefreshHelpers$);
}
}),
"[project]/src/components/Header.tsx [app-client] (ecmascript)", ((__turbopack_context__) => {
"use strict";

__turbopack_context__.s([
    "Header",
    ()=>Header
]);
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/next/dist/compiled/react/jsx-dev-runtime.js [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$image$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/next/image.js [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$clsx$2f$dist$2f$clsx$2e$mjs__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/clsx/dist/clsx.mjs [app-client] (ecmascript)");
"use client";
;
;
;
const STATUS_LABEL = {
    idle: "Ready to connect",
    connecting: "Connecting to ml-bench...",
    running: "RUNNING",
    completed: "COMPLETED",
    error: "Connection failed (retrying with backoff)"
};
const STATUS_COLOR = {
    idle: "text-[var(--text-muted)]",
    connecting: "text-[var(--accent-amber)]",
    running: "text-[var(--accent-green)]",
    completed: "text-[var(--accent-blue)]",
    error: "text-[var(--accent-red)]"
};
function fmtTime(secs) {
    const m = Math.floor(secs / 60);
    const s = secs % 60;
    return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
}
function Header({ status, config, elapsedSecs, wsUrl, onWsUrlChange, onConnect, onDisconnect }) {
    const isRunning = status === "running" || status === "connecting";
    return /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("header", {
        className: "sticky top-0 z-50 flex items-center justify-between gap-4 px-6 py-3",
        style: {
            background: "rgba(8,11,18,0.92)",
            backdropFilter: "blur(16px)",
            borderBottom: "1px solid var(--border)"
        },
        children: [
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                className: "flex items-center gap-3 min-w-0",
                children: [
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                        className: "flex items-center justify-center w-8 h-8 rounded-lg overflow-hidden",
                        style: {
                            background: "#000",
                            border: "1px solid var(--border-bright)"
                        },
                        children: /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$image$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["default"], {
                            src: "/blazil-icon.jpg",
                            alt: "Blazil",
                            width: 32,
                            height: 32,
                            className: "object-cover",
                            priority: true
                        }, void 0, false, {
                            fileName: "[project]/src/components/Header.tsx",
                            lineNumber: 65,
                            columnNumber: 11
                        }, this)
                    }, void 0, false, {
                        fileName: "[project]/src/components/Header.tsx",
                        lineNumber: 61,
                        columnNumber: 9
                    }, this),
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                        children: [
                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                                className: "font-bold text-sm tracking-wide text-white",
                                children: "BLAZIL"
                            }, void 0, false, {
                                fileName: "[project]/src/components/Header.tsx",
                                lineNumber: 75,
                                columnNumber: 11
                            }, this),
                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                                className: "text-[10px] text-[var(--text-muted)] -mt-0.5",
                                children: "AI INFERENCE DASHBOARD"
                            }, void 0, false, {
                                fileName: "[project]/src/components/Header.tsx",
                                lineNumber: 78,
                                columnNumber: 11
                            }, this)
                        ]
                    }, void 0, true, {
                        fileName: "[project]/src/components/Header.tsx",
                        lineNumber: 74,
                        columnNumber: 9
                    }, this),
                    config && /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                        className: "hidden md:flex items-center gap-2 ml-4 px-3 py-1 rounded-full text-xs",
                        style: {
                            background: "rgba(0,229,160,0.08)",
                            border: "1px solid rgba(0,229,160,0.2)",
                            color: "var(--accent-green)"
                        },
                        children: [
                            'mode' in config && /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("span", {
                                children: config.mode
                            }, void 0, false, {
                                fileName: "[project]/src/components/Header.tsx",
                                lineNumber: 92,
                                columnNumber: 34
                            }, this),
                            'mode' in config && /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("span", {
                                style: {
                                    color: "var(--border-bright)"
                                },
                                children: "·"
                            }, void 0, false, {
                                fileName: "[project]/src/components/Header.tsx",
                                lineNumber: 93,
                                columnNumber: 34
                            }, this),
                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("span", {
                                children: config.duration_secs ? `${config.duration_secs}s` : "event mode"
                            }, void 0, false, {
                                fileName: "[project]/src/components/Header.tsx",
                                lineNumber: 94,
                                columnNumber: 13
                            }, this),
                            'dataset' in config && /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("span", {
                                style: {
                                    color: "var(--border-bright)"
                                },
                                children: "·"
                            }, void 0, false, {
                                fileName: "[project]/src/components/Header.tsx",
                                lineNumber: 97,
                                columnNumber: 37
                            }, this),
                            'dataset' in config && /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("span", {
                                className: "truncate max-w-[120px]",
                                title: config.dataset,
                                children: config.dataset
                            }, void 0, false, {
                                fileName: "[project]/src/components/Header.tsx",
                                lineNumber: 99,
                                columnNumber: 15
                            }, this),
                            'batch_size' in config && /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("span", {
                                style: {
                                    color: "var(--border-bright)"
                                },
                                children: "·"
                            }, void 0, false, {
                                fileName: "[project]/src/components/Header.tsx",
                                lineNumber: 103,
                                columnNumber: 40
                            }, this),
                            'batch_size' in config && /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("span", {
                                children: [
                                    "batch=",
                                    String(config.batch_size)
                                ]
                            }, void 0, true, {
                                fileName: "[project]/src/components/Header.tsx",
                                lineNumber: 104,
                                columnNumber: 40
                            }, this),
                            'num_workers' in config && /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("span", {
                                style: {
                                    color: "var(--border-bright)"
                                },
                                children: "·"
                            }, void 0, false, {
                                fileName: "[project]/src/components/Header.tsx",
                                lineNumber: 105,
                                columnNumber: 41
                            }, this),
                            'num_workers' in config && /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("span", {
                                children: [
                                    "workers=",
                                    String(config.num_workers)
                                ]
                            }, void 0, true, {
                                fileName: "[project]/src/components/Header.tsx",
                                lineNumber: 106,
                                columnNumber: 41
                            }, this)
                        ]
                    }, void 0, true, {
                        fileName: "[project]/src/components/Header.tsx",
                        lineNumber: 84,
                        columnNumber: 11
                    }, this)
                ]
            }, void 0, true, {
                fileName: "[project]/src/components/Header.tsx",
                lineNumber: 60,
                columnNumber: 7
            }, this),
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                className: "flex items-center gap-3",
                children: [
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                        className: "flex items-center gap-2",
                        children: [
                            status === "running" && /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                                className: "relative flex items-center justify-center w-3 h-3",
                                children: [
                                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                                        className: "absolute w-3 h-3 rounded-full pulse-ring",
                                        style: {
                                            background: "var(--accent-green)",
                                            opacity: 0.4
                                        }
                                    }, void 0, false, {
                                        fileName: "[project]/src/components/Header.tsx",
                                        lineNumber: 116,
                                        columnNumber: 15
                                    }, this),
                                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                                        className: "w-2 h-2 rounded-full",
                                        style: {
                                            background: "var(--accent-green)"
                                        }
                                    }, void 0, false, {
                                        fileName: "[project]/src/components/Header.tsx",
                                        lineNumber: 120,
                                        columnNumber: 15
                                    }, this)
                                ]
                            }, void 0, true, {
                                fileName: "[project]/src/components/Header.tsx",
                                lineNumber: 115,
                                columnNumber: 13
                            }, this),
                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("span", {
                                className: (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$clsx$2f$dist$2f$clsx$2e$mjs__$5b$app$2d$client$5d$__$28$ecmascript$29$__["clsx"])("text-xs font-semibold tracking-widest", STATUS_COLOR[status]),
                                children: STATUS_LABEL[status]
                            }, void 0, false, {
                                fileName: "[project]/src/components/Header.tsx",
                                lineNumber: 126,
                                columnNumber: 11
                            }, this)
                        ]
                    }, void 0, true, {
                        fileName: "[project]/src/components/Header.tsx",
                        lineNumber: 113,
                        columnNumber: 9
                    }, this),
                    elapsedSecs > 0 && /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                        className: "font-mono text-sm font-bold",
                        style: {
                            color: "var(--accent-green)"
                        },
                        children: fmtTime(elapsedSecs)
                    }, void 0, false, {
                        fileName: "[project]/src/components/Header.tsx",
                        lineNumber: 131,
                        columnNumber: 11
                    }, this)
                ]
            }, void 0, true, {
                fileName: "[project]/src/components/Header.tsx",
                lineNumber: 112,
                columnNumber: 7
            }, this),
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                className: "flex items-center gap-2",
                children: [
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("input", {
                        type: "text",
                        value: wsUrl,
                        onChange: (e)=>onWsUrlChange(e.target.value),
                        disabled: isRunning,
                        className: "hidden md:block text-xs font-mono rounded-lg px-3 py-1.5 w-56 outline-none",
                        style: {
                            background: "var(--bg-card)",
                            border: "1px solid var(--border)",
                            color: "var(--text)",
                            opacity: isRunning ? 0.6 : 1
                        },
                        placeholder: "ws://host:9090/ws"
                    }, void 0, false, {
                        fileName: "[project]/src/components/Header.tsx",
                        lineNumber: 142,
                        columnNumber: 9
                    }, this),
                    isRunning ? /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("button", {
                        onClick: onDisconnect,
                        className: "text-xs font-semibold px-4 py-1.5 rounded-lg transition-all",
                        style: {
                            background: "rgba(239,68,68,0.15)",
                            border: "1px solid rgba(239,68,68,0.3)",
                            color: "var(--accent-red)"
                        },
                        children: "Disconnect"
                    }, void 0, false, {
                        fileName: "[project]/src/components/Header.tsx",
                        lineNumber: 157,
                        columnNumber: 11
                    }, this) : /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("button", {
                        onClick: onConnect,
                        className: "text-xs font-semibold px-4 py-1.5 rounded-lg transition-all",
                        style: {
                            background: "rgba(0,229,160,0.12)",
                            border: "1px solid rgba(0,229,160,0.3)",
                            color: "var(--accent-green)"
                        },
                        children: "Connect"
                    }, void 0, false, {
                        fileName: "[project]/src/components/Header.tsx",
                        lineNumber: 169,
                        columnNumber: 11
                    }, this)
                ]
            }, void 0, true, {
                fileName: "[project]/src/components/Header.tsx",
                lineNumber: 141,
                columnNumber: 7
            }, this)
        ]
    }, void 0, true, {
        fileName: "[project]/src/components/Header.tsx",
        lineNumber: 51,
        columnNumber: 5
    }, this);
}
_c = Header;
var _c;
__turbopack_context__.k.register(_c, "Header");
if (typeof globalThis.$RefreshHelpers$ === 'object' && globalThis.$RefreshHelpers !== null) {
    __turbopack_context__.k.registerExports(__turbopack_context__.m, globalThis.$RefreshHelpers$);
}
}),
"[project]/src/components/HeroMetrics.tsx [app-client] (ecmascript)", ((__turbopack_context__) => {
"use strict";

__turbopack_context__.s([
    "HeroMetrics",
    ()=>HeroMetrics
]);
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/next/dist/compiled/react/jsx-dev-runtime.js [app-client] (ecmascript)");
"use client";
;
function fmtNum(n) {
    if (n >= 1_000_000) return (n / 1_000_000).toFixed(2) + "M";
    if (n >= 1_000) return (n / 1_000).toFixed(1) + "K";
    return n.toLocaleString();
}
function fmtLatency(us) {
    if (us === 0) return "—";
    if (us >= 1_000_000) return (us / 1_000_000).toFixed(2) + " s";
    if (us >= 1_000) return (us / 1_000).toFixed(1) + " ms";
    return us.toLocaleString() + " µs";
}
function BigCard({ label, value, sub, accent = "var(--accent-green)", glow }) {
    return /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
        className: "card flex-1 min-w-0 flex flex-col justify-between p-5",
        style: glow ? {
            boxShadow: `0 0 40px rgba(0,229,160,0.1)`
        } : undefined,
        children: [
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                className: "text-[10px] font-semibold tracking-widest uppercase",
                style: {
                    color: "var(--text-muted)"
                },
                children: label
            }, void 0, false, {
                fileName: "[project]/src/components/HeroMetrics.tsx",
                lineNumber: 36,
                columnNumber: 7
            }, this),
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                className: `font-black text-3xl xl:text-4xl tracking-tight mt-1 tabular-nums ${glow ? "tps-glow" : ""}`,
                style: {
                    color: accent
                },
                children: value
            }, void 0, false, {
                fileName: "[project]/src/components/HeroMetrics.tsx",
                lineNumber: 42,
                columnNumber: 7
            }, this),
            sub && /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                className: "text-xs mt-1",
                style: {
                    color: "var(--text-muted)"
                },
                children: sub
            }, void 0, false, {
                fileName: "[project]/src/components/HeroMetrics.tsx",
                lineNumber: 49,
                columnNumber: 9
            }, this)
        ]
    }, void 0, true, {
        fileName: "[project]/src/components/HeroMetrics.tsx",
        lineNumber: 32,
        columnNumber: 5
    }, this);
}
_c = BigCard;
function StatCard({ label, value, accent = "var(--text)" }) {
    return /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
        className: "card flex flex-col gap-1 p-4",
        children: [
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                className: "text-[10px] font-semibold tracking-widest uppercase",
                style: {
                    color: "var(--text-muted)"
                },
                children: label
            }, void 0, false, {
                fileName: "[project]/src/components/HeroMetrics.tsx",
                lineNumber: 68,
                columnNumber: 7
            }, this),
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                className: "font-bold text-xl tabular-nums",
                style: {
                    color: accent
                },
                children: value
            }, void 0, false, {
                fileName: "[project]/src/components/HeroMetrics.tsx",
                lineNumber: 74,
                columnNumber: 7
            }, this)
        ]
    }, void 0, true, {
        fileName: "[project]/src/components/HeroMetrics.tsx",
        lineNumber: 65,
        columnNumber: 5
    }, this);
}
_c1 = StatCard;
function HeroMetrics({ state }) {
    const { mode, current_tps, peak_tps, total_samples, total_predictions, total_errors, current_p50_us, current_p99_us, summary } = state;
    // AI-specific: dataloader shows samples/sec, inference shows RPS
    const throughputLabel = mode === 'inference' ? 'Request Rate' : 'Throughput';
    const peakLabel = mode === 'inference' ? 'Peak RPS' : 'Peak Samples/s';
    // Calculate bandwidth if summary available
    const bandwidth_gb_s = summary && 'bandwidth_gb_s' in summary ? summary.bandwidth_gb_s : 0;
    return /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
        className: "flex flex-col gap-4",
        children: [
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                className: "flex gap-4",
                children: [
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(BigCard, {
                        label: throughputLabel,
                        value: current_tps > 0 ? fmtNum(current_tps) : "—",
                        sub: current_tps > 0 ? mode === 'inference' ? 'requests/sec' : 'samples/sec' : "Waiting for data…",
                        glow: current_tps > 0
                    }, void 0, false, {
                        fileName: "[project]/src/components/HeroMetrics.tsx",
                        lineNumber: 98,
                        columnNumber: 9
                    }, this),
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(BigCard, {
                        label: peakLabel,
                        value: peak_tps > 0 ? fmtNum(peak_tps) : "—",
                        sub: peak_tps > 0 ? `Best second` : undefined,
                        accent: "var(--accent-blue)"
                    }, void 0, false, {
                        fileName: "[project]/src/components/HeroMetrics.tsx",
                        lineNumber: 104,
                        columnNumber: 9
                    }, this),
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(BigCard, {
                        label: "Bandwidth",
                        value: bandwidth_gb_s > 0 ? `${bandwidth_gb_s.toFixed(2)}` : "—",
                        sub: bandwidth_gb_s > 0 ? "GB/s (transport)" : "Run complete for bandwidth",
                        accent: "var(--accent-purple)"
                    }, void 0, false, {
                        fileName: "[project]/src/components/HeroMetrics.tsx",
                        lineNumber: 110,
                        columnNumber: 9
                    }, this)
                ]
            }, void 0, true, {
                fileName: "[project]/src/components/HeroMetrics.tsx",
                lineNumber: 97,
                columnNumber: 7
            }, this),
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                className: "grid grid-cols-2 md:grid-cols-4 xl:grid-cols-6 gap-3",
                children: [
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(StatCard, {
                        label: mode === 'inference' ? 'Predictions' : 'Samples Loaded',
                        value: total_predictions !== undefined ? total_predictions.toLocaleString() : total_samples > 0 ? total_samples.toLocaleString() : "—",
                        accent: "var(--accent-green)"
                    }, void 0, false, {
                        fileName: "[project]/src/components/HeroMetrics.tsx",
                        lineNumber: 120,
                        columnNumber: 9
                    }, this),
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(StatCard, {
                        label: "Errors",
                        value: total_errors.toLocaleString(),
                        accent: total_errors > 0 ? "var(--accent-red)" : "var(--text-muted)"
                    }, void 0, false, {
                        fileName: "[project]/src/components/HeroMetrics.tsx",
                        lineNumber: 125,
                        columnNumber: 9
                    }, this),
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(StatCard, {
                        label: "Error Rate",
                        value: total_samples > 0 ? `${(total_errors / total_samples * 100).toFixed(3)}%` : "—",
                        accent: total_errors > 0 ? "var(--accent-red)" : "var(--accent-green)"
                    }, void 0, false, {
                        fileName: "[project]/src/components/HeroMetrics.tsx",
                        lineNumber: 130,
                        columnNumber: 9
                    }, this),
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(StatCard, {
                        label: "p50 Latency",
                        value: fmtLatency(current_p50_us),
                        accent: "var(--accent-blue)"
                    }, void 0, false, {
                        fileName: "[project]/src/components/HeroMetrics.tsx",
                        lineNumber: 135,
                        columnNumber: 9
                    }, this),
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(StatCard, {
                        label: "p99 Latency",
                        value: fmtLatency(current_p99_us),
                        accent: "var(--accent-amber)"
                    }, void 0, false, {
                        fileName: "[project]/src/components/HeroMetrics.tsx",
                        lineNumber: 140,
                        columnNumber: 9
                    }, this),
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(StatCard, {
                        label: "Consistency",
                        value: summary && 'consistency' in summary ? `${summary.consistency.toFixed(1)}%` : state.history.length > 10 ? (()=>{
                            const tps = state.history.map((h)=>h.tps).filter((v)=>v > 0);
                            const min = Math.min(...tps);
                            const max = Math.max(...tps);
                            return max > 0 ? `${(min / max * 100).toFixed(1)}%` : "—";
                        })() : "—",
                        accent: "var(--accent-purple)"
                    }, void 0, false, {
                        fileName: "[project]/src/components/HeroMetrics.tsx",
                        lineNumber: 145,
                        columnNumber: 9
                    }, this)
                ]
            }, void 0, true, {
                fileName: "[project]/src/components/HeroMetrics.tsx",
                lineNumber: 119,
                columnNumber: 7
            }, this)
        ]
    }, void 0, true, {
        fileName: "[project]/src/components/HeroMetrics.tsx",
        lineNumber: 95,
        columnNumber: 5
    }, this);
}
_c2 = HeroMetrics;
var _c, _c1, _c2;
__turbopack_context__.k.register(_c, "BigCard");
__turbopack_context__.k.register(_c1, "StatCard");
__turbopack_context__.k.register(_c2, "HeroMetrics");
if (typeof globalThis.$RefreshHelpers$ === 'object' && globalThis.$RefreshHelpers !== null) {
    __turbopack_context__.k.registerExports(__turbopack_context__.m, globalThis.$RefreshHelpers$);
}
}),
"[project]/src/components/TPSChart.tsx [app-client] (ecmascript)", ((__turbopack_context__) => {
"use strict";

__turbopack_context__.s([
    "TPSChart",
    ()=>TPSChart
]);
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/next/dist/compiled/react/jsx-dev-runtime.js [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$chart$2f$AreaChart$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/recharts/es6/chart/AreaChart.js [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$cartesian$2f$Area$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/recharts/es6/cartesian/Area.js [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$cartesian$2f$XAxis$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/recharts/es6/cartesian/XAxis.js [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$cartesian$2f$YAxis$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/recharts/es6/cartesian/YAxis.js [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$cartesian$2f$CartesianGrid$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/recharts/es6/cartesian/CartesianGrid.js [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$component$2f$Tooltip$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/recharts/es6/component/Tooltip.js [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$component$2f$ResponsiveContainer$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/recharts/es6/component/ResponsiveContainer.js [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$cartesian$2f$ReferenceLine$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/recharts/es6/cartesian/ReferenceLine.js [app-client] (ecmascript)");
"use client";
;
;
function formatTPS(v) {
    if (v >= 1_000_000) return `${(v / 1_000_000).toFixed(1)}M`;
    if (v >= 1_000) return `${(v / 1_000).toFixed(0)}K`;
    return `${v}`;
}
function CustomTooltip({ active, payload, label }) {
    if (!active || !payload?.length) return null;
    const tps = payload[0]?.value ?? 0;
    return /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
        className: "px-3 py-2 text-xs rounded-lg font-mono",
        style: {
            background: "var(--bg-card)",
            border: "1px solid var(--border-bright)",
            color: "var(--text)"
        },
        children: [
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                style: {
                    color: "var(--text-muted)"
                },
                children: [
                    "t+",
                    label,
                    "s"
                ]
            }, void 0, true, {
                fileName: "[project]/src/components/TPSChart.tsx",
                lineNumber: 51,
                columnNumber: 7
            }, this),
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                style: {
                    color: "var(--accent-green)",
                    fontSize: 14,
                    fontWeight: 700
                },
                children: [
                    tps.toLocaleString(),
                    " /s"
                ]
            }, void 0, true, {
                fileName: "[project]/src/components/TPSChart.tsx",
                lineNumber: 52,
                columnNumber: 7
            }, this)
        ]
    }, void 0, true, {
        fileName: "[project]/src/components/TPSChart.tsx",
        lineNumber: 43,
        columnNumber: 5
    }, this);
}
_c = CustomTooltip;
function TPSChart({ history, duration_secs }) {
    const data = history.map((h)=>({
            t: h.t,
            tps: h.tps
        }));
    const maxTPS = Math.max(...data.map((d)=>d.tps), 1);
    // Nice Y-axis ceiling
    const yMax = Math.ceil(maxTPS * 1.15 / 100_000) * 100_000 || 100_000;
    return /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
        className: "card p-5 flex flex-col gap-3",
        children: [
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                className: "flex items-center justify-between",
                children: [
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                        children: [
                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                                className: "font-semibold text-sm text-white",
                                children: "Throughput Over Time"
                            }, void 0, false, {
                                fileName: "[project]/src/components/TPSChart.tsx",
                                lineNumber: 69,
                                columnNumber: 11
                            }, this),
                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                                className: "text-xs mt-0.5",
                                style: {
                                    color: "var(--text-muted)"
                                },
                                children: "Samples/sec (dataloader) or Requests/sec (inference)"
                            }, void 0, false, {
                                fileName: "[project]/src/components/TPSChart.tsx",
                                lineNumber: 72,
                                columnNumber: 11
                            }, this)
                        ]
                    }, void 0, true, {
                        fileName: "[project]/src/components/TPSChart.tsx",
                        lineNumber: 68,
                        columnNumber: 9
                    }, this),
                    data.length > 0 && /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                        className: "text-xs font-mono px-3 py-1 rounded-full",
                        style: {
                            background: "rgba(0,229,160,0.08)",
                            color: "var(--accent-green)",
                            border: "1px solid rgba(0,229,160,0.15)"
                        },
                        children: [
                            data.length,
                            "s recorded"
                        ]
                    }, void 0, true, {
                        fileName: "[project]/src/components/TPSChart.tsx",
                        lineNumber: 77,
                        columnNumber: 11
                    }, this)
                ]
            }, void 0, true, {
                fileName: "[project]/src/components/TPSChart.tsx",
                lineNumber: 67,
                columnNumber: 7
            }, this),
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                style: {
                    height: 240
                },
                children: data.length === 0 ? /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                    className: "h-full flex items-center justify-center",
                    children: /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                        className: "text-sm",
                        style: {
                            color: "var(--text-muted)"
                        },
                        children: "Waiting for bench data…"
                    }, void 0, false, {
                        fileName: "[project]/src/components/TPSChart.tsx",
                        lineNumber: 93,
                        columnNumber: 13
                    }, this)
                }, void 0, false, {
                    fileName: "[project]/src/components/TPSChart.tsx",
                    lineNumber: 92,
                    columnNumber: 11
                }, this) : /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$component$2f$ResponsiveContainer$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["ResponsiveContainer"], {
                    width: "100%",
                    height: "100%",
                    children: /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$chart$2f$AreaChart$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["AreaChart"], {
                        data: data,
                        margin: {
                            top: 4,
                            right: 8,
                            left: 0,
                            bottom: 0
                        },
                        children: [
                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("defs", {
                                children: /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("linearGradient", {
                                    id: "tps-gradient",
                                    x1: "0",
                                    y1: "0",
                                    x2: "0",
                                    y2: "1",
                                    children: [
                                        /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("stop", {
                                            offset: "0%",
                                            stopColor: "var(--accent-green)",
                                            stopOpacity: 0.3
                                        }, void 0, false, {
                                            fileName: "[project]/src/components/TPSChart.tsx",
                                            lineNumber: 102,
                                            columnNumber: 19
                                        }, this),
                                        /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("stop", {
                                            offset: "100%",
                                            stopColor: "var(--accent-green)",
                                            stopOpacity: 0.01
                                        }, void 0, false, {
                                            fileName: "[project]/src/components/TPSChart.tsx",
                                            lineNumber: 103,
                                            columnNumber: 19
                                        }, this)
                                    ]
                                }, void 0, true, {
                                    fileName: "[project]/src/components/TPSChart.tsx",
                                    lineNumber: 101,
                                    columnNumber: 17
                                }, this)
                            }, void 0, false, {
                                fileName: "[project]/src/components/TPSChart.tsx",
                                lineNumber: 100,
                                columnNumber: 15
                            }, this),
                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$cartesian$2f$CartesianGrid$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["CartesianGrid"], {
                                strokeDasharray: "3 3",
                                stroke: "var(--border)"
                            }, void 0, false, {
                                fileName: "[project]/src/components/TPSChart.tsx",
                                lineNumber: 106,
                                columnNumber: 15
                            }, this),
                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$cartesian$2f$XAxis$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["XAxis"], {
                                dataKey: "t",
                                tickFormatter: (v)=>`${v}s`,
                                interval: "preserveStartEnd",
                                minTickGap: 60
                            }, void 0, false, {
                                fileName: "[project]/src/components/TPSChart.tsx",
                                lineNumber: 107,
                                columnNumber: 15
                            }, this),
                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$cartesian$2f$YAxis$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["YAxis"], {
                                tickFormatter: formatTPS,
                                domain: [
                                    0,
                                    yMax
                                ],
                                width: 48
                            }, void 0, false, {
                                fileName: "[project]/src/components/TPSChart.tsx",
                                lineNumber: 113,
                                columnNumber: 15
                            }, this),
                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$component$2f$Tooltip$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["Tooltip"], {
                                content: /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(CustomTooltip, {}, void 0, false, {
                                    fileName: "[project]/src/components/TPSChart.tsx",
                                    lineNumber: 118,
                                    columnNumber: 33
                                }, this)
                            }, void 0, false, {
                                fileName: "[project]/src/components/TPSChart.tsx",
                                lineNumber: 118,
                                columnNumber: 15
                            }, this),
                            duration_secs && /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$cartesian$2f$ReferenceLine$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["ReferenceLine"], {
                                x: duration_secs,
                                stroke: "rgba(59,130,246,0.4)",
                                strokeDasharray: "4 4"
                            }, void 0, false, {
                                fileName: "[project]/src/components/TPSChart.tsx",
                                lineNumber: 120,
                                columnNumber: 17
                            }, this),
                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$cartesian$2f$Area$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["Area"], {
                                type: "monotone",
                                dataKey: "tps",
                                stroke: "var(--accent-green)",
                                strokeWidth: 2,
                                fill: "url(#tps-gradient)",
                                dot: false,
                                activeDot: {
                                    r: 4,
                                    stroke: "var(--accent-green)",
                                    strokeWidth: 2,
                                    fill: "var(--bg)"
                                },
                                isAnimationActive: false
                            }, void 0, false, {
                                fileName: "[project]/src/components/TPSChart.tsx",
                                lineNumber: 126,
                                columnNumber: 15
                            }, this)
                        ]
                    }, void 0, true, {
                        fileName: "[project]/src/components/TPSChart.tsx",
                        lineNumber: 99,
                        columnNumber: 13
                    }, this)
                }, void 0, false, {
                    fileName: "[project]/src/components/TPSChart.tsx",
                    lineNumber: 98,
                    columnNumber: 11
                }, this)
            }, void 0, false, {
                fileName: "[project]/src/components/TPSChart.tsx",
                lineNumber: 90,
                columnNumber: 7
            }, this)
        ]
    }, void 0, true, {
        fileName: "[project]/src/components/TPSChart.tsx",
        lineNumber: 66,
        columnNumber: 5
    }, this);
}
_c1 = TPSChart;
var _c, _c1;
__turbopack_context__.k.register(_c, "CustomTooltip");
__turbopack_context__.k.register(_c1, "TPSChart");
if (typeof globalThis.$RefreshHelpers$ === 'object' && globalThis.$RefreshHelpers !== null) {
    __turbopack_context__.k.registerExports(__turbopack_context__.m, globalThis.$RefreshHelpers$);
}
}),
"[project]/src/components/LatencyPanel.tsx [app-client] (ecmascript)", ((__turbopack_context__) => {
"use strict";

__turbopack_context__.s([
    "LatencyPanel",
    ()=>LatencyPanel
]);
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/next/dist/compiled/react/jsx-dev-runtime.js [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$chart$2f$BarChart$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/recharts/es6/chart/BarChart.js [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$cartesian$2f$Bar$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/recharts/es6/cartesian/Bar.js [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$cartesian$2f$XAxis$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/recharts/es6/cartesian/XAxis.js [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$cartesian$2f$YAxis$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/recharts/es6/cartesian/YAxis.js [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$cartesian$2f$CartesianGrid$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/recharts/es6/cartesian/CartesianGrid.js [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$component$2f$Tooltip$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/recharts/es6/component/Tooltip.js [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$component$2f$ResponsiveContainer$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/recharts/es6/component/ResponsiveContainer.js [app-client] (ecmascript)");
"use client";
;
;
function fmtLatency(ns) {
    if (ns === 0) return "—";
    const ms = ns / 1_000_000;
    if (ms >= 1000) return `${(ms / 1000).toFixed(2)}s`;
    return `${ms.toFixed(0)}ms`;
}
function fmtLatencyUs(us) {
    if (us === 0) return "—";
    const ms = us / 1_000;
    if (ms >= 1000) return `${(ms / 1000).toFixed(2)}s`;
    return `${ms.toFixed(0)}ms`;
}
function LatencyRow({ label, value, accent, bar, max }) {
    return /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
        className: "flex items-center gap-3",
        children: [
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                className: "w-12 text-[10px] font-semibold uppercase",
                style: {
                    color: "var(--text-muted)"
                },
                children: label
            }, void 0, false, {
                fileName: "[project]/src/components/LatencyPanel.tsx",
                lineNumber: 47,
                columnNumber: 7
            }, this),
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                className: "flex-1 h-1.5 rounded-full",
                style: {
                    background: "var(--border)"
                },
                children: /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                    className: "h-full rounded-full transition-all duration-500",
                    style: {
                        width: max > 0 ? `${Math.min(bar / max * 100, 100)}%` : "0%",
                        background: accent
                    }
                }, void 0, false, {
                    fileName: "[project]/src/components/LatencyPanel.tsx",
                    lineNumber: 51,
                    columnNumber: 9
                }, this)
            }, void 0, false, {
                fileName: "[project]/src/components/LatencyPanel.tsx",
                lineNumber: 50,
                columnNumber: 7
            }, this),
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                className: "w-16 text-right font-mono text-xs font-semibold",
                style: {
                    color: accent
                },
                children: value
            }, void 0, false, {
                fileName: "[project]/src/components/LatencyPanel.tsx",
                lineNumber: 59,
                columnNumber: 7
            }, this)
        ]
    }, void 0, true, {
        fileName: "[project]/src/components/LatencyPanel.tsx",
        lineNumber: 46,
        columnNumber: 5
    }, this);
}
_c = LatencyRow;
function LiveGauge({ label, valueUs, accent }) {
    return /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
        className: "flex flex-col gap-1 p-3 rounded-lg",
        style: {
            background: "rgba(255,255,255,0.02)",
            border: "1px solid var(--border)"
        },
        children: [
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                className: "text-[10px] uppercase font-semibold",
                style: {
                    color: "var(--text-muted)"
                },
                children: label
            }, void 0, false, {
                fileName: "[project]/src/components/LatencyPanel.tsx",
                lineNumber: 80,
                columnNumber: 7
            }, this),
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                className: "font-mono font-bold text-lg",
                style: {
                    color: accent
                },
                children: fmtLatencyUs(valueUs)
            }, void 0, false, {
                fileName: "[project]/src/components/LatencyPanel.tsx",
                lineNumber: 83,
                columnNumber: 7
            }, this)
        ]
    }, void 0, true, {
        fileName: "[project]/src/components/LatencyPanel.tsx",
        lineNumber: 76,
        columnNumber: 5
    }, this);
}
_c1 = LiveGauge;
function LatencyPanel({ state }) {
    const { summary, current_p50_us, current_p99_us } = state;
    // Type guard for fintech summary (has p50_ns) vs AI summary (has p50_us)
    const p50_ns = summary && 'p50_ns' in summary ? summary.p50_ns : summary && 'p50_us' in summary ? summary.p50_us * 1000 : 0;
    const p99_ns = summary && 'p99_ns' in summary ? summary.p99_ns : summary && 'p99_us' in summary ? summary.p99_us * 1000 : 0;
    const p999_ns = summary && 'p999_ns' in summary ? summary.p999_ns : summary && 'p999_us' in summary ? summary.p999_us * 1000 : 0;
    const mean_ns = summary && 'mean_ns' in summary ? summary.mean_ns : 0;
    const maxBar = p999_ns;
    // Per-second latency history for sparkbar chart.
    const latData = state.history.filter((h)=>h.p50_us > 0 || h.p99_us > 0).slice(-60).map((h)=>({
            t: h.t,
            p50: Math.round(h.p50_us / 1_000),
            p99: Math.round(h.p99_us / 1_000)
        }));
    return /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
        className: "card p-5 flex flex-col gap-4",
        children: [
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                children: [
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                        className: "font-semibold text-sm text-white",
                        children: "Latency"
                    }, void 0, false, {
                        fileName: "[project]/src/components/LatencyPanel.tsx",
                        lineNumber: 113,
                        columnNumber: 9
                    }, this),
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                        className: "text-xs mt-0.5",
                        style: {
                            color: "var(--text-muted)"
                        },
                        children: "Per-batch latency (decode + queue submission)"
                    }, void 0, false, {
                        fileName: "[project]/src/components/LatencyPanel.tsx",
                        lineNumber: 114,
                        columnNumber: 9
                    }, this)
                ]
            }, void 0, true, {
                fileName: "[project]/src/components/LatencyPanel.tsx",
                lineNumber: 112,
                columnNumber: 7
            }, this),
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                className: "grid grid-cols-2 gap-2",
                children: [
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(LiveGauge, {
                        label: "p50 (rolling)",
                        valueUs: current_p50_us,
                        accent: "var(--accent-blue)"
                    }, void 0, false, {
                        fileName: "[project]/src/components/LatencyPanel.tsx",
                        lineNumber: 121,
                        columnNumber: 9
                    }, this),
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(LiveGauge, {
                        label: "p99 (rolling)",
                        valueUs: current_p99_us,
                        accent: "var(--accent-amber)"
                    }, void 0, false, {
                        fileName: "[project]/src/components/LatencyPanel.tsx",
                        lineNumber: 122,
                        columnNumber: 9
                    }, this)
                ]
            }, void 0, true, {
                fileName: "[project]/src/components/LatencyPanel.tsx",
                lineNumber: 120,
                columnNumber: 7
            }, this),
            summary ? /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                className: "flex flex-col gap-3",
                children: [
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                        className: "text-xs font-semibold",
                        style: {
                            color: "var(--text-muted)"
                        },
                        children: "Final Percentiles"
                    }, void 0, false, {
                        fileName: "[project]/src/components/LatencyPanel.tsx",
                        lineNumber: 128,
                        columnNumber: 11
                    }, this),
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(LatencyRow, {
                        label: "mean",
                        value: fmtLatency(mean_ns),
                        accent: "var(--text-muted)",
                        bar: mean_ns,
                        max: maxBar
                    }, void 0, false, {
                        fileName: "[project]/src/components/LatencyPanel.tsx",
                        lineNumber: 131,
                        columnNumber: 11
                    }, this),
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(LatencyRow, {
                        label: "p50",
                        value: fmtLatency(p50_ns),
                        accent: "var(--accent-blue)",
                        bar: p50_ns,
                        max: maxBar
                    }, void 0, false, {
                        fileName: "[project]/src/components/LatencyPanel.tsx",
                        lineNumber: 132,
                        columnNumber: 11
                    }, this),
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(LatencyRow, {
                        label: "p99",
                        value: fmtLatency(p99_ns),
                        accent: "var(--accent-amber)",
                        bar: p99_ns,
                        max: maxBar
                    }, void 0, false, {
                        fileName: "[project]/src/components/LatencyPanel.tsx",
                        lineNumber: 133,
                        columnNumber: 11
                    }, this),
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(LatencyRow, {
                        label: "p99.9",
                        value: fmtLatency(p999_ns),
                        accent: "var(--accent-red)",
                        bar: p999_ns,
                        max: maxBar
                    }, void 0, false, {
                        fileName: "[project]/src/components/LatencyPanel.tsx",
                        lineNumber: 134,
                        columnNumber: 11
                    }, this)
                ]
            }, void 0, true, {
                fileName: "[project]/src/components/LatencyPanel.tsx",
                lineNumber: 127,
                columnNumber: 9
            }, this) : /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                className: "text-xs",
                style: {
                    color: "var(--text-muted)"
                },
                children: "Final percentiles available after run completes."
            }, void 0, false, {
                fileName: "[project]/src/components/LatencyPanel.tsx",
                lineNumber: 137,
                columnNumber: 9
            }, this),
            latData.length > 2 && /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                children: [
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                        className: "text-xs mb-2 font-semibold",
                        style: {
                            color: "var(--text-muted)"
                        },
                        children: "p50 / p99 per second (ms)"
                    }, void 0, false, {
                        fileName: "[project]/src/components/LatencyPanel.tsx",
                        lineNumber: 145,
                        columnNumber: 11
                    }, this),
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                        style: {
                            height: 100
                        },
                        children: /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$component$2f$ResponsiveContainer$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["ResponsiveContainer"], {
                            width: "100%",
                            height: "100%",
                            children: /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$chart$2f$BarChart$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["BarChart"], {
                                data: latData,
                                margin: {
                                    top: 0,
                                    right: 0,
                                    left: 0,
                                    bottom: 0
                                },
                                barGap: 0,
                                children: [
                                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$cartesian$2f$CartesianGrid$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["CartesianGrid"], {
                                        strokeDasharray: "2 2",
                                        stroke: "var(--border)"
                                    }, void 0, false, {
                                        fileName: "[project]/src/components/LatencyPanel.tsx",
                                        lineNumber: 151,
                                        columnNumber: 17
                                    }, this),
                                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$cartesian$2f$XAxis$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["XAxis"], {
                                        dataKey: "t",
                                        hide: true
                                    }, void 0, false, {
                                        fileName: "[project]/src/components/LatencyPanel.tsx",
                                        lineNumber: 152,
                                        columnNumber: 17
                                    }, this),
                                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$cartesian$2f$YAxis$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["YAxis"], {
                                        width: 32,
                                        tickFormatter: (v)=>`${v}`
                                    }, void 0, false, {
                                        fileName: "[project]/src/components/LatencyPanel.tsx",
                                        lineNumber: 153,
                                        columnNumber: 17
                                    }, this),
                                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$component$2f$Tooltip$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["Tooltip"], {
                                        formatter: (v, name)=>[
                                                `${v}ms`,
                                                name
                                            ],
                                        contentStyle: {
                                            background: "var(--bg-card)",
                                            border: "1px solid var(--border-bright)",
                                            borderRadius: 8,
                                            color: "var(--text)",
                                            fontSize: 11
                                        }
                                    }, void 0, false, {
                                        fileName: "[project]/src/components/LatencyPanel.tsx",
                                        lineNumber: 154,
                                        columnNumber: 17
                                    }, this),
                                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$cartesian$2f$Bar$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["Bar"], {
                                        dataKey: "p50",
                                        fill: "var(--accent-blue)",
                                        opacity: 0.7,
                                        isAnimationActive: false
                                    }, void 0, false, {
                                        fileName: "[project]/src/components/LatencyPanel.tsx",
                                        lineNumber: 164,
                                        columnNumber: 17
                                    }, this),
                                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$recharts$2f$es6$2f$cartesian$2f$Bar$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["Bar"], {
                                        dataKey: "p99",
                                        fill: "var(--accent-amber)",
                                        opacity: 0.7,
                                        isAnimationActive: false
                                    }, void 0, false, {
                                        fileName: "[project]/src/components/LatencyPanel.tsx",
                                        lineNumber: 165,
                                        columnNumber: 17
                                    }, this)
                                ]
                            }, void 0, true, {
                                fileName: "[project]/src/components/LatencyPanel.tsx",
                                lineNumber: 150,
                                columnNumber: 15
                            }, this)
                        }, void 0, false, {
                            fileName: "[project]/src/components/LatencyPanel.tsx",
                            lineNumber: 149,
                            columnNumber: 13
                        }, this)
                    }, void 0, false, {
                        fileName: "[project]/src/components/LatencyPanel.tsx",
                        lineNumber: 148,
                        columnNumber: 11
                    }, this)
                ]
            }, void 0, true, {
                fileName: "[project]/src/components/LatencyPanel.tsx",
                lineNumber: 144,
                columnNumber: 9
            }, this)
        ]
    }, void 0, true, {
        fileName: "[project]/src/components/LatencyPanel.tsx",
        lineNumber: 111,
        columnNumber: 5
    }, this);
}
_c2 = LatencyPanel;
var _c, _c1, _c2;
__turbopack_context__.k.register(_c, "LatencyRow");
__turbopack_context__.k.register(_c1, "LiveGauge");
__turbopack_context__.k.register(_c2, "LatencyPanel");
if (typeof globalThis.$RefreshHelpers$ === 'object' && globalThis.$RefreshHelpers !== null) {
    __turbopack_context__.k.registerExports(__turbopack_context__.m, globalThis.$RefreshHelpers$);
}
}),
"[project]/src/app/page.tsx [app-client] (ecmascript)", ((__turbopack_context__) => {
"use strict";

__turbopack_context__.s([
    "default",
    ()=>DashboardPage
]);
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/next/dist/compiled/react/jsx-dev-runtime.js [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$index$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/node_modules/next/dist/compiled/react/index.js [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$src$2f$hooks$2f$useBenchWS$2e$ts__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/src/hooks/useBenchWS.ts [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$src$2f$components$2f$Header$2e$tsx__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/src/components/Header.tsx [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$src$2f$components$2f$HeroMetrics$2e$tsx__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/src/components/HeroMetrics.tsx [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$src$2f$components$2f$TPSChart$2e$tsx__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/src/components/TPSChart.tsx [app-client] (ecmascript)");
var __TURBOPACK__imported__module__$5b$project$5d2f$src$2f$components$2f$LatencyPanel$2e$tsx__$5b$app$2d$client$5d$__$28$ecmascript$29$__ = __turbopack_context__.i("[project]/src/components/LatencyPanel.tsx [app-client] (ecmascript)");
;
var _s = __turbopack_context__.k.signature();
"use client";
;
;
;
;
;
;
// ═══════════════════════════════════════════════════════════════
// BLAZIL AI INFERENCE BENCHMARK DASHBOARD
// ═══════════════════════════════════════════════════════════════
// Port: 3333 (different from fintech:3331)
// WebSocket: ws://localhost:9092/ws (AI only, NOT fintech 9090)
// Benchmark duration: 120s default
// ═══════════════════════════════════════════════════════════════
const DEFAULT_WS_URL = "ws://localhost:9092/ws";
function DashboardPage() {
    _s();
    const [wsUrl, setWsUrl] = (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$index$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["useState"])(DEFAULT_WS_URL);
    const { state, connect, disconnect, sendCommand } = (0, __TURBOPACK__imported__module__$5b$project$5d2f$src$2f$hooks$2f$useBenchWS$2e$ts__$5b$app$2d$client$5d$__$28$ecmascript$29$__["useBenchWS"])(wsUrl);
    // Auto-scroll event log to bottom.
    const eventsEndRef = (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$index$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["useCallback"])({
        "DashboardPage.useCallback[eventsEndRef]": (el)=>{
            el?.scrollIntoView({
                behavior: "smooth"
            });
        }
    }["DashboardPage.useCallback[eventsEndRef]"], []);
    // Auto-connect on mount
    (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$index$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["useEffect"])({
        "DashboardPage.useEffect": ()=>{
            connect();
        }
    }["DashboardPage.useEffect"], []); // eslint-disable-line react-hooks/exhaustive-deps
    // Exponential backoff retry: delay doubles on each failure (5s → 10s → 20s → 40s → 60s max)
    (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$index$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["useEffect"])({
        "DashboardPage.useEffect": ()=>{
            if (state.status !== "error" && state.status !== "idle") return;
            // Retry delay is managed in useBenchWS hook with exponential backoff
            const timer = setTimeout({
                "DashboardPage.useEffect.timer": ()=>connect()
            }["DashboardPage.useEffect.timer"], 5_000);
            return ({
                "DashboardPage.useEffect": ()=>clearTimeout(timer)
            })["DashboardPage.useEffect"];
        }
    }["DashboardPage.useEffect"], [
        state.status,
        connect
    ]);
    // Browser title updates with live throughput.
    (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$index$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["useEffect"])({
        "DashboardPage.useEffect": ()=>{
            if (state.status === "running" && state.current_tps > 0) {
                const throughput = state.current_tps >= 1_000_000 ? `${(state.current_tps / 1_000_000).toFixed(2)}M` : `${(state.current_tps / 1_000).toFixed(0)}K`;
                const label = state.mode === 'inference' ? 'RPS' : 'Samples/s';
                document.title = `${throughput} ${label} — Blazil AI`;
            } else {
                document.title = "Blazil AI Dashboard";
            }
        }
    }["DashboardPage.useEffect"], [
        state.current_tps,
        state.status,
        state.mode
    ]);
    return /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
        className: "min-h-screen flex flex-col",
        style: {
            background: "var(--bg)"
        },
        children: [
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$src$2f$components$2f$Header$2e$tsx__$5b$app$2d$client$5d$__$28$ecmascript$29$__["Header"], {
                status: state.status,
                config: state.config,
                elapsedSecs: state.elapsed_secs,
                wsUrl: wsUrl,
                onWsUrlChange: setWsUrl,
                onConnect: connect,
                onDisconnect: disconnect
            }, void 0, false, {
                fileName: "[project]/src/app/page.tsx",
                lineNumber: 58,
                columnNumber: 7
            }, this),
            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("main", {
                className: "flex-1 px-4 md:px-6 pb-8 pt-5 max-w-[1600px] mx-auto w-full",
                children: [
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$src$2f$components$2f$HeroMetrics$2e$tsx__$5b$app$2d$client$5d$__$28$ecmascript$29$__["HeroMetrics"], {
                        state: state
                    }, void 0, false, {
                        fileName: "[project]/src/app/page.tsx",
                        lineNumber: 70,
                        columnNumber: 9
                    }, this),
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                        className: "mt-5 grid grid-cols-1 xl:grid-cols-3 gap-4",
                        children: [
                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                                className: "xl:col-span-2",
                                children: /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$src$2f$components$2f$TPSChart$2e$tsx__$5b$app$2d$client$5d$__$28$ecmascript$29$__["TPSChart"], {
                                    history: state.history,
                                    duration_secs: state.config?.duration_secs ?? null
                                }, void 0, false, {
                                    fileName: "[project]/src/app/page.tsx",
                                    lineNumber: 75,
                                    columnNumber: 13
                                }, this)
                            }, void 0, false, {
                                fileName: "[project]/src/app/page.tsx",
                                lineNumber: 74,
                                columnNumber: 11
                            }, this),
                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                                className: "xl:col-span-1",
                                children: /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$src$2f$components$2f$LatencyPanel$2e$tsx__$5b$app$2d$client$5d$__$28$ecmascript$29$__["LatencyPanel"], {
                                    state: state
                                }, void 0, false, {
                                    fileName: "[project]/src/app/page.tsx",
                                    lineNumber: 81,
                                    columnNumber: 13
                                }, this)
                            }, void 0, false, {
                                fileName: "[project]/src/app/page.tsx",
                                lineNumber: 80,
                                columnNumber: 11
                            }, this)
                        ]
                    }, void 0, true, {
                        fileName: "[project]/src/app/page.tsx",
                        lineNumber: 73,
                        columnNumber: 9
                    }, this),
                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                        className: "mt-5",
                        children: [
                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                                className: "text-xs font-semibold uppercase tracking-widest mb-3",
                                style: {
                                    color: "var(--text-muted)"
                                },
                                children: "Event Log"
                            }, void 0, false, {
                                fileName: "[project]/src/app/page.tsx",
                                lineNumber: 87,
                                columnNumber: 11
                            }, this),
                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                                className: "card p-3 font-mono text-xs overflow-y-auto",
                                style: {
                                    maxHeight: 200,
                                    minHeight: 80
                                },
                                children: state.events.length === 0 ? /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                                    style: {
                                        color: "var(--text-dim)"
                                    },
                                    children: "No events yet. Connect to a running bench instance."
                                }, void 0, false, {
                                    fileName: "[project]/src/app/page.tsx",
                                    lineNumber: 95,
                                    columnNumber: 15
                                }, this) : /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])(__TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["Fragment"], {
                                    children: [
                                        state.events.map((ev, i)=>/*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                                                className: "flex gap-3 py-0.5",
                                                style: {
                                                    color: ev.kind === "fault_inject" ? "var(--accent-red)" : ev.kind === "fault_recover" ? "var(--accent-green)" : ev.kind === "node_down" ? "var(--accent-red)" : ev.kind === "node_up" ? "var(--accent-green)" : ev.kind === "bench_done" ? "var(--accent-blue)" : "var(--text-muted)"
                                                },
                                                children: [
                                                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("span", {
                                                        style: {
                                                            color: "var(--text-dim)"
                                                        },
                                                        children: [
                                                            "t+",
                                                            ev.t,
                                                            "s"
                                                        ]
                                                    }, void 0, true, {
                                                        fileName: "[project]/src/app/page.tsx",
                                                        lineNumber: 119,
                                                        columnNumber: 21
                                                    }, this),
                                                    /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("span", {
                                                        children: ev.message
                                                    }, void 0, false, {
                                                        fileName: "[project]/src/app/page.tsx",
                                                        lineNumber: 120,
                                                        columnNumber: 21
                                                    }, this)
                                                ]
                                            }, i, true, {
                                                fileName: "[project]/src/app/page.tsx",
                                                lineNumber: 101,
                                                columnNumber: 19
                                            }, this)),
                                        /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                                            ref: eventsEndRef
                                        }, void 0, false, {
                                            fileName: "[project]/src/app/page.tsx",
                                            lineNumber: 123,
                                            columnNumber: 17
                                        }, this)
                                    ]
                                }, void 0, true)
                            }, void 0, false, {
                                fileName: "[project]/src/app/page.tsx",
                                lineNumber: 90,
                                columnNumber: 11
                            }, this)
                        ]
                    }, void 0, true, {
                        fileName: "[project]/src/app/page.tsx",
                        lineNumber: 86,
                        columnNumber: 9
                    }, this),
                    state.summary && /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                        className: "mt-5",
                        children: [
                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                                className: "text-xs font-semibold uppercase tracking-widest mb-3",
                                style: {
                                    color: "var(--text-muted)"
                                },
                                children: "Run Summary"
                            }, void 0, false, {
                                fileName: "[project]/src/app/page.tsx",
                                lineNumber: 132,
                                columnNumber: 13
                            }, this),
                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                                className: "card p-5 grid grid-cols-2 md:grid-cols-4 xl:grid-cols-8 gap-4",
                                children: (()=>{
                                    /* Fintech mode summary */ if ('avg_tps' in state.summary) {
                                        const s = state.summary;
                                        return [
                                            {
                                                label: "Average TPS",
                                                value: s.avg_tps.toLocaleString(),
                                                accent: "var(--accent-green)"
                                            },
                                            {
                                                label: "Peak TPS",
                                                value: s.max_tps.toLocaleString(),
                                                accent: "var(--accent-green)"
                                            },
                                            {
                                                label: "Min TPS",
                                                value: s.min_tps.toLocaleString(),
                                                accent: "var(--text-muted)"
                                            },
                                            {
                                                label: "Consistency",
                                                value: `${s.consistency.toFixed(1)}%`,
                                                accent: "var(--accent-purple)"
                                            },
                                            {
                                                label: "Committed",
                                                value: s.total_committed.toLocaleString(),
                                                accent: "var(--accent-green)"
                                            },
                                            {
                                                label: "Rejected",
                                                value: s.total_rejected.toLocaleString(),
                                                accent: s.total_rejected > 0 ? "var(--accent-red)" : "var(--text-muted)"
                                            },
                                            {
                                                label: "Error Rate",
                                                value: `${s.error_rate.toFixed(3)}%`,
                                                accent: s.error_rate > 0.01 ? "var(--accent-red)" : "var(--accent-green)"
                                            },
                                            {
                                                label: "Survival Rate",
                                                value: `${s.survival_rate.toFixed(2)}%`,
                                                accent: "var(--accent-green)"
                                            }
                                        ];
                                    }
                                    /* Inference mode summary */ if ('rps' in state.summary) {
                                        const s = state.summary;
                                        return [
                                            {
                                                label: "RPS",
                                                value: s.rps.toFixed(0),
                                                accent: "var(--accent-green)"
                                            },
                                            {
                                                label: "Bandwidth",
                                                value: `${s.bandwidth_gb_s.toFixed(2)} GB/s`,
                                                accent: "var(--accent-blue)"
                                            },
                                            {
                                                label: "Total Data",
                                                value: `${s.total_gb.toFixed(1)} GB`,
                                                accent: "var(--text-muted)"
                                            },
                                            {
                                                label: "Total Predictions",
                                                value: s.total_predictions.toLocaleString(),
                                                accent: "var(--accent-green)"
                                            },
                                            {
                                                label: "Total Samples",
                                                value: s.total_samples.toLocaleString(),
                                                accent: "var(--text)"
                                            },
                                            {
                                                label: "Errors",
                                                value: s.total_errors.toLocaleString(),
                                                accent: s.total_errors > 0 ? "var(--accent-red)" : "var(--accent-green)"
                                            },
                                            {
                                                label: "Error Rate",
                                                value: `${s.error_rate.toFixed(3)}%`,
                                                accent: s.error_rate > 0.01 ? "var(--accent-red)" : "var(--accent-green)"
                                            },
                                            {
                                                label: "P99 Latency",
                                                value: `${(s.p99_us / 1000).toFixed(2)}ms`,
                                                accent: "var(--accent-amber)"
                                            }
                                        ];
                                    }
                                    /* Dataloader mode summary */ const s = state.summary;
                                    return [
                                        {
                                            label: "Samples/sec",
                                            value: s.samples_per_sec.toFixed(0),
                                            accent: "var(--accent-green)"
                                        },
                                        {
                                            label: "Bandwidth",
                                            value: `${s.bandwidth_gb_s.toFixed(2)} GB/s`,
                                            accent: "var(--accent-blue)"
                                        },
                                        {
                                            label: "Total Data",
                                            value: `${s.total_gb.toFixed(1)} GB`,
                                            accent: "var(--text-muted)"
                                        },
                                        {
                                            label: "Total Samples",
                                            value: s.total_samples.toLocaleString(),
                                            accent: "var(--accent-green)"
                                        },
                                        {
                                            label: "Total Batches",
                                            value: s.total_batches.toLocaleString(),
                                            accent: "var(--text)"
                                        },
                                        {
                                            label: "Errors",
                                            value: s.total_errors.toLocaleString(),
                                            accent: s.total_errors > 0 ? "var(--accent-red)" : "var(--accent-green)"
                                        },
                                        {
                                            label: "Error Rate",
                                            value: `${s.error_rate.toFixed(3)}%`,
                                            accent: s.error_rate > 0.01 ? "var(--accent-red)" : "var(--accent-green)"
                                        },
                                        {
                                            label: "P99 Latency",
                                            value: `${(s.p99_us / 1000).toFixed(2)}ms`,
                                            accent: "var(--accent-amber)"
                                        }
                                    ];
                                })().map(({ label, value, accent })=>/*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                                        className: "flex flex-col gap-1",
                                        children: [
                                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                                                className: "text-[10px] uppercase font-semibold tracking-widest",
                                                style: {
                                                    color: "var(--text-muted)"
                                                },
                                                children: label
                                            }, void 0, false, {
                                                fileName: "[project]/src/app/page.tsx",
                                                lineNumber: 179,
                                                columnNumber: 19
                                            }, this),
                                            /*#__PURE__*/ (0, __TURBOPACK__imported__module__$5b$project$5d2f$node_modules$2f$next$2f$dist$2f$compiled$2f$react$2f$jsx$2d$dev$2d$runtime$2e$js__$5b$app$2d$client$5d$__$28$ecmascript$29$__["jsxDEV"])("div", {
                                                className: "font-bold text-lg tabular-nums",
                                                style: {
                                                    color: accent
                                                },
                                                children: value
                                            }, void 0, false, {
                                                fileName: "[project]/src/app/page.tsx",
                                                lineNumber: 182,
                                                columnNumber: 19
                                            }, this)
                                        ]
                                    }, label, true, {
                                        fileName: "[project]/src/app/page.tsx",
                                        lineNumber: 178,
                                        columnNumber: 17
                                    }, this))
                            }, void 0, false, {
                                fileName: "[project]/src/app/page.tsx",
                                lineNumber: 135,
                                columnNumber: 13
                            }, this)
                        ]
                    }, void 0, true, {
                        fileName: "[project]/src/app/page.tsx",
                        lineNumber: 131,
                        columnNumber: 11
                    }, this)
                ]
            }, void 0, true, {
                fileName: "[project]/src/app/page.tsx",
                lineNumber: 68,
                columnNumber: 7
            }, this)
        ]
    }, void 0, true, {
        fileName: "[project]/src/app/page.tsx",
        lineNumber: 57,
        columnNumber: 5
    }, this);
}
_s(DashboardPage, "EEL+kFtWyr5Bwh4weeosQFii4A8=", false, function() {
    return [
        __TURBOPACK__imported__module__$5b$project$5d2f$src$2f$hooks$2f$useBenchWS$2e$ts__$5b$app$2d$client$5d$__$28$ecmascript$29$__["useBenchWS"]
    ];
});
_c = DashboardPage;
var _c;
__turbopack_context__.k.register(_c, "DashboardPage");
if (typeof globalThis.$RefreshHelpers$ === 'object' && globalThis.$RefreshHelpers !== null) {
    __turbopack_context__.k.registerExports(__turbopack_context__.m, globalThis.$RefreshHelpers$);
}
}),
]);

//# sourceMappingURL=src_0_6di64._.js.map