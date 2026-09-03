#![allow(dead_code)]
//! gRPC client for the backend Orchestrator streaming turn (Phase 1).
//!
//! Opens `RunTurnStream` and pumps the server stream into an mpsc channel of `TurnEvent`,
//! so the session can feed tokens into TTS sentence-by-sentence (low time-to-first-word).

use crate::error::MediaError;
use std::time::Duration;
use tokio::sync::{mpsc, OnceCell};
use tonic::metadata::MetadataValue;
use tonic::transport::{Channel, Endpoint};
use tonic::Request;

pub mod pb {
    tonic::include_proto!("auralius.voice.v1");
}
use pb::orchestrator_client::OrchestratorClient;
use pb::turn_chunk::Payload;

/// One warm HTTP/2 channel reused across every turn — multiplexed and auto-reconnecting —
/// instead of a fresh TCP+H2 handshake per turn. Keepalive pings hold the pool warm between
/// calls so the first token of each turn skips connection setup. The gRPC URL is a process
/// constant (config), so caching on first use is safe.
static CHANNEL: OnceCell<Channel> = OnceCell::const_new();

async fn shared_channel(url: &str) -> Result<Channel, MediaError> {
    let channel = CHANNEL
        .get_or_try_init(|| async {
            let endpoint = Endpoint::from_shared(url.to_string())
                .map_err(|e| MediaError::TurnError(format!("bad gRPC url: {e}")))?
                .http2_keep_alive_interval(Duration::from_secs(30))
                .keep_alive_timeout(Duration::from_secs(10))
                .keep_alive_while_idle(true)
                .tcp_keepalive(Some(Duration::from_secs(30)));
            // connect_lazy: build the channel now, connect on first RPC, reconnect on drop.
            Ok::<Channel, MediaError>(endpoint.connect_lazy())
        })
        .await?;
    Ok(channel.clone())
}

#[derive(Debug, Clone)]
pub struct TurnRequest {
    pub tenant_id: String,
    pub agent_id: String,
    /// Backend Call id — forwarded so the orchestrator persists per-turn transcripts and keeps
    /// per-call memory. Empty when absent (orchestrator then skips transcript writes).
    pub call_id: String,
    pub text: String,
    pub trace_id: String,
}

#[derive(Debug, Clone)]
pub enum TurnEvent {
    VoiceId(String),
    Delta(String),
    Done { full_answer: String },
    Error(String),
}

pub struct TurnClient {
    grpc_url: String,
    service_token: String,
}

impl TurnClient {
    pub fn new(grpc_url: String, service_token: String) -> Self {
        Self {
            grpc_url,
            service_token,
        }
    }

    /// Open the streaming turn. Returns a receiver of `TurnEvent`; a background task pumps the
    /// gRPC stream into it. When the receiver is dropped (barge-in / hang up), the pump stops.
    pub async fn run_turn_stream(
        &self,
        req: TurnRequest,
    ) -> Result<mpsc::Receiver<TurnEvent>, MediaError> {
        let channel = shared_channel(&self.grpc_url).await?;
        let mut client = OrchestratorClient::new(channel);

        let mut request = Request::new(pb::TurnRequest {
            tenant_id: req.tenant_id,
            agent_id: req.agent_id,
            text: req.text,
            trace_id: req.trace_id,
            call_id: req.call_id,
        });
        let meta: MetadataValue<_> = format!("Bearer {}", self.service_token)
            .parse()
            .map_err(|_| MediaError::TurnError("bad auth metadata".into()))?;
        request.metadata_mut().insert("authorization", meta);

        let mut streaming = client
            .run_turn_stream(request)
            .await
            .map_err(|e| MediaError::TurnError(format!("gRPC call failed: {e}")))?
            .into_inner();

        let (tx, rx) = mpsc::channel::<TurnEvent>(64);
        tokio::spawn(async move {
            loop {
                match streaming.message().await {
                    Ok(Some(chunk)) => {
                        let ev = match chunk.payload {
                            Some(Payload::VoiceId(v)) => TurnEvent::VoiceId(v),
                            Some(Payload::Delta(d)) => TurnEvent::Delta(d),
                            Some(Payload::Done(d)) => TurnEvent::Done {
                                full_answer: d.full_answer,
                            },
                            Some(Payload::Error(e)) => TurnEvent::Error(e),
                            None => continue,
                        };
                        if tx.send(ev).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let _ = tx.send(TurnEvent::Error(e.to_string())).await;
                        break;
                    }
                }
            }
        });

        Ok(rx)
    }
}
