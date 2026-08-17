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
pub trait Tts: Send + Sync {
    async fn speak(
        &self,
        voice_id: &str,
        text_rx: mpsc::Receiver<String>,
        audio_tx: mpsc::Sender<Vec<u8>>,
    ) -> Result<(), MediaError>;
}

#[derive(Debug, Serialize)]
struct ElevenLabsRequest {
    text: String,
    xi_api_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    voice_settings: Option<VoiceSettings>,
}

#[derive(Debug, Serialize)]
struct VoiceSettings {
    stability: f32,
    similarity_boost: f32,
}

#[derive(Debug, Deserialize)]
struct ElevenLabsResponse {
    audio: Option<String>,
    #[serde(rename = "isFinal")]
    is_final: Option<bool>,
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
            "wss://api.elevenlabs.io/v1/text-to-speech/{}/stream-input?model_id={}&output_format=ulaw_8000",
            voice_id, self.model_id
        );

        let (ws_stream, _) = connect_async(&ws_url)
            .await
            .map_err(|e| MediaError::TtsError(e.to_string()))?;

        let (mut write, mut read) = ws_stream.split();

        // Initial connection message with API key
        let init_msg = ElevenLabsRequest {
            text: " ".to_string(),
            xi_api_key: self.api_key.clone(),
            voice_settings: None,
        };

        write
            .send(Message::Text(serde_json::to_string(&init_msg)?))
            .await
            .map_err(|e| MediaError::TtsError(e.to_string()))?;

        // Send text chunks
        let text_sender_handle = tokio::spawn(async move {
            while let Some(text) = text_rx.recv().await {
                let msg = ElevenLabsRequest {
                    text,
                    xi_api_key: String::new(), // Not needed after init
                    voice_settings: None,
                };
                if let Ok(json) = serde_json::to_string(&msg) {
                    if write.send(Message::Text(json)).await.is_err() {
                        break;
                    }
                }
            }
            // Send EOS
            let eos = ElevenLabsRequest {
                text: "".to_string(),
                xi_api_key: String::new(),
                voice_settings: None,
            };
            if let Ok(json) = serde_json::to_string(&eos) {
                let _ = write.send(Message::Text(json)).await;
            }
        });

        // Receive audio chunks
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(resp) = serde_json::from_str::<ElevenLabsResponse>(&text) {
                        if let Some(err) = resp.error {
                            text_sender_handle.abort();
                            return Err(MediaError::TtsError(err));
                        }
                        if let Some(audio_base64) = resp.audio {
                            if let Ok(audio_bytes) = BASE64.decode(audio_base64) {
                                if audio_tx.send(audio_bytes).await.is_err() {
                                    text_sender_handle.abort();
                                    return Ok(()); // receiver dropped, likely barge-in
                                }
                            }
                        }
                        if resp.is_final.unwrap_or(false) {
                            break;
                        }
                    }
                }
                Ok(Message::Close(_)) => break,
                Err(e) => {
                    text_sender_handle.abort();
                    return Err(MediaError::TtsError(e.to_string()));
                }
                _ => {}
            }
        }

        text_sender_handle.abort();
        Ok(())
    }
}
