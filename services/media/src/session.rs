//! Per-call session: bridges a Twilio Media Stream to STT → backend turn → TTS.
//!
//! Data flow:
//!   Twilio (μ-law 8k) ──► STT (Scribe v2 Realtime, server-side VAD) ──► committed transcript
//!                    └──► local energy VAD (barge-in detection only)
//!   committed transcript ──► POST /voice/turn (LLM + RAG) ──► answer + voice_id
//!   answer ──► TTS (μ-law 8k) ──► Twilio
//!
//! Turns are driven by STT `committed_transcript` events (consumed in a dedicated task), so
//! there is no local "utterance-end + try_recv" race. Barge-in is detected by a fast local
//! VAD while the assistant is speaking; it cancels playback per-chunk and flushes Twilio's
//! buffer with a `clear`, which works because the outbound sink is locked per send (never for
//! the whole playback).

use crate::{
    codec,
    config::Config,
    error::MediaError,
    stt::SttParams,
    tts::{build_fallback_tts, build_tts, Tts},
    turn::{TurnClient, TurnEvent, TurnRequest},
    turn_detector::SmartTurn,
    twilio::{InboundMessage, OutboundMessage},
    vad::{VadEngine, VadState},
};
use axum::extract::ws::{Message, WebSocket};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::{sync::mpsc, sync::oneshot, sync::Mutex, task::JoinHandle};

type WsSink = SplitSink<WebSocket, Message>;

/// Shared, cloneable per-call state used by the spawned transcript/turn/playback tasks.
#[derive(Clone)]
struct Shared {
    config: Arc<Config>,
    ws_tx: Arc<Mutex<WsSink>>,
    stream_sid: String,
    tenant_id: String,
    agent_id: String,
    /// Backend Call id (from the TwiML <Parameter>). Forwarded to every turn so the orchestrator
    /// persists transcripts and keeps per-call memory. Empty when absent.
    call_id: String,
    /// True while the assistant is producing/playing audio (gates barge-in).
    speaking: Arc<AtomicBool>,
    /// Set true to stop the current playback loop mid-stream (barge-in / supersede).
    play_cancel: Arc<AtomicBool>,
    /// Handle to the in-flight TTS+playback task so it can be aborted.
    tts_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    /// Set once when the call has been ended on our side (e.g. ElevenLabs quota outage). Makes the
    /// quota fallback idempotent — STT and TTS can both trip it, but the clip plays / socket closes
    /// exactly once.
    ended: Arc<AtomicBool>,
}

pub struct Session {
    config: Arc<Config>,
    vad: VadEngine,
    audio_buffer: Vec<u8>,
    stt_tx: Option<mpsc::Sender<Vec<u8>>>,
    /// Predictive endpointing (Bước 3): per-frame (pcm, is_speech, silence_ms) to the endpoint
    /// loop, which runs Smart Turn and commits early. `None` when predictive endpointing is off.
    endpoint_tx: Option<mpsc::Sender<(Vec<i16>, bool, u64)>>,
    /// Keypad digits from Twilio `dtmf` events → the DTMF collector loop, which buffers them and
    /// commits a turn on `#` or after an inter-digit timeout. `None` until the stream starts.
    dtmf_tx: Option<mpsc::Sender<char>>,
    /// Shared Smart Turn model (loaded once at startup, not per call). `None` when predictive
    /// endpointing is off or the model failed to load at boot → falls back to VAD endpointing.
    smart_turn: Option<Arc<SmartTurn>>,
    /// RNNoise denoiser for the STT input (Some only when DENOISE_ENABLED). Attenuates background
    /// noise; never mutes — cannot cause dead-air.
    denoiser: Option<crate::denoise::Denoiser>,
    shared: Option<Shared>,
    tasks: Vec<JoinHandle<()>>,
}

impl Session {
    pub fn new(config: Arc<Config>, smart_turn: Option<Arc<SmartTurn>>) -> Self {
        let vad = VadEngine::new(config.vad_aggressiveness);
        let denoiser = if config.denoise_enabled {
            Some(crate::denoise::Denoiser::new())
        } else {
            None
        };
        Self {
            config,
            vad,
            denoiser,
            audio_buffer: Vec::with_capacity(codec::SAMPLES_PER_FRAME * 4),
            stt_tx: None,
            endpoint_tx: None,
            dtmf_tx: None,
            smart_turn,
            shared: None,
            tasks: Vec::new(),
        }
    }

