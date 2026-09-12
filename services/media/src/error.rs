use thiserror::Error;

#[derive(Error, Debug)]
#[allow(dead_code)]
pub enum MediaError {
    #[error("WebSocket disconnected")]
    Disconnected,

    #[error("Invalid Twilio frame: {0}")]
    InvalidFrame(String),

    #[error("Audio decode error: {0}")]
    DecodeError(String),

    #[error("STT streaming error: {0}")]
    SttError(String),

    #[error("TTS streaming error: {0}")]
    TtsError(String),

    #[error("Turn processing error: {0}")]
    TurnError(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

impl MediaError {
    /// True when the underlying STT/TTS error message indicates a quota, rate-limit, or
    /// account-exhaustion condition (e.g. ElevenLabs quota outage, Deepgram rate limiting).
    /// Used to trigger the graceful fallback-clip-and-hangup behavior in session.rs.
    pub fn is_quota(&self) -> bool {
        let msg = match self {
            MediaError::SttError(m) | MediaError::TtsError(m) => m,
            _ => return false,
        };
        let m = msg.to_lowercase();
        m.contains("quota")
            || m.contains("rate_limited")
            || m.contains("rate limit")
            || m.contains("exceeded")
            || m.contains("exhausted")
    }
}
