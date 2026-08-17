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