    pub async fn run(mut self, socket: WebSocket) {
        let (ws_sink, mut ws_rx) = socket.split();
        let ws_tx = Arc::new(Mutex::new(ws_sink));

        while let Some(msg) = ws_rx.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(inbound) = serde_json::from_str::<InboundMessage>(&text) {
                        self.handle_inbound(inbound, &ws_tx).await;
                    }
                }
                Ok(Message::Close(_)) => {
                    tracing::info!("Twilio WS closed by peer");
                    break;
                }
                Err(e) => {
                    // Twilio commonly ends calls with a TCP RST rather than a proper WS
                    // Close handshake, producing `ResetWithoutClosingHandshake`. This is
                    // semantically identical to a normal peer-close (the call ended) and
                    // must not be logged as ERROR — it floods logs and triggers false alerts.
                    let msg = format!("{e:?}");
                    if msg.contains("ResetWithoutClosingHandshake") {
                        tracing::info!(
                            "Twilio WS reset by peer (call ended without close handshake)"
                        );
                    } else {
                        tracing::error!("WS read error: {:?}", e);
                    }
                    break;
                }
                _ => {}
            }
        }

        // Cleanup: stop any playback and drop the spawned STT/transcript tasks.
        if let Some(shared) = &self.shared {
            stop_playback(shared, false).await;
        }
        for t in self.tasks.drain(..) {
            t.abort();
        }
    }

    async fn handle_inbound(&mut self, msg: InboundMessage, ws_tx: &Arc<Mutex<WsSink>>) {
        match msg {
            InboundMessage::Connected { protocol, version } => {
                tracing::info!("Connected: protocol={}, version={}", protocol, version);
            }
            InboundMessage::Start { stream_sid, start } => {
                let params = start.custom_parameters.unwrap_or_default();
                let tenant_id = params.get("tenant_id").cloned().unwrap_or_default();
                let agent_id = params.get("agent_id").cloned().unwrap_or_default();
                let call_id = params.get("call_id").cloned().unwrap_or_default();
                // Per-agent language from the TwiML <Parameter>; empty → auto-detect (falls
                // back to the media plane's global STT_LANGUAGE_CODE / language detection).
                let language = params.get("language").filter(|s| !s.is_empty()).cloned();
                // Lexicon Prompting: Custom vocabulary to bias the STT model (comma-separated).
                let lexicon = params.get("lexicon").filter(|s| !s.is_empty()).cloned();
                tracing::info!(
                    "Started stream {} for call {} (tenant={}, agent={}, language={}, lexicon={:?})",
                    stream_sid,
                    start.call_sid,
                    tenant_id,
                    agent_id,
                    language.as_deref().unwrap_or("auto"),
                    lexicon,
                );

                let shared = Shared {
                    config: Arc::clone(&self.config),
                    ws_tx: Arc::clone(ws_tx),
                    stream_sid,
                    tenant_id,
                    agent_id,
                    call_id,
                    speaking: Arc::new(AtomicBool::new(false)),
                    play_cancel: Arc::new(AtomicBool::new(false)),
                    tts_task: Arc::new(Mutex::new(None)),
                    ended: Arc::new(AtomicBool::new(false)),
                };
                self.shared = Some(shared.clone());

                // Predictive endpointing (Bước 3). Default off → server-side VAD (current behavior).
                // On: load Smart Turn, switch STT to manual commit, spawn the endpoint loop that
                // commits early when the model says the caller is done. If the model can't load,
                // fall back to VAD (never break the call).
                let (commit_tx, commit_rx) = mpsc::channel::<()>(8);
                let mut commit_strategy = "vad".to_string();
                // Reuse the process-wide Smart Turn (loaded once at startup) — no per-call ONNX load.
                // `smart_turn` is Some only when predictive endpointing was on + the model loaded.
                if let Some(st) = &self.smart_turn {
                    commit_strategy = "manual".to_string();
                    let (ep_tx, ep_rx) = mpsc::channel::<(Vec<i16>, bool, u64)>(256);
                    self.endpoint_tx = Some(ep_tx);
                    self.tasks.push(tokio::spawn(endpoint_loop(
                        Arc::clone(st),
                        ep_rx,
                        commit_tx.clone(),
                        self.config.endpoint_short_silence_ms,
                        self.config.endpoint_max_silence_ms,
                    )));
                }

                self.start_stt(
                    shared.clone(),
                    language,
                    lexicon,
                    commit_rx,
                    commit_strategy,
                );

                // DTMF collector: buffers keypad digits and commits them as a turn (so the agent
                // handles both "press 1 for…" menus and spoken/typed codes uniformly). Harmless when
                // the caller never presses a key — no events, loop just idles.
                let (dtmf_tx, dtmf_rx) = mpsc::channel::<char>(16);
                self.dtmf_tx = Some(dtmf_tx);
                self.tasks.push(tokio::spawn(dtmf_loop(
                    shared.clone(),
                    dtmf_rx,
                    self.config.dtmf_interdigit_ms,
                )));

                // Agent greets first (in its own persona) unless disabled.
                if !self.config.greeting_prompt.trim().is_empty() {
                    let greet = shared.clone();
                    let prompt = self.config.greeting_prompt.clone();
                    self.tasks.push(tokio::spawn(
                        async move { do_turn(greet, prompt, true).await },
                    ));
                }
            }
            InboundMessage::Media { media, .. } => {
                let Ok(bytes) = BASE64.decode(media.payload) else {
                    return;
                };
                self.audio_buffer.extend_from_slice(&bytes);
                while self.audio_buffer.len() >= codec::SAMPLES_PER_FRAME {
                    let frame: Vec<u8> = self
                        .audio_buffer
                        .drain(0..codec::SAMPLES_PER_FRAME)
                        .collect();
                    self.process_frame(frame).await;
                }
            }
            InboundMessage::Stop { stream_sid } => {
                tracing::info!("Stopped stream {}", stream_sid);
                if let Some(shared) = &self.shared {
                    stop_playback(shared, false).await;
                }
            }
            InboundMessage::Mark { mark, .. } => {
                if mark.name == "tts_end" {
                    if let Some(shared) = &self.shared {
                        shared.speaking.store(false, Ordering::Relaxed);
                    }
                }
            }
            InboundMessage::Dtmf { dtmf, .. } => {
                // Forward the single keypad digit to the collector loop. `try_send` drops it only
                // if the (16-deep) channel is momentarily full — implausible at human keypress rate.
                if let (Some(tx), Some(digit)) = (&self.dtmf_tx, dtmf.digit.chars().next()) {
                    tracing::info!("DTMF digit: {}", digit);
                    let _ = tx.try_send(digit);
                }
            }
        }
    }

    async fn process_frame(&mut self, frame: Vec<u8>) {
        let pcm = codec::decode_frame(&frame);

        // Pre-Start (no shared yet): forward as-is + keep the VAD warm, then done.
        let Some(shared) = self.shared.clone() else {
            if let Some(tx) = &self.stt_tx {
                let _ = tx.try_send(frame);
            }
            let _ = self.vad.process_frame(&pcm);
            return;
        };

        let state = self.vad.process_frame(&pcm);
        let is_speech = state == VadState::Speech;

        // Forward to STT. When DENOISE_ENABLED, clean background noise off the frame first
        // (re-encode to μ-law); otherwise pass the original frame through untouched. Denoise only
        // attenuates noise — it never mutes — so this path can never produce dead-air.
        if self.stt_tx.is_some() {
            // Compute the outbound frame BEFORE re-borrowing stt_tx (denoiser needs &mut self).
            let out = if let Some(d) = self.denoiser.as_mut() {
                codec::encode_frame(&d.process_8k(&pcm))
            } else {
                frame
            };
            if let Some(tx) = &self.stt_tx {
                let _ = tx.try_send(out);
            }
        }

        // Local VAD: barge-in while the assistant is speaking.
        if shared.speaking.load(Ordering::Relaxed)
            && self.vad.is_barge_in(self.config.barge_in_ms, true)
        {
            tracing::info!("Barge-in detected — interrupting playback");
            stop_playback(&shared, true).await;
            self.vad.reset_counters();
        }

        // Predictive endpointing: hand the frame to the endpoint loop (it runs Smart Turn and
        // commits early). Only present when predictive endpointing is on. `try_send` drops the
        // frame if the loop is briefly busy running inference — harmless.
        if let Some(ep) = &self.endpoint_tx {
            let _ = ep.try_send((pcm, is_speech, self.vad.consecutive_silence_ms()));
        }
    }

    fn start_stt(
        &mut self,
        shared: Shared,
        language: Option<String>,
        lexicon: Option<String>,
        commit_rx: mpsc::Receiver<()>,
        commit_strategy: String,
    ) {
        let (ulaw_tx, ulaw_rx) = mpsc::channel::<Vec<u8>>(256);
        let (transcript_tx, mut transcript_rx) = mpsc::channel::<String>(16);
        self.stt_tx = Some(ulaw_tx);

        let cfg = Arc::clone(&self.config);
        let params = SttParams {
            api_key: cfg.elevenlabs_api_key.clone(),
            model_id: cfg.elevenlabs_stt_model.clone(),
            language_code: language.or(cfg.stt_language_code.clone()),
            vad_silence_secs: (cfg.silence_end_ms as f32) / 1000.0,
            commit_strategy,
            lexicon,
        };
        let stt_shared = shared.clone();
        let cfg = Arc::clone(&self.config);
        self.tasks.push(tokio::spawn(async move {
            // C1: run the primary STT provider, then transparently fail over to the fallback
            // (e.g. ElevenLabs → Deepgram) mid-call. This returns an error ONLY when every
            // configured provider is exhausted — the terminal behavior below is unchanged.
            if let Err(e) = crate::stt_failover::run_stt_with_failover(
                cfg,
                params,
                ulaw_rx,
                transcript_tx,
                commit_rx,
            )
            .await
            {
                tracing::error!("STT stream error (all providers): {:?}", e);
                // Any terminal STT failure — quota outage OR persistent reconnect failures —
                // means the caller can never be transcribed again. Silently dropping the
                // transcript channel leaves them in dead air with no hint that anything went
                // wrong. Always end gracefully: play the fallback clip and close the WS so
                // Twilio hangs up cleanly. `fail_quota` is idempotent and guards against
                // double-invocation when both STT and TTS fail simultaneously.
                fail_quota(&stt_shared, "stt").await;
            }
        }));

        // Consume committed transcripts → drive backend turns (one at a time).
        self.tasks.push(tokio::spawn(async move {
            while let Some(transcript) = transcript_rx.recv().await {
                // Skip empty/whitespace commits so we never run a turn on nothing.
                if transcript.trim().is_empty() {
                    continue;
                }
                tracing::info!("Committed transcript: {}", transcript);
                do_turn(shared.clone(), transcript, false).await;
            }
        }));
    }
}

