#![allow(dead_code)]
use crate::error::MediaError;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Serialize)]
pub struct TurnRequest {
    pub tenant_id: String,
    pub agent_id: String,
    pub text: String,
    pub trace_id: String,
}

#[derive(Debug, Deserialize)]
pub struct TurnResponse {
    pub answer: String,
    pub tier: Option<String>,
    pub model: Option<String>,
    pub tokens_in: Option<u32>,
    pub tokens_out: Option<u32>,
}

pub struct TurnClient {
    client: Client,
    base_url: String,
    service_token: String,
}

impl TurnClient {
    pub fn new(base_url: String, service_token: String) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .unwrap_or_default();

        Self {
            client,
            base_url,
            service_token,
        }
    }

    pub async fn run_turn(&self, request: &TurnRequest) -> Result<TurnResponse, MediaError> {
        let url = format!("{}/voice/turn", self.base_url.trim_end_matches('/'));

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.service_token))
            .json(request)
            .send()
            .await
            .map_err(|e| MediaError::TurnError(format!("Network error: {}", e)))?;

        if resp.status() != StatusCode::OK {
            return Err(MediaError::TurnError(format!(
                "API returned status {}",
                resp.status()
            )));
        }

        let data = resp
            .json::<TurnResponse>()
            .await
            .map_err(|e| MediaError::TurnError(format!("Failed to parse response: {}", e)))?;

        Ok(data)
    }
}
