use anyhow::{Context, Result};
use std::env;

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Config {
    pub bind_addr: String,
    pub public_wss: String,
    pub elevenlabs_api_key: String,
    pub elevenlabs_tts_model: String,
    pub elevenlabs_stt_model: String,
    pub orch_base_url: String,
    pub orch_service_token: String,
    pub twilio_stream_auth: Option<String>,
    pub vad_aggressiveness: u8,
    pub barge_in_ms: u64,
    pub silence_end_ms: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            bind_addr: env::var("MEDIA_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            public_wss: env::var("MEDIA_PUBLIC_WSS")
                .unwrap_or_else(|_| "ws://localhost:8080/media/twilio".into()),
            elevenlabs_api_key: env::var("ELEVENLABS_API_KEY")
                .context("ELEVENLABS_API_KEY is required")?,
            elevenlabs_tts_model: env::var("ELEVENLABS_TTS_MODEL")
                .unwrap_or_else(|_| "eleven_turbo_v2_5".into()),
            elevenlabs_stt_model: env::var("ELEVENLABS_STT_MODEL")
                .unwrap_or_else(|_| "scribe_v1".into()),
            orch_base_url: env::var("ORCH_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:8000".into()),
            orch_service_token: env::var("ORCH_SERVICE_TOKEN")
                .context("ORCH_SERVICE_TOKEN is required")?,
            twilio_stream_auth: env::var("TWILIO_STREAM_AUTH").ok(),
            vad_aggressiveness: env::var("VAD_AGGRESSIVENESS")
                .unwrap_or_else(|_| "2".into())
                .parse()
                .context("Invalid VAD_AGGRESSIVENESS")?,
            barge_in_ms: env::var("BARGE_IN_MS")
                .unwrap_or_else(|_| "200".into())
                .parse()
                .context("Invalid BARGE_IN_MS")?,
            silence_end_ms: env::var("SILENCE_END_MS")
                .unwrap_or_else(|_| "700".into())
                .parse()
                .context("Invalid SILENCE_END_MS")?,
        })
    }
}
