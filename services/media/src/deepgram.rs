//! Real-time speech-to-text via Deepgram (Nova-3) — the STT failover provider (C1).
//!
//! WebSocket: `wss://api.deepgram.com/v1/listen`. We stream Twilio's native G.711 μ-law 8 kHz
//! straight through (`encoding=mulaw&sample_rate=8000`), so no transcoding is needed on the media
//! plane — identical to the ElevenLabs path.
//!
//! Endpointing parity with `ElevenLabsStt` (both drive the SAME `Stt` trait):
//!   * `commit_strategy = "manual"` (Smart Turn v3 predictive endpointing): Deepgram endpointing is
//!     disabled; the media plane owns end-of-turn. When the session sends a commit we push a
//!     `{"type":"Finalize"}` control frame, Deepgram flushes, and the concatenation of finalized
//!     segments since the last commit becomes ONE committed transcript — exactly the semantics of
//!     ElevenLabs manual commit.
//!   * `commit_strategy = "vad"`: Deepgram segments server-side on silence; we emit the utterance
//!     when `speech_final` (or an `UtteranceEnd` event) arrives.
//!
//! Auth is the `Authorization: Token <key>` header on the handshake. This provider only ever opens
//! a socket when it is the active provider (see `stt_failover`), so a healthy primary pays nothing.

use crate::error::MediaError;
use crate::stt::Stt;
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, http::HeaderValue, Message},
};

/// Grace after a manual `Finalize` before we emit whatever is buffered, so a turn is never hung
/// waiting on a flush result that Deepgram already delivered inline.
const FLUSH_GRACE: Duration = Duration::from_millis(400);
/// KeepAlive cadence — Deepgram closes an idle socket at ~12 s. Twilio streams continuous frames
/// during a call so this is a safety net (e.g. long TTS playback with a silent caller).
const KEEPALIVE_SECS: u64 = 8;

/// Parameters that shape the Deepgram realtime session. Mirrors what `SttParams` carries, plus the
/// Deepgram-specific credential/model, so `stt_failover` can build it from config + the shared
/// `SttParams` without ElevenLabs and Deepgram sharing a struct.
#[derive(Clone)]
pub struct DeepgramParams {
    pub api_key: String,
    pub model: String,
    /// ISO-639 code to constrain recognition, or `None` → Nova-3 multilingual (`language=multi`).
    pub language: Option<String>,
    /// "manual" (Smart Turn drives commits) or "vad" (Deepgram segments on silence).
    pub commit_strategy: String,
    /// Silence (seconds) Deepgram treats as end-of-utterance — VAD strategy only.
    pub vad_silence_secs: f32,
}

pub struct DeepgramStt {
    params: DeepgramParams,
}

impl DeepgramStt {
    pub fn new(params: DeepgramParams) -> Self {
        Self { params }
    }

    fn is_manual(&self) -> bool {
        self.params.commit_strategy == "manual"
    }

    fn ws_url(&self) -> String {
        let mut url = format!(
            "wss://api.deepgram.com/v1/listen\
             ?model={}&encoding=mulaw&sample_rate=8000&channels=1\
             &interim_results=true&smart_format=true",
            self.params.model
        );
        if self.is_manual() {
            // Smart Turn owns end-of-turn — never let Deepgram auto-finalize on silence.
            url.push_str("&endpointing=false");
        } else {
            let ms = (self.params.vad_silence_secs * 1000.0) as u32;
            url.push_str(&format!(
                "&endpointing={}&utterance_end_ms={}&vad_events=true",
                ms.max(10),
                ms.max(1000) // Deepgram requires utterance_end_ms >= 1000
            ));
        }
        let lang = self
            .params
            .language
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("multi");
        url.push_str(&format!("&language={lang}"));
        url
    }
}

