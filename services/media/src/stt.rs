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
    /// Silence (seconds) that VAD treats as end-of-utterance before committing.
    pub vad_silence_secs: f32,
}

#[async_trait]
pub trait Stt: Send + Sync {
    /// Consume μ-law 8 kHz frames from `ulaw_rx`, push committed transcripts to `transcript_tx`.
    async fn stream(
        &self,
        ulaw_rx: mpsc::Receiver<Vec<u8>>,
        transcript_tx: mpsc::Sender<String>,
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
        let mut url = format!(
            "wss://api.elevenlabs.io/v1/speech-to-text/realtime\
             ?model_id={}&audio_format=ulaw_8000&commit_strategy=vad\
             &vad_silence_threshold_secs={}",
            self.params.model_id, self.params.vad_silence_secs
        );
        if let Some(lang) = &self.params.language_code {
            if !lang.is_empty() {
                url.push_str(&format!("&language_code={lang}"));
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

        // Writer task: forward μ-law audio chunks as base64 input_audio_chunk messages.
        let audio_sender = tokio::spawn(async move {
            while let Some(ulaw) = ulaw_rx.recv().await {
                let payload = serde_json::json!({
                    "message_type": "input_audio_chunk",
                    "audio_base_64": BASE64.encode(&ulaw),
                });
                if write
                    .send(Message::Text(payload.to_string()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        // Reader loop: surface committed (final, immutable) transcripts to the session.
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
                Ok(Message::Close(_)) => break,
                Err(e) => {
                    audio_sender.abort();
                    return Err(MediaError::SttError(e.to_string()));
                }
                _ => {}
            }
        }

        audio_sender.abort();
        Ok(())
    }
}
