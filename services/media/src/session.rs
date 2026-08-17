use crate::{
    codec,
    config::Config,
    error::MediaError,
    stt::{ElevenLabsStt, Stt},
    tts::{ElevenLabsTts, Tts},
    turn::{TurnClient, TurnRequest},
    twilio::{InboundMessage, OutboundMessage},
    vad::{VadEngine, VadState},
};
use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

pub struct Session {
    config: Arc<Config>,
    vad: VadEngine,
    stream_sid: Option<String>,
    call_sid: Option<String>,
    tenant_id: Option<String>,
    agent_id: Option<String>,

    audio_buffer: Vec<u8>,
    stt_tx: Option<mpsc::Sender<Vec<u8>>>,
    transcript_rx: Option<mpsc::Receiver<String>>,

    tts_task: Option<tokio::task::JoinHandle<()>>,
    is_assistant_speaking: Arc<RwLock<bool>>,
}

impl Session {
    pub fn new(config: Arc<Config>) -> Self {
        let vad = VadEngine::new(config.vad_aggressiveness);
        Self {
            config,
            vad,
            stream_sid: None,
            call_sid: None,
            tenant_id: None,
            agent_id: None,
            audio_buffer: Vec::with_capacity(codec::SAMPLES_PER_FRAME * 2),
            stt_tx: None,
            transcript_rx: None,
            tts_task: None,
            is_assistant_speaking: Arc::new(RwLock::new(false)),
        }
    }

