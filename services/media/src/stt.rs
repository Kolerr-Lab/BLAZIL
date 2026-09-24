//! Real-time speech-to-text via ElevenLabs Scribe v2 Realtime.
//!
//! WebSocket handshake: `wss://api.elevenlabs.io/v1/speech-to-text/realtime`.
//! We stream Twilio's native G.711 μ-law 8 kHz straight through (`audio_format=ulaw_8000`),
//! so no transcoding is needed on the media plane. With `commit_strategy=vad` ElevenLabs
//! segments speech server-side and emits one `committed_transcript` per utterance — that is
//! the signal the session loop turns into a backend turn, which removes any local
//! "utterance-end + try_recv" race.
//!
//! Auth: server-side uses the `xi-api-key` header (never a single-use token).

use crate::error::MediaError;
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, http::HeaderValue, Message},
};

/// Parameters that shape the realtime STT session.
#[derive(Clone)]
pub struct SttParams {
    pub api_key: String,
    pub model_id: String,
    /// ISO-639 code to bias recognition; `None` = auto-detect.
    pub language_code: Option<String>,
    /// Silence (seconds) that VAD treats as end-of-utterance before committing (VAD strategy only).
    pub vad_silence_secs: f32,
    /// "vad" = server segments on silence (default). "manual" = the caller decides when to commit
    /// (predictive endpointing sends an explicit commit signal). See Stt::stream `commit_rx`.
    pub commit_strategy: String,
    /// Custom vocabulary / keywords to bias STT accuracy (Lexicon Prompting).
    pub lexicon: Option<String>,
}

#[async_trait]
pub trait Stt: Send + Sync {
    /// Consume μ-law 8 kHz frames from `ulaw_rx`, push committed transcripts to `transcript_tx`.
    /// A `()` on `commit_rx` sends an explicit commit to the server (manual-commit / predictive
    /// endpointing). In VAD mode nothing is sent on `commit_rx` and the server segments on silence.
    async fn stream(
        &self,
        ulaw_rx: mpsc::Receiver<Vec<u8>>,
        transcript_tx: mpsc::Sender<String>,
        commit_rx: mpsc::Receiver<()>,
    ) -> Result<(), MediaError>;
}

/// One inbound event from the Scribe realtime socket. Only the fields we act on are typed;
/// the discriminator is `message_type`.
#[derive(Debug, Deserialize)]
struct SttEvent {
    message_type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

pub struct ElevenLabsStt {
    params: SttParams,
}

impl ElevenLabsStt {
    pub fn new(params: SttParams) -> Self {
        Self { params }
    }

    fn ws_url(&self) -> String {
        // inactivity_timeout=180 (the documented max): ElevenLabs closes a realtime socket after
        // its inactivity window (default 20s). Continuous Twilio frames normally count as activity,
        // but we set the ceiling to the max so the socket is never idle-closed prematurely. A
        // mid-call close from any cause is still handled by the failover supervisor's reconnect.
        let mut url = format!(
            "wss://api.elevenlabs.io/v1/speech-to-text/realtime\
             ?model_id={}&audio_format=ulaw_8000&commit_strategy={}&inactivity_timeout=180",
            self.params.model_id, self.params.commit_strategy
        );
        // The silence threshold only applies to server-side VAD segmentation.
        if self.params.commit_strategy == "vad" {
            url.push_str(&format!(
                "&vad_silence_threshold_secs={}",
                self.params.vad_silence_secs
            ));
        }
        if let Some(lang) = &self.params.language_code {
            if !lang.is_empty() {
                url.push_str(&format!("&language_code={lang}"));
            }
        }
        if let Some(lexicon) = &self.params.lexicon {
            if !lexicon.is_empty() {
                // ElevenLabs supports 'prompt' to bias the STT model
                let encoded = lexicon.replace(" ", "%20");
                url.push_str(&format!("&prompt={encoded}"));
            }
        }
        url
    }
}

#[async_trait]
impl Stt for ElevenLabsStt {
    async fn stream(
        &self,
        mut ulaw_rx: mpsc::Receiver<Vec<u8>>,
        transcript_tx: mpsc::Sender<String>,
        mut commit_rx: mpsc::Receiver<()>,
    ) -> Result<(), MediaError> {
        let url = self.ws_url();
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|e| MediaError::SttError(format!("Bad STT URL: {e}")))?;
        request.headers_mut().insert(
            "xi-api-key",
            HeaderValue::from_str(&self.params.api_key)
                .map_err(|e| MediaError::SttError(format!("Bad API key header: {e}")))?,
        );

