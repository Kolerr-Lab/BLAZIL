//! In-process / on-host VieNeu-TTS v3 Turbo integration for live call audio streaming.
//!
//! Replaces Cartesia Vietnamese TTS with local VieNeu ONNX Turbo engine.
//! Calls `{orch_base_url}/voices/{voice_id}/stream-chunk` using persistent connection pooling.
//! Returns native G.711 μ-law 8 kHz audio directly (no transcoding, 0ms international network RTT).
//! Slices into ~80ms frames (640 bytes) for smooth streaming and instant barge-in response.

use crate::error::MediaError;
use crate::tts::Tts;
use async_trait::async_trait;
use tokio::sync::mpsc;

/// Legacy Cartesia Vietnamese voice IDs to be resolved to canonical VieNeu presets.
pub const LEGACY_CARTESIA_VIETNAMESE: &[(&str, &str)] = &[
    ("8e8f222d-60a6-427c-9b88-12ee4999f7d0", "vn_north_mai_anh"), // Linh
    ("935a9060-ad68-45ec-9f5b-6ec217c91723", "vn_north_truc_ly"), // Mai
    (
        "b8cd71e3-c215-4676-9289-498c464fa7e2",
        "vn_north_doan_trang",
    ), // Lan
    ("0e58d60a-d8c3-4d44-93e1-7e8c37466cf8", "vn_north_hai_dang"), // Minh
];

/// Check if a voice ID is one of the 4 legacy Cartesia Vietnamese voices.
pub fn is_legacy_cartesia_vietnamese(voice_id: &str) -> bool {
    let clean = voice_id.trim();
    LEGACY_CARTESIA_VIETNAMESE
        .iter()
        .any(|(legacy, _)| *legacy == clean)
}

/// Resolve any voice ID to a canonical VieNeu ID.
pub fn resolve_vieneu_voice(voice_id: &str, default_voice: &str) -> String {
    let clean = voice_id.trim();
    if clean.is_empty() {
        return default_voice.trim().to_string();
    }
    for (legacy, mapped) in LEGACY_CARTESIA_VIETNAMESE {
        if *legacy == clean {
            return mapped.to_string();
        }
    }
    if clean.starts_with("vn_") {
        return clean.to_string();
    }
    // Fallback to default VieNeu voice if an unknown non-Vietnamese ID was passed
    default_voice.trim().to_string()
}

pub struct VieneuTts {
    client: reqwest::Client,
    orch_base_url: String,
    orch_service_token: String,
    default_voice_id: String,
}

impl VieneuTts {
    pub fn new(
        orch_base_url: String,
        orch_service_token: String,
        default_voice_id: String,
    ) -> Self {
        let client = reqwest::Client::builder()
            .tcp_nodelay(true)
            .pool_idle_timeout(Some(std::time::Duration::from_secs(90)))
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let default_voice = if default_voice_id.trim().is_empty() {
            "vn_north_hai_dang".to_string()
        } else {
            default_voice_id.trim().to_string()
        };

        Self {
            client,
            orch_base_url,
            orch_service_token,
            default_voice_id: default_voice,
        }
    }
}

#[async_trait]
impl Tts for VieneuTts {
    async fn speak(
        &self,
        voice_id: &str,
        mut text_rx: mpsc::Receiver<String>,
        audio_tx: mpsc::Sender<Vec<u8>>,
    ) -> Result<(), MediaError> {
        let voice = resolve_vieneu_voice(voice_id, &self.default_voice_id);
        let base = self.orch_base_url.trim_end_matches('/');
        let url = format!("{base}/voices/{voice}/stream-chunk");

        while let Some(text) = text_rx.recv().await {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }

            let mut req = self.client.post(&url).json(&serde_json::json!({
                "text": trimmed,
            }));

            if !self.orch_service_token.is_empty() {
                req = req.header(
                    "Authorization",
                    format!("Bearer {}", self.orch_service_token),
                );
            }

            let resp = req
                .send()
                .await
                .map_err(|e| MediaError::TtsError(format!("VieNeu stream request failed: {e}")))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let err_text = resp.text().await.unwrap_or_default();
                return Err(MediaError::TtsError(format!(
                    "VieNeu HTTP error {status}: {err_text}"
                )));
            }

            let bytes = resp
                .bytes()
                .await
                .map_err(|e| MediaError::TtsError(format!("VieNeu read bytes failed: {e}")))?;

            if bytes.is_empty() {
                continue;
            }

            // Slice into frames of 640 bytes (~80ms of 8kHz mu-law audio)
            // for smooth playout and immediate barge-in responsiveness
            const FRAME_SIZE: usize = 640;
            for chunk in bytes.chunks(FRAME_SIZE) {
                if audio_tx.send(chunk.to_vec()).await.is_err() {
                    return Ok(()); // Receiver dropped (barge-in / call ended)
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_legacy_cartesia_vietnamese_mapping() {
        assert!(is_legacy_cartesia_vietnamese(
            "0e58d60a-d8c3-4d44-93e1-7e8c37466cf8"
        ));
        assert!(is_legacy_cartesia_vietnamese(
            "8e8f222d-60a6-427c-9b88-12ee4999f7d0"
        ));
        assert!(!is_legacy_cartesia_vietnamese("unknown-id"));

        assert_eq!(
            resolve_vieneu_voice("0e58d60a-d8c3-4d44-93e1-7e8c37466cf8", "vn_north_hai_dang"),
            "vn_north_hai_dang"
        );
        assert_eq!(
            resolve_vieneu_voice("8e8f222d-60a6-427c-9b88-12ee4999f7d0", "vn_north_hai_dang"),
            "vn_north_mai_anh"
        );
        assert_eq!(
            resolve_vieneu_voice("vn_south_thuy_dung", "vn_north_hai_dang"),
            "vn_south_thuy_dung"
        );
        assert_eq!(
            resolve_vieneu_voice("", "vn_north_hai_dang"),
            "vn_north_hai_dang"
        );
    }
}