/// Stop the current assistant response: abort the TTS task, cancel the play loop, mark the
/// assistant as no longer speaking, and (optionally) flush Twilio's playout buffer.
async fn stop_playback(shared: &Shared, send_clear: bool) {
    shared.play_cancel.store(true, Ordering::Relaxed);
    shared.speaking.store(false, Ordering::Relaxed);
    if let Some(handle) = shared.tts_task.lock().await.take() {
        handle.abort();
    }
    if send_clear {
        let clear = OutboundMessage::clear(&shared.stream_sid);
        if let Ok(json) = serde_json::to_string(&clear) {
            let mut tx = shared.ws_tx.lock().await;
            let _ = tx.send(Message::Text(json)).await;
        }
    }
}

/// Predictive endpointing loop (Bước 3): owns the current utterance's PCM, runs Smart Turn when the
/// caller pauses, and signals STT to commit early. Being the sole owner of the buffer avoids any
/// cross-task shared state. Inference runs on `spawn_blocking` so it never stalls audio.
async fn endpoint_loop(
    smart_turn: Arc<SmartTurn>,
    mut rx: mpsc::Receiver<(Vec<i16>, bool, u64)>,
    commit_tx: mpsc::Sender<()>,
    short_ms: u64,
    max_ms: u64,
) {
    const MAX_BUF: usize = 8_000 * 12; // ~12s of 8 kHz PCM; Smart Turn uses only the last 8s
    let mut buf: Vec<i16> = Vec::new();
    let mut in_utt = false;
    let mut checked = false;
    let mut keepalive_frames = 0; // count frames (20ms each) of continuous silence

    while let Some((pcm, is_speech, silence_ms)) = rx.recv().await {
        if is_speech {
            keepalive_frames = 0; // reset keepalive timer
            buf.extend_from_slice(&pcm);
            if buf.len() > MAX_BUF {
                let drop = buf.len() - MAX_BUF;
                buf.drain(0..drop);
            }
            in_utt = true;
            checked = false;
        } else if in_utt {
            keepalive_frames = 0; // reset keepalive timer while processing end of speech
            if !checked && silence_ms >= short_ms {
                // One Smart Turn check per pause. If "complete" → commit now; else keep listening
                // (a mid-sentence pause) until the caller resumes or the max-silence fallback hits.
                checked = true;
                let st = Arc::clone(&smart_turn);
                let snapshot = buf.clone();
                let complete = tokio::task::spawn_blocking(move || st.is_complete(&snapshot))
                    .await
                    .unwrap_or(false);
                if complete {
                    tracing::info!("predictive endpoint: complete → commit");
                    let _ = commit_tx.send(()).await;
                    buf.clear();
                    in_utt = false;
                    checked = false;
                }
            } else if silence_ms >= max_ms {
                tracing::info!("predictive endpoint: max-silence fallback → commit");
                let _ = commit_tx.send(()).await;
                buf.clear();
                in_utt = false;
                checked = false;
            }
        } else {
            // Not in an utterance (pure silence / noise gated out).
            // ElevenLabs STT has a ~36s hard limit for uncommitted segments when using
            // manual commit. If the agent speaks a long sentence, we send 0xFF silence
            // frames continuously. Smart Turn never triggers a commit because `in_utt` is false.
            // ElevenLabs hits the limit and drops the socket. If the user barges in right as
            // it drops, their audio is lost -> dead air.
            // Fix: send an empty commit every 15 seconds (750 frames * 20ms) of silence
            // to flush ElevenLabs' buffer and reset their segment timer.
            keepalive_frames += 1;
            if keepalive_frames >= 750 {
                tracing::debug!("STT keepalive: flushing silence to prevent provider disconnect");
                let _ = commit_tx.send(()).await;
                keepalive_frames = 0;
            }
        }
    }
}