/// One inbound Deepgram event. Only the fields we act on are typed; `type` is the discriminator.
#[derive(Debug, Deserialize)]
struct DgEvent {
    #[serde(rename = "type", default)]
    typ: Option<String>,
    #[serde(default)]
    channel: Option<DgChannel>,
    #[serde(default)]
    is_final: Option<bool>,
    #[serde(default)]
    speech_final: Option<bool>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DgChannel {
    #[serde(default)]
    alternatives: Vec<DgAlt>,
}

#[derive(Debug, Deserialize)]
struct DgAlt {
    #[serde(default)]
    transcript: Option<String>,
}

impl DgEvent {
    fn transcript(&self) -> String {
        self.channel
            .as_ref()
            .and_then(|c| c.alternatives.first())
            .and_then(|a| a.transcript.as_deref())
            .unwrap_or("")
            .trim()
            .to_string()
    }
}

/// Join and emit the buffered utterance to the session. Returns false if the receiver was dropped
/// (session gone / barge-in) so the caller can stop cleanly.
async fn emit(buffer: &mut Vec<String>, tx: &mpsc::Sender<String>) -> bool {
    if buffer.is_empty() {
        return true;
    }
    let text = buffer.join(" ").trim().to_string();
    buffer.clear();
    if text.is_empty() {
        return true;
    }
    tx.send(text).await.is_ok()
}

/// Map a Deepgram close/error into a `MediaError`. Quota/limit/auth reasons carry keywords so
/// `MediaError::is_quota()` recognizes them for the graceful end-of-call fallback.
fn stt_err(reason: impl Into<String>) -> MediaError {
    MediaError::SttError(reason.into())
}

#[async_trait]
impl Stt for DeepgramStt {
    async fn stream(
        &self,
        mut ulaw_rx: mpsc::Receiver<Vec<u8>>,
        transcript_tx: mpsc::Sender<String>,
        mut commit_rx: mpsc::Receiver<()>,
    ) -> Result<(), MediaError> {
        let manual = self.is_manual();
        let mut request = self
            .ws_url()
            .as_str()
            .into_client_request()
            .map_err(|e| stt_err(format!("Bad Deepgram URL: {e}")))?;
        request.headers_mut().insert(
            "Authorization",
            HeaderValue::from_str(&format!("Token {}", self.params.api_key))
                .map_err(|e| stt_err(format!("Bad Deepgram auth header: {e}")))?,
        );

        let (ws_stream, _) = connect_async(request)
            .await
            .map_err(|e| stt_err(format!("Deepgram connect failed: {e}")))?;
        let (mut write, mut read) = ws_stream.split();

        // `pending` is set by the writer when it forwards a manual commit (Finalize), and observed
        // by the reader so the next finalized segment closes the utterance. Lock-free, no shared
        // buffer between the two tasks.
        let pending = Arc::new(AtomicBool::new(false));
        let pending_w = Arc::clone(&pending);

        // Writer task: μ-law audio as BINARY frames, manual commit → Finalize, periodic KeepAlive.
        let writer = tokio::spawn(async move {
            let mut keepalive = tokio::time::interval(Duration::from_secs(KEEPALIVE_SECS));
            keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut commit_open = true;
            loop {
                tokio::select! {
                    maybe_audio = ulaw_rx.recv() => match maybe_audio {
                        Some(ulaw) => {
                            if write.send(Message::Binary(ulaw)).await.is_err() {
                                break;
                            }
                        }
                        None => {
                            // Call ended: ask Deepgram to close the stream cleanly.
                            let _ = write
                                .send(Message::Text(r#"{"type":"CloseStream"}"#.to_string()))
                                .await;
                            break;
                        }
                    },
                    maybe_commit = commit_rx.recv(), if commit_open => match maybe_commit {
                        Some(()) => {
                            if write
                                .send(Message::Text(r#"{"type":"Finalize"}"#.to_string()))
                                .await
                                .is_err()
                            {
                                break;
                            }
                            pending_w.store(true, Ordering::Release);
                        }
                        None => commit_open = false,
                    },
                    _ = keepalive.tick() => {
                        if write
                            .send(Message::Text(r#"{"type":"KeepAlive"}"#.to_string()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });

        // Reader loop: accumulate finalized segments; emit one committed transcript per utterance.
        let mut buffer: Vec<String> = Vec::new();
        let mut pending_since: Option<Instant> = None;
        let mut ticker = tokio::time::interval(Duration::from_millis(100));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let outcome: Result<(), MediaError> = loop {
            tokio::select! {
                _ = ticker.tick() => {
                    // Manual flush safety net: if a commit fired but Deepgram produced no new final
                    // (everything already finalized), emit what we have after a short grace.
                    if manual && pending.load(Ordering::Acquire) {
                        match pending_since {
                            None => pending_since = Some(Instant::now()),
                            Some(t) if t.elapsed() >= FLUSH_GRACE => {
                                if !emit(&mut buffer, &transcript_tx).await {
                                    break Ok(());
                                }
                                pending.store(false, Ordering::Release);
                                pending_since = None;
                            }
                            _ => {}
                        }
                    }
                }
                msg = read.next() => {
                    let Some(msg) = msg else { break Ok(()) };
                    match msg {
                        Ok(Message::Text(text)) => {
                            let Ok(ev) = serde_json::from_str::<DgEvent>(&text) else {
                                continue;
                            };
                            match ev.typ.as_deref() {
                                Some("Results") | None => {
                                    let transcript = ev.transcript();
                                    let is_final = ev.is_final.unwrap_or(false);
                                    let speech_final = ev.speech_final.unwrap_or(false);
                                    if is_final && !transcript.is_empty() {
                                        buffer.push(transcript);
                                    }
                                    if manual {
                                        if pending.load(Ordering::Acquire) && is_final {
                                            if !emit(&mut buffer, &transcript_tx).await {
                                                break Ok(());
                                            }
                                            pending.store(false, Ordering::Release);
                                            pending_since = None;
                                        }
                                    } else if speech_final && !emit(&mut buffer, &transcript_tx).await {
                                        break Ok(());
                                    }
                                }
                                Some("UtteranceEnd") => {
                                    // VAD strategy belt-and-suspenders: flush if speech_final was missed.
                                    if !manual && !emit(&mut buffer, &transcript_tx).await {
                                        break Ok(());
                                    }
                                }
                                Some("Error") => {
                                    let reason = ev
                                        .description
                                        .or(ev.message)
                                        .unwrap_or_else(|| "deepgram error".to_string());
                                    break Err(stt_err(format!("Deepgram error: {reason}")));
                                }
                                _ => {}
                            }
                        }
                        Ok(Message::Close(frame)) => {
                            let reason = frame
                                .map(|f| format!("{f:?}"))
                                .unwrap_or_else(|| "closed".to_string());
                            break Err(stt_err(format!("Deepgram closed: {reason}")));
                        }
                        Err(e) => break Err(stt_err(format!("Deepgram ws error: {e}"))),
                        _ => {}
                    }
                }
            }
        };

        writer.abort();
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(strategy: &str) -> DeepgramParams {
        DeepgramParams {
            api_key: "test".into(),
            model: "nova-3".into(),
            language: None,
            commit_strategy: strategy.into(),
            vad_silence_secs: 0.3,
        }
    }

    #[test]
    fn manual_url_disables_endpointing_and_defaults_multi() {
        let url = DeepgramStt::new(params("manual")).ws_url();
        assert!(url.contains("model=nova-3"));
        assert!(url.contains("encoding=mulaw"));
        assert!(url.contains("sample_rate=8000"));
        assert!(url.contains("endpointing=false"));
        assert!(url.contains("language=multi"));
    }

    #[test]
    fn vad_url_sets_endpointing_and_utterance_end() {
        let url = DeepgramStt::new(params("vad")).ws_url();
        assert!(url.contains("endpointing=300"));
        assert!(url.contains("utterance_end_ms=1000"));
        assert!(url.contains("vad_events=true"));
    }

    #[test]
    fn explicit_language_wins_over_multi() {
        let mut p = params("manual");
        p.language = Some("vi".into());
        let url = DeepgramStt::new(p).ws_url();
        assert!(url.contains("language=vi"));
        assert!(!url.contains("language=multi"));
    }

    #[test]
    fn parses_final_results_transcript() {
        let json = r#"{"type":"Results","channel":{"alternatives":[{"transcript":"hello there"}]},"is_final":true,"speech_final":false}"#;
        let ev: DgEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.typ.as_deref(), Some("Results"));
        assert_eq!(ev.transcript(), "hello there");
        assert_eq!(ev.is_final, Some(true));
        assert_eq!(ev.speech_final, Some(false));
    }

    #[test]
    fn parses_utterance_end_event() {
        let json = r#"{"type":"UtteranceEnd","last_word_end":1.23}"#;
        let ev: DgEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.typ.as_deref(), Some("UtteranceEnd"));
        assert_eq!(ev.transcript(), "");
    }

    #[tokio::test]
    async fn emit_joins_and_clears_buffer() {
        let (tx, mut rx) = mpsc::channel::<String>(4);
        let mut buf = vec!["hello".to_string(), "world".to_string()];
        assert!(emit(&mut buf, &tx).await);
        assert_eq!(rx.recv().await.unwrap(), "hello world");
        assert!(buf.is_empty());
        // Empty buffer is a no-op that still reports the channel open.
        assert!(emit(&mut buf, &tx).await);
    }
}
