//! One request to the Claude Messages API, answered in a fixed JSON shape:
//! the query, a short explanation, and whether Claude is confident.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value as Json, json};

use crate::Error;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2026-10-21";
/// Lets the API retry a declined Opus request on another model by itself.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const MAX_TOKENS: u32 = 16_000;
pub const TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Model {
    Haiku,
    Opus,
}

impl Model {
    pub fn id(self) -> &'static str {
        match self {
            Model::Haiku => "claude-haiku-5-5",
            Model::Opus => "claude-opus-5-5",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Model::Haiku => "Haiku",
            Model::Opus => "Opus",
        }
    }

    /// Haiku is the quick first try; Opus is asked when that wasn't enough,
    /// so it gets more room to think.
    fn effort(self) -> &'static str {
        match self {
            Model::Haiku => "low",
            Model::Opus => "high",
        }
    }
}

/// What Claude answered.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Answer {
    pub query: String,
    /// Read-only query for the relations that connect the answer to the
    /// records the question is about; empty if there are none.
    pub context_query: String,
    pub explanation: String,
    pub confident: bool,
}

/// An answer plus details of the request.
#[derive(Debug, Clone)]
pub struct Reply {
    pub answer: Answer,
    /// The model that actually answered (differs from the one asked if the
    /// API fell back to another model).
    pub served_by: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

pub async fn ask(
    http: &reqwest::Client,
    key: &str,
    model: Model,
    system: &str,
    user: &str,
) -> Result<Reply, Error> {
    let mut request = http
        .post(API_URL)
        .timeout(TIMEOUT)
        .header("x-api-key", key)
        .header("anthropic-version", API_VERSION)
        .json(&request_body(model, system, user));
    if model == Model::Opus {
        request = request.header("anthropic-beta", FALLBACK_BETA);
    }
    let response = request.send().await.map_err(|e| format!("could not reach the Claude API: {e}"))?;
    let status = response.status();
    let body: Json = response
        .json()
        .await
        .map_err(|e| format!("the Claude API sent an unreadable reply ({status}): {e}"))?;
    if !status.is_success() {
        return Err(api_error(status, &body).into());
    }
    parse_reply(&body)
}

pub fn request_body(model: Model, system: &str, user: &str) -> Json {
    let mut body = json!({
        "model": model.id(),
        "max_tokens": MAX_TOKENS,
        "system": system,
        "messages": [{ "role": "user", "content": user }],
        "output_config": {
            "effort": model.effort(),
            "format": { "type": "json_schema", "schema": answer_schema() },
        },
    });
    if model == Model::Opus {
        body["fallbacks"] = json!("default");
    }
    body
}

fn answer_schema() -> Json {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "SurrealQL that returns the records answering the question"
            },
            "context_query": {
                "type": "string",
                "description": "Read-only SurrealQL returning the relation records (edges) that connect the answer records to the records the question is about, or an empty string"
            },
            "explanation": {
                "type": "string",
                "description": "One or two short sentences for the user about what the query does"
            },
            "confident": {
                "type": "boolean",
                "description": "false if the question is ambiguous or you are unsure the query is right"
            }
        },
        "required": ["query", "context_query", "explanation", "confident"],
        "additionalProperties": false
    })
}

pub fn parse_reply(body: &Json) -> Result<Reply, Error> {
    match body["stop_reason"].as_str() {
        Some("refusal") => return Err("Claude declined to answer this question.".into()),
        Some("max_tokens") => return Err("Claude's answer was too long and got cut off.".into()),
        _ => {}
    }
    let text: String = body["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect();
    let answer: Answer = serde_json::from_str(&text)
        .map_err(|e| format!("Claude's answer wasn't in the expected form: {e}"))?;
    Ok(Reply {
        answer,
        served_by: body["model"].as_str().unwrap_or_default().to_string(),
        input_tokens: body["usage"]["input_tokens"].as_u64().unwrap_or(0),
        output_tokens: body["usage"]["output_tokens"].as_u64().unwrap_or(0),
    })
}

/// A readable message for an error reply.
fn api_error(status: reqwest::StatusCode, body: &Json) -> String {
    let message = body["error"]["message"].as_str().unwrap_or("no details");
    match status.as_u16() {
        401 => format!("The Claude API rejected the API key: {message}"),
        429 => format!("Too many requests or spend limit reached: {message}"),
        _ => format!("The Claude API returned an error ({status}): {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_opus_requests_ask_for_fallbacks() {
        let haiku = request_body(Model::Haiku, "s", "u");
        assert_eq!(haiku["model"], "claude-haiku-5-5");
        assert!(haiku.get("fallbacks").is_none());
        assert_eq!(haiku["output_config"]["format"]["type"], "json_schema");

        let opus = request_body(Model::Opus, "s", "u");
        assert_eq!(opus["model"], "claude-opus-5-5");
        assert_eq!(opus["fallbacks"], "default");
    }

    #[test]
    fn reads_the_answer_and_skips_thinking_blocks() {
        let body = json!({
            "model": "claude-haiku-5-5",
            "stop_reason": "end_turn",
            "content": [
                { "type": "thinking", "thinking": "" },
                { "type": "text", "text": "{\"query\":\"SELECT * FROM person;\",\"context_query\":\"\",\"explanation\":\"All people.\",\"confident\":true}" }
            ],
            "usage": { "input_tokens": 900, "output_tokens": 40 }
        });
        let reply = parse_reply(&body).unwrap();
        assert_eq!(reply.answer.query, "SELECT * FROM person;");
        assert!(reply.answer.confident);
        assert_eq!(reply.input_tokens, 900);
    }

    #[test]
    fn a_refusal_is_an_error() {
        let body = json!({ "stop_reason": "refusal", "content": [] });
        assert!(parse_reply(&body).is_err());
    }
}