/// DTMF collector: buffers keypad digits and commits them as a caller turn. `#` commits what's
/// buffered immediately; `*` clears the current entry; otherwise digits accumulate until the caller
/// pauses for `interdigit_ms` (then the buffer is committed). Runs for the life of the call; exits
/// when the sender drops (call teardown).
async fn dtmf_loop(shared: Shared, mut rx: mpsc::Receiver<char>, interdigit_ms: u64) {
    use tokio::time::{timeout, Duration};
    const MAX_DIGITS: usize = 32; // guard against a stuck key / runaway entry
    let mut buf = String::new();
    loop {
        let next = if buf.is_empty() {
            // Idle: block until the first digit (no timeout while nothing is buffered).
            rx.recv().await
        } else {
            // Digits pending: a pause longer than the inter-digit window commits the entry.
            match timeout(Duration::from_millis(interdigit_ms), rx.recv()).await {
                Ok(v) => v,
                Err(_) => {
                    dtmf_flush(&shared, &mut buf).await;
                    continue;
                }
            }
        };
        match next {
            Some('#') => dtmf_flush(&shared, &mut buf).await,
            Some('*') => buf.clear(),
            Some(d) => {
                buf.push(d);
                if buf.len() >= MAX_DIGITS {
                    dtmf_flush(&shared, &mut buf).await;
                }
            }
            None => {
                // Channel closed (call ending): flush any trailing digits, then stop.
                dtmf_flush(&shared, &mut buf).await;
                return;
            }
        }
    }
}

