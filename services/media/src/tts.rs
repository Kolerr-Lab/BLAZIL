//! Streaming text-to-speech via ElevenLabs input-streaming WebSocket.
//!
//! URL: `wss://api.elevenlabs.io/v1/text-to-speech/{voice_id}/stream-input`.
//! We request `output_format=ulaw_8000` so audio comes back in Twilio's native G.711 μ-law
//! 8 kHz — no transcoding before we relay it to the call. Auth is the `xi-api-key` header on
//! the handshake (not an in-band field). Protocol: send a BOS message with voice settings,
//! then the text, then an EOS empty-text message to flush generation.

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

#[async_trait]
pub trait Tts: Send + Sync {
    async fn speak(
        &self,
        voice_id: &str,
        text_rx: mpsc::Receiver<String>,
        audio_tx: mpsc::Sender<Vec<u8>>,
    ) -> Result<(), MediaError>;
}

#[derive(Debug, Deserialize)]
struct TtsResponse {
    #[serde(default)]
    audio: Option<String>,
    #[serde(rename = "isFinal", default)]
    is_final: Option<bool>,
    #[serde(default)]
    error: Option<String>,
}

pub struct ElevenLabsTts {
    api_key: String,
    model_id: String,
}

impl ElevenLabsTts {
    pub fn new(api_key: String, model_id: String) -> Self {
        Self { api_key, model_id }
    }
}

#[async_trait]
impl Tts for ElevenLabsTts {
    async fn speak(
        &self,
        voice_id: &str,
        mut text_rx: mpsc::Receiver<String>,
        audio_tx: mpsc::Sender<Vec<u8>>,
    ) -> Result<(), MediaError> {
        let ws_url = format!(
            "wss://api.elevenlabs.io/v1/text-to-speech/{voice_id}/stream-input\
             ?model_id={}&output_format=ulaw_8000",
            self.model_id
        );

        let mut request = ws_url
            .as_str()
            .into_client_request()
            .map_err(|e| MediaError::TtsError(format!("Bad TTS URL: {e}")))?;
        request.headers_mut().insert(
            "xi-api-key",
            HeaderValue::from_str(&self.api_key)
                .map_err(|e| MediaError::TtsError(format!("Bad API key header: {e}")))?,
        );

        let (ws_stream, _) = connect_async(request)
            .await
            .map_err(|e| MediaError::TtsError(format!("TTS connect failed: {e}")))?;
        let (mut write, mut read) = ws_stream.split();

        // BOS: initialize the stream with voice settings.
        // stability 0.45 + style 0.3 (Bậc 0 human-voice pass): lower stability = more prosodic
        // variation (varied pace/pitch → less monotone, more human), and a touch of style adds
        // expressiveness (turbo_v2_5 supports it). The old 0.7 was raised to avoid the voice yelling
        // on `!`/CAPS — now mitigated upstream (sanitize_for_tts maps `!`→`.` + the reply prompt
        // bans `!`/ALL-CAPS), so we can lower it safely. Same model (turbo) → zero latency change.
        //
        // generation_config.chunk_length_schedule: how many characters ElevenLabs buffers before it
        // generates each chunk. Buffering gives the model LOOKAHEAD, so prosody (intonation/rhythm)
        // flows across the phrase instead of resetting at every fragment — this is what makes ALL
        // voices sound less robotic. Small first bucket (90) keeps time-to-first-audio ~unchanged;
        // later buckets grow so full sentences render smoothly. Paired with dropping the per-chunk
        // `flush` below (flush forces immediate per-fragment generation → prosody discontinuities).
        let bos = serde_json::json!({
            "text": " ",
            "voice_settings": { "stability": 0.45, "style": 0.3, "similarity_boost": 0.8 },
            "generation_config": { "chunk_length_schedule": [90, 160, 250, 290] }
        });
        write
            .send(Message::Text(bos.to_string()))
            .await
            .map_err(|e| MediaError::TtsError(e.to_string()))?;

        // Writer: stream text chunks as they arrive, then EOS to flush.
        // No per-chunk `flush`: we let ElevenLabs buffer per chunk_length_schedule so it has
        // lookahead and prosody flows across fragments (less robotic). The final empty-text EOS
        // flushes whatever is still buffered at end-of-turn, so nothing is left unspoken.
        let text_sender = tokio::spawn(async move {
            while let Some(text) = text_rx.recv().await {
                let msg = serde_json::json!({ "text": text });
                if write.send(Message::Text(msg.to_string())).await.is_err() {
                    return;
                }
            }
            // EOS: empty text closes the generation and flushes the buffered tail.
            let eos = serde_json::json!({ "text": "" });
            let _ = write.send(Message::Text(eos.to_string())).await;
        });

        // Reader: decode base64 μ-law audio chunks back to the caller.
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let Ok(resp) = serde_json::from_str::<TtsResponse>(&text) else {
                        continue;
                    };
                    if let Some(err) = resp.error {
                        text_sender.abort();
                        return Err(MediaError::TtsError(err));
                    }
                    if let Some(b64) = resp.audio {
                        if let Ok(bytes) = BASE64.decode(b64) {
                            if !bytes.is_empty() && audio_tx.send(bytes).await.is_err() {
                                text_sender.abort();
                                return Ok(()); // receiver dropped (barge-in)
                            }
                        }
                    }
                    if resp.is_final.unwrap_or(false) {
                        break;
                    }
                }
                Ok(Message::Close(_)) => break,
                Err(e) => {
                    text_sender.abort();
                    return Err(MediaError::TtsError(e.to_string()));
                }
                _ => {}
            }
        }

        text_sender.abort();
        Ok(())
    }
}
