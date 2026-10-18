//! Streaming text-to-speech via ElevenLabs input-streaming WebSocket.
//!
//! URL: `wss://api.elevenlabs.io/v1/text-to-speech/{voice_id}/stream-input`.
//! We request `output_format=ulaw_8000` so audio comes back in Twilio's native G.711 μ-law
//! 8 kHz — no transcoding before we relay it to the call. Auth is the `xi-api-key` header on
//! the handshake (not an in-band field). Protocol: send a BOS message with voice settings,
//! then the text, then an EOS empty-text message to flush generation.

use crate::config::Config;
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

/// The fallback TTS engine (always ElevenLabs). Used to recover a turn when the primary provider
/// (e.g. Cartesia) produced no audio, so a Cartesia outage never leaves the caller in silence.
pub fn build_fallback_tts(config: &Config) -> ElevenLabsTts {
    ElevenLabsTts::new(
        config.elevenlabs_api_key.clone(),
        config.elevenlabs_tts_model.clone(),
    )
}

/// Routes a turn to the right vendor engine, by language.
///
/// Two rewrites got this wrong, so the reasoning is worth keeping. Until 2026-10-15 an on-host
/// VieNeu-TTS path took every Vietnamese turn; it was dropped for sounding metallic at telephony
/// bandwidth. Its replacement then hardcoded "Vietnamese means ElevenLabs" and — worse — discarded
/// the `voice_id` it was handed in favour of a configured default, so an agent whose Vietnamese
/// voice had been chosen in the dashboard spoke Vietnamese in an English accent. The chosen voice
/// never reached an engine at all.
///
/// Now the split is configuration (`CARTESIA_LANGUAGES`, default `en,vi`) and the voice is always
/// whichever one the caller resolved. Cartesia has curated English and Vietnamese voices and is
/// the faster engine; ElevenLabs covers the long tail of languages Cartesia does not serve.
pub struct UnifiedTts {
    cartesia: Option<crate::cartesia::CartesiaTts>,
    elevenlabs: ElevenLabsTts,
    provider: String,
    /// Whether Cartesia serves the language this turn is answered in.
    cartesia_serves_language: bool,
}

impl UnifiedTts {
    pub fn new(config: &Config, language: Option<String>) -> Self {
        let cartesia_serves_language = config.cartesia_speaks(language.as_deref());

        let cartesia = if config.tts_provider == "cartesia" {
            Some(
                crate::cartesia::CartesiaTts::new(
                    config.cartesia_api_key.clone(),
                    config.cartesia_model.clone(),
                    config.cartesia_version.clone(),
                    config.cartesia_voice_id.clone(),
                )
                .with_language(language),
            )
        } else {
            None
        };

        let elevenlabs = ElevenLabsTts::new(
            config.elevenlabs_api_key.clone(),
            config.elevenlabs_tts_model.clone(),
        );

        Self {
            cartesia,
            elevenlabs,
            provider: config.tts_provider.clone(),
            cartesia_serves_language,
        }
    }
}

#[async_trait]
impl Tts for UnifiedTts {
    async fn speak(
        &self,
        voice_id: &str,
        text_rx: mpsc::Receiver<String>,
        audio_tx: mpsc::Sender<Vec<u8>>,
    ) -> Result<(), MediaError> {
        // Cartesia when it serves this language and is the configured provider; ElevenLabs
        // otherwise. `voice_id` is passed through untouched in both cases — resolving which voice
        // belongs to which engine is the caller's job (see `session.rs`), and doing it here as
        // well is what produced the accent bug described above.
        if self.cartesia_serves_language && self.provider == "cartesia" {
            if let Some(ref c) = self.cartesia {
                return c.speak(voice_id, text_rx, audio_tx).await;
            }
        }
        self.elevenlabs.speak(voice_id, text_rx, audio_tx).await
    }
}

/// Pick the TTS provider from config (`TTS_PROVIDER`). Default Cartesia.
pub fn build_tts(config: &Config) -> Box<dyn Tts> {
    build_tts_for(config, None)
}

/// Same, but tell the engine which language it is about to speak.
pub fn build_tts_for(config: &Config, language: Option<String>) -> Box<dyn Tts> {
    Box::new(UnifiedTts::new(config, language))
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    fn langs(csv: &str) -> HashSet<String> {
        csv.split(',').map(|s| s.trim().to_lowercase()).collect()
    }

    /// Mirrors `Config::cartesia_speaks`. Kept as a local copy because building a whole `Config`
    /// needs required env vars; the behaviour under test is the matching rule, not the struct.
    fn speaks(set: &HashSet<String>, language: Option<&str>) -> bool {
        let Some(lang) = language else { return true };
        let base = lang
            .trim()
            .split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_lowercase();
        base.is_empty() || set.contains(&base)
    }

    #[test]
    fn configured_languages_go_to_cartesia() {
        let set = langs("en,vi");
        for tag in ["en", "vi", "EN", " vi ", "en-US", "vi-VN", "vi_vn"] {
            assert!(speaks(&set, Some(tag)), "Cartesia should serve {tag:?}");
        }
    }

    #[test]
    fn everything_else_goes_to_elevenlabs() {
        let set = langs("en,vi");
        for tag in ["ja", "ko", "th", "zh-CN", "fr"] {
            assert!(!speaks(&set, Some(tag)), "ElevenLabs should serve {tag:?}");
        }
    }

    #[test]
    fn no_detected_language_stays_on_cartesia() {
        // The agent is answering in its configured default, whose voice is a Cartesia id. Sending
        // that id to ElevenLabs would 404 and drop the turn into the zero-audio recovery path.
        let set = langs("en,vi");
        assert!(speaks(&set, None));
        assert!(speaks(&set, Some("")));
        assert!(speaks(&set, Some("   ")));
    }

    #[test]
    fn the_set_is_configuration_not_a_hardcoded_pair() {
        // Guards the whole point of the change: an operator can move Vietnamese to ElevenLabs, or
        // add a language to Cartesia, without touching this file.
        let english_only = langs("en");
        assert!(speaks(&english_only, Some("en")));
        assert!(!speaks(&english_only, Some("vi")));

        let plus_japanese = langs("en,vi,ja");
        assert!(speaks(&plus_japanese, Some("ja")));
    }
}