/// Commit the buffered DTMF digits as a caller turn, phrased so the agent understands they came from
/// the phone keypad (covers both IVR-style "press 1" and code/PIN entry). No-op on an empty buffer.
async fn dtmf_flush(shared: &Shared, buf: &mut String) {
    if buf.is_empty() {
        return;
    }
    let digits = std::mem::take(buf);
    tracing::info!("DTMF committed: {}", digits);
    do_turn(shared.clone(), format_dtmf_turn(&digits), false).await;
}

/// Phrase collected keypad digits as a caller turn the agent understands (covers both IVR-style
/// "press 1" and code/PIN entry). Digits are spaced so TTS/read-back treats them one at a time.
fn format_dtmf_turn(digits: &str) -> String {
    let spaced: String = digits
        .chars()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    format!("[The caller pressed these keys on their phone keypad: {spaced}]")
}

/// Run one turn: supersede any in-flight response, call the backend, then speak the answer.
/// `is_greeting` suppresses the thinking-filler for the opening greeting turn.
async fn do_turn(shared: Shared, text: String, is_greeting: bool) {
    // A new user utterance (or greeting) supersedes whatever we were saying.
    stop_playback(&shared, true).await;
    shared.play_cancel.store(false, Ordering::Relaxed);
    // NOTE: `speaking` is intentionally NOT set here. It is armed inside run_response only
    // once real audio is flowing (see below). Setting it now would arm barge-in during the
    // multi-second think phase (backend turn), so the caller's trailing words — or line noise
    // — would abort a reply before it ever starts, leaving the agent mute after the greeting.

    let worker = shared.clone();
    let handle = tokio::spawn(async move { run_response(worker, text, is_greeting).await });
    *shared.tts_task.lock().await = Some(handle);
}

/// Map a `/unit` suffix (e.g. "/mo") to spoken form. None = not a known time unit.
fn spoken_period(word: &str) -> Option<&'static str> {
    match word {
        "mo" | "month" | "months" => Some("per month"),
        "yr" | "yrs" | "year" | "years" => Some("per year"),
        "wk" | "week" | "weeks" => Some("per week"),
        "hr" | "hrs" | "hour" | "hours" => Some("per hour"),
        "day" | "days" => Some("per day"),
        "min" | "minute" | "minutes" => Some("per minute"),
        _ => None,
    }
}

