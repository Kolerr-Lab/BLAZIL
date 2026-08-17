#![allow(dead_code)]
use crate::error::MediaError;
use anyhow::Result;
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[async_trait]
pub trait Stt: Send + Sync {
    async fn stream(
        &self,
        ulaw_rx: mpsc::Receiver<Vec<u8>>,
        transcript_tx: mpsc::Sender<String>,
    ) -> Result<(), MediaError>;
}

#[derive(Debug, Serialize)]
struct SttRequest {
    text: Option<String>,
    audio: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SttResponse {
    text: Option<String>,
    #[serde(rename = "isFinal")]
    is_final: Option<bool>,
    error: Option<String>,
}

pub struct ElevenLabsStt {
    api_key: String,
    model_id: String,
}

impl ElevenLabsStt {
    pub fn new(api_key: String, model_id: String) -> Self {
        Self { api_key, model_id }
    }
}

#[async_trait]
impl Stt for ElevenLabsStt {
    async fn stream(
        &self,
        mut ulaw_rx: mpsc::Receiver<Vec<u8>>,
        transcript_tx: mpsc::Sender<String>,
    ) -> Result<(), MediaError> {
        // ElevenLabs doesn't have a public streaming STT WS endpoint yet, so this is a placeholder URL
        // matching the ElevenLabs TTS WS structure. The implementation will need to be adjusted
        // once ElevenLabs officially supports STT over WS, or if another provider is used.
        let ws_url = format!(
            "wss://api.elevenlabs.io/v1/speech-to-text/stream-input?model_id={}&output_format=ulaw_8000",
            self.model_id
        );

        // TODO: Replace with the actual endpoint for ElevenLabs STT when available.
        let (ws_stream, _) = connect_async(&ws_url)
            .await
            .map_err(|e| MediaError::SttError(e.to_string()))?;

        let (mut write, mut read) = ws_stream.split();

        // Send audio chunks
        let audio_sender_handle = tokio::spawn(async move {
            while let Some(audio_bytes) = ulaw_rx.recv().await {
                let base64_audio = BASE64.encode(audio_bytes);
                let msg = SttRequest {
                    text: None,
                    audio: Some(base64_audio),
                };
                if let Ok(json) = serde_json::to_string(&msg) {
                    if write.send(Message::Text(json)).await.is_err() {
                        break;
                    }
                }
            }
            // Send EOS
            let eos = SttRequest {
                text: Some("".to_string()),
                audio: None,
            };
            if let Ok(json) = serde_json::to_string(&eos) {
                let _ = write.send(Message::Text(json)).await;
            }
        });

        // Receive transcript
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(resp) = serde_json::from_str::<SttResponse>(&text) {
                        if let Some(err) = resp.error {
                            audio_sender_handle.abort();
                            return Err(MediaError::SttError(err));
                        }
                        if let Some(transcript) = resp.text {
                            if transcript_tx.send(transcript).await.is_err() {
                                audio_sender_handle.abort();
                                return Ok(()); // Receiver dropped
                            }
                        }
                        if resp.is_final.unwrap_or(false) {
                            break;
                        }
                    }
                }
                Ok(Message::Close(_)) => break,
                Err(e) => {
                    audio_sender_handle.abort();
                    return Err(MediaError::SttError(e.to_string()));
                }
                _ => {}
            }
        }

        audio_sender_handle.abort();
        Ok(())
    }
}
