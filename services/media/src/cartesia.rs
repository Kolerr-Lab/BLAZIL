//! Streaming text-to-speech via Cartesia Sonic (input-streaming WebSocket) — A/B alternative to
//! ElevenLabs (faster TTFA, more natural). Selected by `TTS_PROVIDER=cartesia`.
//!
//! URL: `wss://api.cartesia.ai/tts/websocket?api_key=…&cartesia_version=…`.
//! We request raw `pcm_s16le` @ 8 kHz, then convert to Twilio's μ-law with `codec::encode_frame`
//! (Cartesia has no native μ-law output, but 8 kHz PCM avoids any resampling — one cheap encode).
//! Protocol: one JSON message per text chunk sharing a `context_id`, `continue:true` while more
//! text is coming, then a final `continue:false` to flush. Responses are `{data:<b64 pcm>}` chunks
//! then `{done:true}`.
//!
//! Phase A: ignores the per-agent (ElevenLabs) `voice_id` and uses one configured Cartesia voice
//! (`CARTESIA_VOICE_ID`). Per-agent Cartesia voices = Phase B.

use crate::codec;
use crate::error::MediaError;
use crate::tts::Tts;
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[derive(Debug, Deserialize)]
struct CartesiaResponse {
    #[serde(default)]
    data: Option<String>, // base64 pcm_s16le
    #[serde(default)]
    done: Option<bool>,
    #[serde(rename = "type", default)]
    typ: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

pub struct CartesiaTts {
    api_key: String,
    model: String,
    version: String,
    voice_id: String,
}

impl CartesiaTts {
    pub fn new(api_key: String, model: String, version: String, voice_id: String) -> Self {
        Self {
            api_key,
            model,
            version,
            voice_id,
        }
    }
}

#[async_trait]
impl Tts for CartesiaTts {
    async fn speak(
        &self,
        voice_id: &str,
        mut text_rx: mpsc::Receiver<String>,
        audio_tx: mpsc::Sender<Vec<u8>>,
    ) -> Result<(), MediaError> {
        // Use the agent's selected Cartesia voice (the picker now stores Cartesia IDs); fall back to
        // the configured default when the agent has no/blank voice or a stale non-Cartesia id.
        let voice = if voice_id.trim().is_empty() {
            self.voice_id.clone()
        } else {
            voice_id.trim().to_string()
        };
        if self.api_key.is_empty() || voice.is_empty() {
            return Err(MediaError::TtsError(
                "Cartesia not configured (CARTESIA_API_KEY / voice)".into(),
            ));
        }

        let url = format!(
            "wss://api.cartesia.ai/tts/websocket?api_key={}&cartesia_version={}",
            self.api_key, self.version
        );
        let (ws_stream, _) = connect_async(url)
            .await
            .map_err(|e| MediaError::TtsError(format!("Cartesia connect failed: {e}")))?;
        let (mut write, mut read) = ws_stream.split();

        let context_id = uuid::Uuid::new_v4().to_string();
        let model = self.model.clone();
        let ctx = context_id.clone();

        // Writer: one message per text chunk (continue:true), then a final flush (continue:false).
        let text_sender = tokio::spawn(async move {
            while let Some(text) = text_rx.recv().await {
                let msg = serde_json::json!({
                    "model_id": model,
                    "transcript": text,
                    "voice": { "mode": "id", "id": voice },
                    "output_format": { "container": "raw", "encoding": "pcm_s16le", "sample_rate": 8000 },
                    "context_id": ctx,
                    "continue": true,
                });
                if write.send(Message::Text(msg.to_string())).await.is_err() {
                    return;
                }
            }
            // Flush: empty transcript with continue:false closes the context.
            let eos = serde_json::json!({
                "model_id": model,
                "transcript": "",
                "voice": { "mode": "id", "id": voice },
                "output_format": { "container": "raw", "encoding": "pcm_s16le", "sample_rate": 8000 },
                "context_id": ctx,
                "continue": false,
            });
            let _ = write.send(Message::Text(eos.to_string())).await;
        });

        // Reader: decode base64 pcm_s16le → i16 → μ-law → relay. Carry a stray odd byte across
        // messages so 16-bit samples never split.
        let mut carry: Option<u8> = None;
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let Ok(resp) = serde_json::from_str::<CartesiaResponse>(&text) else {
                        continue;
                    };
                    if let Some(err) = resp.error {
                        text_sender.abort();
                        return Err(MediaError::TtsError(format!("Cartesia error: {err}")));
                    }
                    if let Some(b64) = resp.data {
                        if let Ok(mut bytes) = BASE64.decode(b64) {
                            if let Some(c) = carry.take() {
                                bytes.insert(0, c);
                            }
                            if bytes.len() % 2 == 1 {
                                carry = bytes.pop();
                            }
                            let pcm: Vec<i16> = bytes
                                .chunks_exact(2)
                                .map(|b| i16::from_le_bytes([b[0], b[1]]))
                                .collect();
                            if !pcm.is_empty() {
                                let ulaw = codec::encode_frame(&pcm);
                                if audio_tx.send(ulaw).await.is_err() {
                                    text_sender.abort();
                                    return Ok(()); // receiver dropped (barge-in)
                                }
                            }
                        }
                    }
                    if resp.done.unwrap_or(false) || resp.typ.as_deref() == Some("done") {
                        break;
                    }
                }
                Ok(Message::Close(_)) => break,
                Err(e) => {
                    text_sender.abort();
                    return Err(MediaError::TtsError(format!("Cartesia ws error: {e}")));
                }
                _ => {}
            }
        }

        text_sender.abort();
        Ok(())
    }
}
