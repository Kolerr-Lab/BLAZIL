use anyhow::{Context, Result};
use std::env;

/// Parse a numeric env var, falling back to `default` on missing/empty/invalid input.
/// Deliberately non-fatal: a bad tuning value must never crash boot (which fails healthcheck).
fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    env::var(key)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(default)
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Config {
    pub bind_addr: String,
    pub public_wss: String,
    pub elevenlabs_api_key: String,
    pub elevenlabs_tts_model: String,
    pub elevenlabs_stt_model: String,
    /// ISO-639 code to bias STT; empty/None = auto-detect.
    pub stt_language_code: Option<String>,
    /// Fallback TTS voice used only when the backend returns no per-agent voice.
    pub default_voice_id: String,
    pub orch_base_url: String,
    /// gRPC endpoint of the backend Orchestrator (streaming turn). Private network, h2c.
    pub orch_grpc_url: String,
    pub orch_service_token: String,
    pub twilio_stream_auth: Option<String>,
    pub vad_aggressiveness: u8,
    pub barge_in_ms: u64,
    pub silence_end_ms: u64,
    /// TTS early-feed: flush the FIRST spoken chunk after this many words even without a sentence
    /// terminator, to cut time-to-first-audio. Later chunks stay sentence-based (prosody). 0 = off
    /// (feed whole sentences only) — the safe default; raise (e.g. 4) to A/B snappier starts.
    pub tts_early_feed_words: usize,
    /// User-turn text that opens the call so the agent greets first, in its own persona.
    /// Empty disables the auto-greeting (agent waits for the caller to speak).
    pub greeting_prompt: String,
    /// Short phrase spoken IMMEDIATELY at the start of a caller turn (in the agent's own voice) to
    /// mask backend think-time — e.g. "One moment.". Empty = disabled (no filler). Keep it short
    /// (~0.5-0.8s of speech) so a fast turn isn't delayed waiting for it to finish.
    pub thinking_filler: String,
    /// Predictive endpointing (Bước 3). When true, STT runs in MANUAL-commit mode and a local
    /// Smart Turn v2 model decides end-of-turn — committing early instead of waiting out silence.
    /// Default false → STT stays on server-side VAD (current behavior; zero risk).
    pub predictive_endpoint: bool,
    /// Path to the Smart Turn v2 ONNX model (wav2vec2, 16 kHz mono input, single-prob output).
    pub smart_turn_model_path: String,
    /// Utterance is treated as complete when the model probability ≥ this (0..1).
    pub smart_turn_threshold: f32,
    /// Silence (ms) after speech that triggers ONE Smart Turn check.
    pub endpoint_short_silence_ms: u64,
    /// Hard fallback: commit anyway after this much silence even if the model never says "done"
    /// (so a hesitant caller never hangs the turn).
    pub endpoint_max_silence_ms: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            // Railway injects $PORT; honor it, then MEDIA_BIND_ADDR, then a local default.
            bind_addr: env::var("MEDIA_BIND_ADDR").ok().unwrap_or_else(|| {
                env::var("PORT")
                    .map(|p| format!("0.0.0.0:{p}"))
                    .unwrap_or_else(|_| "0.0.0.0:8080".into())
            }),
            public_wss: env::var("MEDIA_PUBLIC_WSS")
                .unwrap_or_else(|_| "ws://localhost:8080/media/twilio".into()),
            elevenlabs_api_key: env::var("ELEVENLABS_API_KEY")
                .context("ELEVENLABS_API_KEY is required")?,
            elevenlabs_tts_model: env::var("ELEVENLABS_TTS_MODEL")
                .unwrap_or_else(|_| "eleven_turbo_v2_5".into()),
            elevenlabs_stt_model: env::var("ELEVENLABS_STT_MODEL")
                .unwrap_or_else(|_| "scribe_v2_realtime".into()),
            stt_language_code: env::var("STT_LANGUAGE_CODE").ok().filter(|s| !s.is_empty()),
            default_voice_id: env::var("DEFAULT_VOICE_ID")
                .unwrap_or_else(|_| "21m00Tcm4TlvDq8ikWAM".into()),
            orch_base_url: env::var("ORCH_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:8000".into()),
            orch_grpc_url: env::var("ORCH_GRPC_URL")
                .unwrap_or_else(|_| "http://localhost:50051".into()),
            orch_service_token: env::var("ORCH_SERVICE_TOKEN")
                .context("ORCH_SERVICE_TOKEN is required")?,
            twilio_stream_auth: env::var("TWILIO_STREAM_AUTH").ok(),
            vad_aggressiveness: env_parse("VAD_AGGRESSIVENESS", 2u8),
            // 120ms: with the adaptive-noise-floor barge gate, only clearly-voiced speech counts,
            // so a shorter window is safe and cuts perceived interrupt latency vs. the old 200ms.
            barge_in_ms: env_parse("BARGE_IN_MS", 120u64),
            // 300ms end-of-utterance: cuts ~180ms of dead wait per turn vs the old 480ms. English
            // (primary) pauses less mid-sentence, so this is safe; RAISE via SILENCE_END_MS if it
            // clips slow talkers.
            silence_end_ms: env_parse("SILENCE_END_MS", 300u64),
            tts_early_feed_words: env_parse("TTS_EARLY_FEED_WORDS", 0usize),
            greeting_prompt: env::var("MEDIA_GREETING_PROMPT").unwrap_or_else(|_| {
                "The call has just connected. Greet the caller warmly in one short sentence \
                 and ask how you can help."
                    .into()
            }),
            thinking_filler: env::var("MEDIA_THINKING_FILLER").unwrap_or_default(),
            predictive_endpoint: env::var("PREDICTIVE_ENDPOINT")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
            smart_turn_model_path: env::var("SMART_TURN_MODEL_PATH")
                .unwrap_or_else(|_| "/opt/models/smart-turn-v2/model.onnx".into()),
            smart_turn_threshold: env_parse("SMART_TURN_THRESHOLD", 0.5f32),
            endpoint_short_silence_ms: env_parse("ENDPOINT_SHORT_SILENCE_MS", 250u64),
            endpoint_max_silence_ms: env_parse("ENDPOINT_MAX_SILENCE_MS", 1500u64),
        })
    }
}