/// Deterministic safety net so TTS never trips on raw money/number formatting even when the model
/// ignores the "say it in words" prompt. Turns `$1,999` → `1999 dollars`, strips thousands commas
/// (`1,999` → `1999`), and expands a `/unit` price suffix (`/mo` → ` per month`). Not a full
/// number-to-words pass — it just removes the symbols/formatting that break pronunciation.
fn normalize_numbers_for_tts(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 8);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '$' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] == ' ' {
                j += 1;
            }
            let mut num = String::new();
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == ',') {
                if chars[j].is_ascii_digit() {
                    num.push(chars[j]);
                }
                j += 1;
            }
            if j + 1 < chars.len() && chars[j] == '.' && chars[j + 1].is_ascii_digit() {
                num.push('.');
                j += 1;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    num.push(chars[j]);
                    j += 1;
                }
            }
            if num.is_empty() {
                i += 1; // lone '$' → drop
            } else {
                out.push_str(&num);
                out.push_str(" dollars");
                i = j;
            }
            continue;
        }
        // Thousands comma between digits: "1,999" → "1999".
        if c == ','
            && i > 0
            && i + 1 < chars.len()
            && chars[i - 1].is_ascii_digit()
            && chars[i + 1].is_ascii_digit()
        {
            i += 1;
            continue;
        }
        // "/unit" price suffix → " per unit" (only for known time units; leaves "24/7", "km/h").
        if c == '/' {
            let start = i + 1;
            let mut j = start;
            while j < chars.len() && chars[j].is_ascii_alphabetic() {
                j += 1;
            }
            if j > start {
                let word: String = chars[start..j].iter().collect::<String>().to_lowercase();
                if let Some(rep) = spoken_period(&word) {
                    if !out.ends_with(' ') {
                        out.push(' ');
                    }
                    out.push_str(rep);
                    i = j;
                    continue;
                }
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Calm LLM text for TTS: normalize money/number formatting (see `normalize_numbers_for_tts`), then
/// convert `!`→`.`, drop stray markdown, and collapse runs of sentence punctuation. (ALL-CAPS is
/// handled by the reply prompt to avoid mangling acronyms.)
fn sanitize_for_tts(s: &str) -> String {
    let s = normalize_numbers_for_tts(s);
    let mut out = String::with_capacity(s.len());
    let mut prev_punct = false;
    for ch in s.chars() {
        let c = match ch {
            '!' => '.',
            '*' | '#' | '_' | '`' => ' ',
            other => other,
        };
        let is_punct = matches!(c, '.' | '?' | ',' | ';' | ':' | '…');
        if is_punct && prev_punct {
            continue; // collapse "?!", "...", ".." → a single mark
        }
        prev_punct = is_punct;
        out.push(c);
    }
    out.trim().to_string()
}

/// Drain complete sentences (or an over-long buffer) from `buf` into the TTS text sink so the
/// agent starts speaking sentence-1 while the LLM is still generating sentence-2. Returns false
/// if the sink is closed (barge-in / TTS gone).
async fn flush_sentences(
    buf: &mut String,
    tx: &mpsc::Sender<String>,
    early_words: usize,
    first_done: &mut bool,
) -> bool {
    loop {
        let mut cut: Option<usize> = None;
        for (i, c) in buf.char_indices() {
            if matches!(c, '.' | '!' | '?' | '\n' | '…' | ';') {
                cut = Some(i + c.len_utf8());
                break;
            }
        }
        // Early-feed (A/B via TTS_EARLY_FEED_WORDS): for the FIRST spoken chunk only, cut after
        // `early_words` words even without a terminator, so audio starts sooner. Later chunks stay
        // sentence-based to preserve prosody.
        if cut.is_none() && !*first_done && early_words > 0 {
            let mut words = 0usize;
            let mut in_word = false;
            for (i, c) in buf.char_indices() {
                if c.is_whitespace() {
                    if in_word {
                        words += 1;
                        in_word = false;
                        if words >= early_words {
                            cut = Some(i);
                            break;
                        }
                    }
                } else {
                    in_word = true;
                }
            }
        }
        let piece = if let Some(end) = cut {
            buf.drain(..end).collect::<String>()
        } else if buf.len() > 160 {
            // No terminator yet but long — flush to keep latency low.
            std::mem::take(buf)
        } else {
            return true;
        };
        let trimmed = sanitize_for_tts(piece.trim());
        if !trimmed.is_empty() {
            *first_done = true;
            if tx.send(trimmed).await.is_err() {
                return false;
            }
        }
    }
}

/// Synthesize a single fixed line with the PRIMARY TTS (connect/greeting/error fallbacks).
async fn speak_once(shared: &Shared, voice_id: &str, line: &str) {
    speak_once_with(shared, build_tts(&shared.config), voice_id, line).await;
}

/// Synthesize a single fixed line with a SPECIFIC TTS engine and relay it to Twilio. Lets the
/// zero-audio recovery re-speak through the ElevenLabs fallback when the primary (Cartesia) produced
/// nothing — so a Cartesia outage never leaves the caller in silence.
async fn speak_once_with(shared: &Shared, tts: Box<dyn Tts>, voice_id: &str, line: &str) {
    let (text_tx, text_rx) = mpsc::channel::<String>(1);
    let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(256);
    let _ = text_tx.send(line.to_string()).await;
    drop(text_tx);
    let voice = voice_id.to_string();
    let tts_handle = tokio::spawn(async move {
        let _ = tts.speak(&voice, text_rx, audio_tx).await;
    });
    let mut playing = false;
    while let Some(audio) = audio_rx.recv().await {
        if shared.play_cancel.load(Ordering::Relaxed) {
            break;
        }
        if !playing {
            shared.speaking.store(true, Ordering::Relaxed);
            playing = true;
        }
        let msg = OutboundMessage::media(&shared.stream_sid, BASE64.encode(audio));
        if let Ok(json) = serde_json::to_string(&msg) {
            let mut tx = shared.ws_tx.lock().await;
            if tx.send(Message::Text(json)).await.is_err() {
                break;
            }
        }
    }
    tts_handle.abort();
    shared.speaking.store(false, Ordering::Relaxed);
}

/// ElevenLabs quota outage handler (STT or TTS): idempotently play the fixed fallback clip (if
/// configured) and end the call, instead of leaving the caller in dead air. The first task to trip
/// it wins via the `ended` flag; any concurrent tripper no-ops.
async fn fail_quota(shared: &Shared, source: &str) {
    if shared.ended.swap(true, Ordering::Relaxed) {
        return; // already handled by the other (STT/TTS) side
    }
    tracing::error!("ElevenLabs quota exceeded ({source}) — playing fallback clip, ending call");
    shared.play_cancel.store(true, Ordering::Relaxed);
    let path = shared.config.quota_fallback_audio_path.clone();
    play_ulaw_file(shared, &path).await;
    // Close our side of the Twilio Media Stream → the (bidirectional) call ends cleanly.
    let mut tx = shared.ws_tx.lock().await;
    let _ = tx.send(Message::Close(None)).await;
}

/// Stream a pre-recorded G.711 μ-law 8 kHz (raw, headerless) file to Twilio as 20 ms media frames,
/// then wait out its duration so playback isn't cut off by the socket closing. No-op (with a warn)
/// when the file is missing/empty — the caller still hangs up, which beats dead air.
async fn play_ulaw_file(shared: &Shared, path: &str) {
    if path.trim().is_empty() {
        return;
    }
    let bytes = match tokio::fs::read(path).await {
        Ok(b) if !b.is_empty() => b,
        _ => {
            tracing::warn!("quota fallback clip missing/empty at '{path}'; ending call silently");
            return;
        }
    };
    const FRAME: usize = 160; // 20 ms of μ-law @ 8 kHz
    for chunk in bytes.chunks(FRAME) {
        let msg = OutboundMessage::media(&shared.stream_sid, BASE64.encode(chunk));
        if let Ok(json) = serde_json::to_string(&msg) {
            let mut tx = shared.ws_tx.lock().await;
            if tx.send(Message::Text(json)).await.is_err() {
                return;
            }
        }
    }
    // Twilio buffers what we sent; give it the clip's wall-clock length (+margin) to play out.
    let dur_ms = (bytes.len() as u64 * 1000) / 8000;
    tokio::time::sleep(std::time::Duration::from_millis(dur_ms + 500)).await;
}

/// Streaming turn: open the backend gRPC stream, feed answer tokens into TTS sentence-by-
/// sentence, and relay audio to Twilio. Honors `play_cancel` for prompt barge-in.
async fn run_response(shared: Shared, text: String, is_greeting: bool) {
    let turn_client = TurnClient::new(
        shared.config.orch_grpc_url.clone(),
        shared.config.orch_service_token.clone(),
    );
    let req = TurnRequest {
        tenant_id: shared.tenant_id.clone(),
        agent_id: shared.agent_id.clone(),
        call_id: shared.call_id.clone(),
        text,
        trace_id: uuid::Uuid::new_v4().to_string(),
    };
    tracing::info!(
        "Turn → backend gRPC (agent={}, {} chars)",
        req.agent_id,
        req.text.len()
    );

    let default_voice = shared.config.default_voice_id.clone();
    let fallback = "I'm sorry, I'm having trouble right now. Could you say that again?";

    let mut rx = match turn_client.run_turn_stream(req).await {
        Ok(rx) => rx,
        Err(e) => {
            tracing::error!("Turn stream error: {:?}", e);
            speak_once(&shared, &default_voice, fallback).await;
            return;
        }
    };

    // Resolve the voice from the first event; stash a non-voice first event to feed later.
    let mut voice_id = default_voice.clone();
    let mut pending: Option<TurnEvent> = None;
    if let Some(ev) = rx.recv().await {
        match ev {
            TurnEvent::VoiceId(v) => {
                if !v.is_empty() && v != "eleven_labs_default" {
                    voice_id = v;
                }
            }
            TurnEvent::Error(e) => {
                tracing::error!("Turn error: {}", e);
                speak_once(&shared, &default_voice, fallback).await;
                return;
            }
            other => {
                pending = Some(other);
            }
        }
    }
    tracing::info!("Turn streaming (voice={})", voice_id);

    let tts = build_tts(&shared.config);
    let (text_tx, text_rx) = mpsc::channel::<String>(16);
    let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(256);

    let voice_for_tts = voice_id.clone();
    // Report the TTS outcome back so the zero-audio path below can tell a quota outage (retry is
    // pointless — the whole account is blocked) from a bad per-agent voice (retry with default).
    let (tts_err_tx, tts_err_rx) = oneshot::channel::<Option<MediaError>>();
    let tts_handle = tokio::spawn(async move {
        let outcome = match tts.speak(&voice_for_tts, text_rx, audio_tx).await {
            Ok(()) => None,
            Err(e) => {
                tracing::error!("TTS error: {:?}", e);
                Some(e)
            }
        };
        let _ = tts_err_tx.send(outcome);
    });

    // Instant-ack (#5): on a real caller turn, speak a short filler in the agent's own voice
    // IMMEDIATELY so the caller hears something while the backend is still thinking. Skipped on the
    // greeting. Sent straight into the TTS sink (not through the feeder), so it never lands in the
    // accumulated answer/transcript. Empty MEDIA_THINKING_FILLER = disabled.
    if !is_greeting {
        let filler = shared.config.thinking_filler.trim().to_string();
        if !filler.is_empty() {
            let _ = text_tx.send(filler).await;
        }
    }

    // Feeder: gRPC token deltas → sentence buffer → text_tx. Dropping text_tx at the end signals
    // end-of-speech to TTS. If run_response is aborted (barge-in), audio_rx drops → TTS ends →
    // text_rx drops → this feeder's send fails → it stops; rx drops → the gRPC pump stops.
    let early_words = shared.config.tts_early_feed_words;
    // The feeder also accumulates the COMPLETE answer and hands it back on `answer_tx`, so if the
    // per-agent voice produces no audio (e.g. a stale/deleted voice_id → voice_id_does_not_exist),
    // run_response can re-speak the full answer with the fallback voice instead of going silent.
    let (answer_tx, answer_rx) = oneshot::channel::<String>();
    let feeder = tokio::spawn(async move {
        let mut buf = String::new();
        let mut full = String::new();
        let mut first_done = false;
        // Keep draining the gRPC stream to complete `full` even after the TTS sink closes.
        let mut tts_open = true;
        if let Some(TurnEvent::Delta(d)) = pending {
            full.push_str(&d);
            buf.push_str(&d);
        }
        if tts_open && !flush_sentences(&mut buf, &text_tx, early_words, &mut first_done).await {
            tts_open = false;
        }
        while let Some(ev) = rx.recv().await {
            match ev {
                TurnEvent::Delta(d) => {
                    full.push_str(&d);
                    if tts_open {
                        buf.push_str(&d);
                        if !flush_sentences(&mut buf, &text_tx, early_words, &mut first_done).await
                        {
                            tts_open = false;
                        }
                    }
                }
                TurnEvent::Done { .. } | TurnEvent::Error(_) => break,
                TurnEvent::VoiceId(_) => {}
            }
        }
        if tts_open {
            let rest = buf.trim().to_string();
            if !rest.is_empty() {
                let _ = text_tx.send(rest).await;
            }
        }
        let _ = answer_tx.send(full.trim().to_string());
    });

    // Relay audio; arm barge-in only once real audio flows.
    let mut playing = false;
    let mut chunks: u32 = 0;
    while let Some(audio) = audio_rx.recv().await {
        if shared.play_cancel.load(Ordering::Relaxed) {
            break;
        }
        if !playing {
            shared.speaking.store(true, Ordering::Relaxed);
            playing = true;
        }
        chunks += 1;
        let msg = OutboundMessage::media(&shared.stream_sid, BASE64.encode(audio));
        if let Ok(json) = serde_json::to_string(&msg) {
            let mut tx = shared.ws_tx.lock().await;
            if tx.send(Message::Text(json)).await.is_err() {
                break;
            }
        }
    }
    tracing::info!(
        "Playback done: {} audio chunks sent, cancelled={}",
        chunks,
        shared.play_cancel.load(Ordering::Relaxed)
    );

    // Silence guard: the per-agent voice produced NO audio and the caller didn't barge in. Two
    // distinct causes, handled differently:
    //   1. ElevenLabs quota outage (account-level) → re-speaking with ANY voice also fails, so play
    //      the fixed fallback clip and end the call gracefully instead of leaving dead air.
    //   2. A stale/deleted persona voice_id (voice_id_does_not_exist) → re-speak the full answer
    //      with the known-good default voice.
    if chunks == 0 && !shared.play_cancel.load(Ordering::Relaxed) {
        let quota = matches!(tts_err_rx.await, Ok(Some(ref e)) if e.is_quota());
        if quota {
            fail_quota(&shared, "tts").await;
        } else if let Ok(answer) = answer_rx.await {
            // Primary TTS produced no audio (a Cartesia outage/bad voice, or a stale ElevenLabs
            // voice_id). Re-speak the full answer through the ElevenLabs FALLBACK engine with the
            // known-good default voice, so the caller is never left in silence. (When ElevenLabs is
            // already the primary, this is the same recover-with-default-voice behavior as before.)
            let answer = answer.trim().to_string();
            if !answer.is_empty() {
                tracing::warn!(
                    "Primary TTS voice '{}' produced no audio; retrying via ElevenLabs fallback",
                    voice_id
                );
                let fallback = Box::new(build_fallback_tts(&shared.config));
                // The fallback is ElevenLabs, so it needs an ElevenLabs voice — NOT default_voice,
                // which is a Cartesia ID under TTS_PROVIDER=cartesia. Fall back to default_voice only
                // when no dedicated EL fallback voice is configured.
                let el_voice = if shared.config.elevenlabs_fallback_voice_id.trim().is_empty() {
                    default_voice.clone()
                } else {
                    shared.config.elevenlabs_fallback_voice_id.clone()
                };
                speak_once_with(&shared, fallback, &el_voice, &answer).await;
            }
        }
    }

    if !shared.play_cancel.load(Ordering::Relaxed) {
        let mark = OutboundMessage::mark(&shared.stream_sid, "tts_end");
        if let Ok(json) = serde_json::to_string(&mark) {
            let mut tx = shared.ws_tx.lock().await;
            let _ = tx.send(Message::Text(json)).await;
        }
    }

    feeder.abort();
    tts_handle.abort();
    shared.speaking.store(false, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::{format_dtmf_turn, normalize_numbers_for_tts, sanitize_for_tts};

    #[test]
    fn dtmf_turn_spaces_digits_for_readback() {
        let out = format_dtmf_turn("1234");
        assert!(out.contains("1 2 3 4"), "digits should be spaced: {out}");
        assert!(out.to_lowercase().contains("keypad"));
    }

    #[test]
    fn dtmf_turn_single_digit_menu_choice() {
        assert!(format_dtmf_turn("2").contains(": 2]"));
    }

    #[test]
    fn normalizes_currency_and_period() {
        assert_eq!(
            normalize_numbers_for_tts("It's $1,999/mo for Scale."),
            "It's 1999 dollars per month for Scale."
        );
        assert_eq!(
            normalize_numbers_for_tts("$149 per seat"),
            "149 dollars per seat"
        );
        assert_eq!(normalize_numbers_for_tts("$1,234.50"), "1234.50 dollars");
    }

    #[test]
    fn strips_thousands_comma_but_keeps_other_slashes() {
        assert_eq!(
            normalize_numbers_for_tts("we handled 12,500 calls"),
            "we handled 12500 calls"
        );
        // Not a price/time unit → left intact.
        assert_eq!(normalize_numbers_for_tts("open 24/7"), "open 24/7");
        assert_eq!(normalize_numbers_for_tts("60 km/h"), "60 km/h");
    }

    #[test]
    fn sanitize_applies_number_normalization() {
        let out = sanitize_for_tts("The Scale plan is $1,999/mo!");
        assert!(out.contains("1999 dollars per month"), "got: {out}");
        assert!(!out.contains('$') && !out.contains('!'));
    }
}