        let (ws_stream, _) = connect_async(request)
            .await
            .map_err(|e| MediaError::SttError(format!("STT connect failed: {e}")))?;
        let (mut write, mut read) = ws_stream.split();

        // Writer task: forward μ-law audio chunks, and on a commit signal send an explicit commit
        // (manual-commit / predictive endpointing). Audio-channel close ends the task; the commit
        // branch self-disables if its channel closes (no busy-loop).
        let audio_sender = tokio::spawn(async move {
            let mut commit_open = true;
            loop {
                tokio::select! {
                    maybe_audio = ulaw_rx.recv() => {
                        let Some(ulaw) = maybe_audio else { break };
                        let payload = serde_json::json!({
                            "message_type": "input_audio_chunk",
                            "audio_base_64": BASE64.encode(&ulaw),
                        });
                        if write.send(Message::Text(payload.to_string())).await.is_err() {
                            break;
                        }
                    }
                    maybe_commit = commit_rx.recv(), if commit_open => {
                        match maybe_commit {
                            Some(()) => {
                                // Scribe Realtime STT manual commit format: an empty audio chunk with commit: true
                                let payload = serde_json::json!({
                                    "message_type": "input_audio_chunk",
                                    "audio_base_64": "",
                                    "commit": true
                                });
                                if write.send(Message::Text(payload.to_string())).await.is_err() {
                                    break;
                                }
                            }
                            None => commit_open = false,
                        }
                    }
                }
            }
        });

        // Reader loop: surface committed (final, immutable) transcripts to the session.
        // Capture WHY the socket ended so the mid-call close error carries ElevenLabs' real reason
        // (close code + text) instead of a generic string — this is what tells us apart a session
        // cap vs a rejected param vs a rate/quota close when diagnosing frequent reconnects.
        let mut close_reason = "read stream ended (no close frame)".to_string();
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let Ok(event) = serde_json::from_str::<SttEvent>(&text) else {
                        continue;
                    };
                    match event.message_type.as_str() {
                        "committed_transcript" => {
                            if let Some(t) = event.text {
                                if !t.trim().is_empty() && transcript_tx.send(t).await.is_err() {
                                    break; // session dropped the receiver
                                }
                            }
                        }
                        // partial_transcript / final_transcript / session_started → ignore:
                        // committed_transcript is the authoritative per-utterance result.
                        mt if mt.contains("error")
                            || mt == "rate_limited"
                            || mt.contains("exceeded")
                            || mt.contains("exhausted") =>
                        {
                            let reason = event.error.unwrap_or_else(|| mt.to_string());
                            audio_sender.abort();
                            return Err(MediaError::SttError(reason));
                        }
                        _ => {}
                    }
                }
                Ok(Message::Close(frame)) => {
                    // Log ElevenLabs' actual close code + reason (e.g. 1000/1011/1013 + text) so a
                    // premature/frequent close is diagnosable instead of opaque.
                    close_reason = frame
                        .map(|f| format!("{f:?}"))
                        .unwrap_or_else(|| "close frame with no payload".to_string());
                    break;
                }
                Err(e) => {
                    audio_sender.abort();
                    return Err(MediaError::SttError(format!("ws read error: {e}")));
                }
                _ => {}
            }
        }

        audio_sender.abort();
        // The read stream ended (vendor Close or stream end) WITHOUT the session tearing us down.
        // While the call is still live this must be treated as a recoverable mid-call close, not a
        // clean finish — otherwise the failover supervisor reads `Ok(())` as "call over" and the
        // agent goes silent for the rest of the call. Returning Err makes the supervisor reconnect.
        // On a genuine call-end the supervisor has already detected upstream-closed and ignores this
        // result, so returning Err here is harmless in that path. The captured close reason rides
        // along so the supervisor's "reconnecting" warn shows exactly why ElevenLabs dropped us.
        Err(MediaError::SttError(format!(
            "elevenlabs stream closed mid-call ({close_reason})"
        )))
    }
}
