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
    stt::{ElevenLabsStt, Stt, SttParams},
    tts::{ElevenLabsTts, Tts},
    turn::{TurnClient, TurnRequest},
    twilio::{InboundMessage, OutboundMessage},
    vad::VadEngine,
};
use axum::extract::ws::{Message, WebSocket};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::{sync::mpsc, sync::Mutex, task::JoinHandle};

type WsSink = SplitSink<WebSocket, Message>;

/// Shared, cloneable per-call state used by the spawned transcript/turn/playback tasks.
#[derive(Clone)]
struct Shared {
    config: Arc<Config>,
    ws_tx: Arc<Mutex<WsSink>>,
    stream_sid: String,
    tenant_id: String,
    agent_id: String,
    /// True while the assistant is producing/playing audio (gates barge-in).
    speaking: Arc<AtomicBool>,
    /// Set true to stop the current playback loop mid-stream (barge-in / supersede).
    play_cancel: Arc<AtomicBool>,
    /// Handle to the in-flight TTS+playback task so it can be aborted.
    tts_task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

pub struct Session {
    config: Arc<Config>,
    vad: VadEngine,
    audio_buffer: Vec<u8>,
    stt_tx: Option<mpsc::Sender<Vec<u8>>>,
    shared: Option<Shared>,
    tasks: Vec<JoinHandle<()>>,
}

impl Session {
    pub fn new(config: Arc<Config>) -> Self {
        let vad = VadEngine::new(config.vad_aggressiveness);
        Self {
            config,
            vad,
            audio_buffer: Vec::with_capacity(codec::SAMPLES_PER_FRAME * 4),
            stt_tx: None,
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
                    tracing::error!("WS read error: {:?}", e);
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
                // Per-agent language from the TwiML <Parameter>; empty → auto-detect (falls
                // back to the media plane's global STT_LANGUAGE_CODE / language detection).
                let language = params.get("language").filter(|s| !s.is_empty()).cloned();
                tracing::info!(
                    "Started stream {} for call {} (tenant={}, agent={}, language={})",
                    stream_sid,
                    start.call_sid,
                    tenant_id,
                    agent_id,
                    language.as_deref().unwrap_or("auto")
                );

                let shared = Shared {
                    config: Arc::clone(&self.config),
                    ws_tx: Arc::clone(ws_tx),
                    stream_sid,
                    tenant_id,
                    agent_id,
                    speaking: Arc::new(AtomicBool::new(false)),
                    play_cancel: Arc::new(AtomicBool::new(false)),
                    tts_task: Arc::new(Mutex::new(None)),
                };
                self.shared = Some(shared.clone());

                self.start_stt(shared.clone(), language);

                // Agent greets first (in its own persona) unless disabled.
                if !self.config.greeting_prompt.trim().is_empty() {
                    let greet = shared.clone();
                    let prompt = self.config.greeting_prompt.clone();
                    self.tasks
                        .push(tokio::spawn(async move { do_turn(greet, prompt).await }));
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
        }
    }

    async fn process_frame(&mut self, frame: Vec<u8>) {
        // Forward every inbound frame to STT; its server-side VAD decides utterance boundaries.
        if let Some(tx) = &self.stt_tx {
            let _ = tx.try_send(frame.clone());
        }

        // Clone the (Arc-backed, cheap) shared handle so we can freely borrow self.vad below.
        let Some(shared) = self.shared.clone() else {
            let pcm = codec::decode_frame(&frame);
            let _ = self.vad.process_frame(&pcm);
            return;
        };

        // Local VAD is used ONLY to detect barge-in while the assistant is speaking.
        let pcm = codec::decode_frame(&frame);
        let _ = self.vad.process_frame(&pcm);
        if shared.speaking.load(Ordering::Relaxed)
            && self.vad.is_barge_in(self.config.barge_in_ms, true)
        {
            tracing::info!("Barge-in detected — interrupting playback");
            stop_playback(&shared, true).await;
            self.vad.reset_counters();
        }
    }

    fn start_stt(&mut self, shared: Shared, language: Option<String>) {
        let (ulaw_tx, ulaw_rx) = mpsc::channel::<Vec<u8>>(256);
        let (transcript_tx, mut transcript_rx) = mpsc::channel::<String>(16);
        self.stt_tx = Some(ulaw_tx);

        // Per-call language (from the agent) wins; otherwise the global default / auto-detect.
        let language_code = language.or_else(|| self.config.stt_language_code.clone());
        let params = SttParams {
            api_key: self.config.elevenlabs_api_key.clone(),
            model_id: self.config.elevenlabs_stt_model.clone(),
            language_code,
            vad_silence_secs: self.config.silence_end_ms as f32 / 1000.0,
        };
        self.tasks.push(tokio::spawn(async move {
            let stt = ElevenLabsStt::new(params);
            if let Err(e) = stt.stream(ulaw_rx, transcript_tx).await {
                tracing::error!("STT stream error: {:?}", e);
            }
        }));

        // Consume committed transcripts → drive backend turns (one at a time).
        self.tasks.push(tokio::spawn(async move {
            while let Some(transcript) = transcript_rx.recv().await {
                tracing::info!("Committed transcript: {}", transcript);
                do_turn(shared.clone(), transcript).await;
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

/// Run one turn: supersede any in-flight response, call the backend, then speak the answer.
async fn do_turn(shared: Shared, text: String) {
    // A new user utterance (or greeting) supersedes whatever we were saying.
    stop_playback(&shared, true).await;
    shared.play_cancel.store(false, Ordering::Relaxed);
    // NOTE: `speaking` is intentionally NOT set here. It is armed inside run_response only
    // once real audio is flowing (see below). Setting it now would arm barge-in during the
    // multi-second think phase (backend turn), so the caller's trailing words — or line noise
    // — would abort a reply before it ever starts, leaving the agent mute after the greeting.

    let worker = shared.clone();
    let handle = tokio::spawn(async move { run_response(worker, text).await });
    *shared.tts_task.lock().await = Some(handle);
}

/// Call the backend for an answer, synthesize it, and relay μ-law audio to Twilio. Honors
/// `play_cancel` between chunks so barge-in interrupts promptly.
async fn run_response(shared: Shared, text: String) {
    let turn_client = TurnClient::new(
        shared.config.orch_base_url.clone(),
        shared.config.orch_service_token.clone(),
    );
    let req = TurnRequest {
        tenant_id: shared.tenant_id.clone(),
        agent_id: shared.agent_id.clone(),
        text,
        trace_id: uuid::Uuid::new_v4().to_string(),
    };

    let (answer, voice_id) = match turn_client.run_turn(&req).await {
        Ok(resp) => {
            // Ignore empty/placeholder voices (e.g. the "eleven_labs_default" sentinel the
            // dashboard stores before a real ElevenLabs voice is chosen) → use the default.
            let voice = resp
                .voice_id
                .filter(|v| !v.is_empty() && v != "eleven_labs_default")
                .unwrap_or_else(|| shared.config.default_voice_id.clone());
            (resp.answer, voice)
        }
        Err(e) => {
            tracing::error!("Turn error: {:?}", e);
            (
                "I'm sorry, I'm having trouble right now. Could you say that again?".to_string(),
                shared.config.default_voice_id.clone(),
            )
        }
    };

    let tts = ElevenLabsTts::new(
        shared.config.elevenlabs_api_key.clone(),
        shared.config.elevenlabs_tts_model.clone(),
    );
    let (text_tx, text_rx) = mpsc::channel::<String>(1);
    let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(256);

    let _ = text_tx.send(answer).await;
    drop(text_tx);

    let tts_handle = tokio::spawn(async move {
        if let Err(e) = tts.speak(&voice_id, text_rx, audio_tx).await {
            tracing::error!("TTS error: {:?}", e);
        }
    });

    // Relay audio chunk-by-chunk; lock the sink per send so barge-in can slip in a `clear`.
    let mut playing = false;
    while let Some(audio) = audio_rx.recv().await {
        if shared.play_cancel.load(Ordering::Relaxed) {
            break;
        }
        // Arm barge-in only once the first real audio chunk goes out — never during the
        // think phase — so the agent's reply can't be cancelled before it has spoken a word.
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

    // If we finished naturally (not barged-in), tell Twilio playback is complete.
    if !shared.play_cancel.load(Ordering::Relaxed) {
        let mark = OutboundMessage::mark(&shared.stream_sid, "tts_end");
        if let Ok(json) = serde_json::to_string(&mark) {
            let mut tx = shared.ws_tx.lock().await;
            let _ = tx.send(Message::Text(json)).await;
        }
    }

    tts_handle.abort();
    shared.speaking.store(false, Ordering::Relaxed);
}