    pub async fn run(mut self, socket: WebSocket) {
        let (ws_tx, mut ws_rx) = socket.split();
        let ws_tx = Arc::new(tokio::sync::Mutex::new(ws_tx));

        while let Some(msg) = ws_rx.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(inbound) = serde_json::from_str::<InboundMessage>(&text) {
                        if let Err(e) = self.handle_inbound(inbound, Arc::clone(&ws_tx)).await {
                            tracing::error!("Error handling inbound message: {:?}", e);
                        }
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

        // Cleanup
        if let Some(task) = self.tts_task.take() {
            task.abort();
        }
    }

    async fn handle_inbound(
        &mut self,
        msg: InboundMessage,
        ws_tx: Arc<tokio::sync::Mutex<futures_util::stream::SplitSink<WebSocket, Message>>>,
    ) -> Result<(), MediaError> {
        match msg {
            InboundMessage::Connected { protocol, version } => {
                tracing::info!("Connected: protocol={}, version={}", protocol, version);
            }
            InboundMessage::Start {
                stream_sid,
                call_sid,
                custom_parameters,
            } => {
                self.stream_sid = Some(stream_sid.clone());
                self.call_sid = Some(call_sid.clone());

                if let Some(params) = custom_parameters {
                    self.tenant_id = params.get("tenant_id").cloned();
                    self.agent_id = params.get("agent_id").cloned();
                }

                tracing::info!(
                    "Started stream {} for call {} (tenant: {:?}, agent: {:?})",
                    stream_sid,
                    call_sid,
                    self.tenant_id,
                    self.agent_id
                );

                self.start_stt();

                // Trigger initial greeting logic if needed, or wait for user.
                // Assuming the backend handles initial greeting via HTTP hooks,
                // but we can trigger a turn here with an empty string or "greeting" event
                // if we want the media plane to initiate it. For now, we wait for VAD.
            }
            InboundMessage::Media { media, .. } => {
                if let Ok(bytes) = BASE64.decode(media.payload) {
                    self.audio_buffer.extend_from_slice(&bytes);

                    while self.audio_buffer.len() >= codec::SAMPLES_PER_FRAME {
                        let frame: Vec<u8> = self
                            .audio_buffer
                            .drain(0..codec::SAMPLES_PER_FRAME)
                            .collect();
                        self.process_audio_frame(frame, Arc::clone(&ws_tx)).await?;
                    }
                }
            }
            InboundMessage::Stop { stream_sid } => {
                tracing::info!("Stopped stream {}", stream_sid);
                if let Some(task) = self.tts_task.take() {
                    task.abort();
                }
            }
            InboundMessage::Mark { mark, .. } => {
                tracing::debug!("Mark received: {}", mark.name);
                // Can be used to sync TTS playback completion
                if mark.name == "tts_end" {
                    let mut speaking = self.is_assistant_speaking.write().await;
                    *speaking = false;
                }
            }
        }
        Ok(())
    }

    fn start_stt(&mut self) {
        let (ulaw_tx, ulaw_rx) = mpsc::channel(100);
        let (transcript_tx, transcript_rx) = mpsc::channel(10);

        self.stt_tx = Some(ulaw_tx);
        self.transcript_rx = Some(transcript_rx);

        let stt = ElevenLabsStt::new(
            self.config.elevenlabs_api_key.clone(),
            self.config.elevenlabs_stt_model.clone(),
        );

        tokio::spawn(async move {
            if let Err(e) = stt.stream(ulaw_rx, transcript_tx).await {
                tracing::error!("STT stream error: {:?}", e);
            }
        });
    }

    async fn process_audio_frame(
        &mut self,
        frame: Vec<u8>,
        ws_tx: Arc<tokio::sync::Mutex<futures_util::stream::SplitSink<WebSocket, Message>>>,
    ) -> Result<(), MediaError> {
        let pcm = codec::decode_frame(&frame);
        let vad_state = self.vad.process_frame(&pcm);

        let is_speaking = *self.is_assistant_speaking.read().await;

        if vad_state == VadState::Speech {
            // Forward to STT
            if let Some(stt_tx) = &self.stt_tx {
                let _ = stt_tx.send(frame).await;
            }

            // Check barge-in
            if self.vad.is_barge_in(self.config.barge_in_ms, is_speaking) {
                tracing::info!("Barge-in detected!");
                self.handle_barge_in(Arc::clone(&ws_tx)).await?;
            }
        }

        // Check utterance end
        if self.vad.is_utterance_end(self.config.silence_end_ms)
            && (self.vad.consecutive_speech_ms() > 0
                || self.vad.consecutive_silence_ms() == self.config.silence_end_ms)
        {
            // We reached the silence threshold after some speech, or just triggered it
            // We should pull the transcript and send to the backend.
            if let Some(rx) = &mut self.transcript_rx {
                if let Ok(transcript) = rx.try_recv() {
                    if !transcript.trim().is_empty() {
                        tracing::info!("Utterance ended. Transcript: {}", transcript);
                        self.handle_turn(transcript, Arc::clone(&ws_tx)).await?;
                        self.vad.reset_counters();
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle_barge_in(
        &mut self,
        ws_tx: Arc<tokio::sync::Mutex<futures_util::stream::SplitSink<WebSocket, Message>>>,
    ) -> Result<(), MediaError> {
        if let Some(task) = self.tts_task.take() {
            task.abort();
        }

        {
            let mut speaking = self.is_assistant_speaking.write().await;
            *speaking = false;
        }

        if let Some(sid) = &self.stream_sid {
            let clear_msg = OutboundMessage::clear(sid);
            if let Ok(json) = serde_json::to_string(&clear_msg) {
                let mut tx = ws_tx.lock().await;
                let _ = tx.send(Message::Text(json)).await;
            }
        }

        // Restart STT to discard buffered partials
        self.start_stt();

        Ok(())
    }

    async fn handle_turn(
        &mut self,
        transcript: String,
        ws_tx: Arc<tokio::sync::Mutex<futures_util::stream::SplitSink<WebSocket, Message>>>,
    ) -> Result<(), MediaError> {
        let tenant_id = self.tenant_id.clone().unwrap_or_default();
        let agent_id = self.agent_id.clone().unwrap_or_default();
        let stream_sid = self.stream_sid.clone().unwrap_or_default();

        let turn_client = TurnClient::new(
            self.config.orch_base_url.clone(),
            self.config.orch_service_token.clone(),
        );

        let req = TurnRequest {
            tenant_id,
            agent_id,
            text: transcript,
            trace_id: uuid::Uuid::new_v4().to_string(),
        };

        // Mark assistant as speaking
        {
            let mut speaking = self.is_assistant_speaking.write().await;
            *speaking = true;
        }

        let config = Arc::clone(&self.config);
        let speaking_flag = Arc::clone(&self.is_assistant_speaking);

        let task = tokio::spawn(async move {
            let answer = match turn_client.run_turn(&req).await {
                Ok(resp) => resp.answer,
                Err(e) => {
                    tracing::error!("Turn error: {:?}", e);
                    "I'm sorry, I'm having trouble connecting to my brain.".to_string()
                }
            };

            let tts = ElevenLabsTts::new(
                config.elevenlabs_api_key.clone(),
                config.elevenlabs_tts_model.clone(),
            );

            let (text_tx, text_rx) = mpsc::channel(1);
            let (audio_tx, mut audio_rx) = mpsc::channel(100);

            // Send text to TTS
            let _ = text_tx.send(answer).await;
            drop(text_tx);

            // Play audio back to Twilio
            let ws_tx_clone = Arc::clone(&ws_tx);
            let stream_sid_clone = stream_sid.clone();

            let play_task = tokio::spawn(async move {
                let mut tx = ws_tx_clone.lock().await;
                while let Some(audio) = audio_rx.recv().await {
                    let base64_audio = BASE64.encode(audio);
                    let msg = OutboundMessage::media(&stream_sid_clone, base64_audio);
                    if let Ok(json) = serde_json::to_string(&msg) {
                        if tx.send(Message::Text(json)).await.is_err() {
                            break;
                        }
                    }
                }

                // Send mark to indicate TTS is done
                let mark_msg = OutboundMessage::mark(&stream_sid_clone, "tts_end");
                if let Ok(json) = serde_json::to_string(&mark_msg) {
                    let _ = tx.send(Message::Text(json)).await;
                }
            });

            // Voice ID could be pulled from Agent config. Hardcoding a default for now.
            let default_voice = "21m00Tcm4TlvDq8ikWAM".to_string();
            if let Err(e) = tts.speak(&default_voice, text_rx, audio_tx).await {
                tracing::error!("TTS Error: {:?}", e);
                let mut speaking = speaking_flag.write().await;
                *speaking = false;
            }

            let _ = play_task.await;
        });

        self.tts_task = Some(task);
        Ok(())
    }
}
