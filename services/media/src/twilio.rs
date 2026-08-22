use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(tag = "event")]
#[allow(dead_code)]
pub enum InboundMessage {
    #[serde(rename = "connected")]
    Connected { protocol: String, version: String },
    #[serde(rename = "start")]
    Start {
        #[serde(rename = "streamSid")]
        stream_sid: String,
        // Real Twilio nests callSid/customParameters inside a `start` object (NOT top-level).
        // Getting this wrong silently drops the start event → no greeting/STT → dead air.
        start: StartMetadata,
    },
    #[serde(rename = "media")]
    Media {
        #[serde(rename = "streamSid")]
        stream_sid: String,
        media: MediaPayload,
    },
    #[serde(rename = "stop")]
    Stop {
        #[serde(rename = "streamSid")]
        stream_sid: String,
    },
    #[serde(rename = "mark")]
    Mark {
        #[serde(rename = "streamSid")]
        stream_sid: String,
        mark: MarkPayload,
    },
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct MediaPayload {
    pub payload: String, // base64 encoded G.711 μ-law
    pub track: Option<String>,
}

/// Nested `start` object from Twilio's Media Streams `start` event. callSid and the
/// `<Parameter>` values (tenant_id/agent_id) live here, not at the message's top level.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct StartMetadata {
    #[serde(rename = "callSid")]
    pub call_sid: String,
    #[serde(rename = "customParameters", default)]
    pub custom_parameters: Option<std::collections::HashMap<String, String>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MarkPayload {
    pub name: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "event")]
#[allow(dead_code)]
pub enum OutboundMessage {
    #[serde(rename = "media")]
    Media {
        #[serde(rename = "streamSid")]
        stream_sid: String,
        media: OutboundMediaPayload,
    },
    #[serde(rename = "clear")]
    Clear {
        #[serde(rename = "streamSid")]
        stream_sid: String,
    },
    #[serde(rename = "mark")]
    Mark {
        #[serde(rename = "streamSid")]
        stream_sid: String,
        mark: MarkPayload,
    },
}

#[derive(Debug, Serialize)]
pub struct OutboundMediaPayload {
    pub payload: String,
}

#[allow(dead_code)]
impl OutboundMessage {
    pub fn media(stream_sid: impl Into<String>, payload: impl Into<String>) -> Self {
        Self::Media {
            stream_sid: stream_sid.into(),
            media: OutboundMediaPayload {
                payload: payload.into(),
            },
        }
    }

    pub fn clear(stream_sid: impl Into<String>) -> Self {
        Self::Clear {
            stream_sid: stream_sid.into(),
        }
    }

    pub fn mark(stream_sid: impl Into<String>, name: impl Into<String>) -> Self {
        Self::Mark {
            stream_sid: stream_sid.into(),
            mark: MarkPayload { name: name.into() },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_start_event() {
        // Mirrors the REAL Twilio start frame: callSid + customParameters nested under `start`.
        let json = r#"{
            "event": "start",
            "sequenceNumber": "1",
            "streamSid": "MZ123",
            "start": {
                "streamSid": "MZ123",
                "accountSid": "AC123",
                "callSid": "CA123",
                "tracks": ["inbound"],
                "customParameters": {
                    "tenant_id": "t-1",
                    "agent_id": "a-1"
                }
            }
        }"#;

        let msg: InboundMessage = serde_json::from_str(json).unwrap();
        match msg {
            InboundMessage::Start { stream_sid, start } => {
                assert_eq!(stream_sid, "MZ123");
                assert_eq!(start.call_sid, "CA123");
                let params = start.custom_parameters.unwrap();
                assert_eq!(params.get("tenant_id").unwrap(), "t-1");
                assert_eq!(params.get("agent_id").unwrap(), "a-1");
            }
            _ => panic!("Expected Start event"),
        }
    }

    #[test]
    fn parse_media_event() {
        let json = r#"{
            "event": "media",
            "streamSid": "MZ123",
            "media": {
                "track": "inbound",
                "payload": "abc"
            }
        }"#;

        let msg: InboundMessage = serde_json::from_str(json).unwrap();
        match msg {
            InboundMessage::Media { media, .. } => {
                assert_eq!(media.payload, "abc");
                assert_eq!(media.track.as_deref(), Some("inbound"));
            }
            _ => panic!("Expected Media event"),
        }
    }
}
