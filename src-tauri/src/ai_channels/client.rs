//! One request/response per protocol: plain text in, plain text out.
//!
//! Base URLs follow each provider SDK's convention, so a URL copied from a
//! relay's docs works unchanged: OpenAI URLs include the version
//! (`https://api.openai.com/v1`) and paths are appended as-is; Anthropic URLs
//! may omit it (`https://api.anthropic.com`) and `/v1` is added unless the URL
//! already ends in it.

use std::time::Duration;

use serde_json::{json, Value};

use super::{AiApi, AiChannel};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const ANTHROPIC_VERSION: &str = "2023-06-01";
const ERROR_EXCERPT: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiError {
    /// No channel/model is chosen for this purpose, or it is disabled.
    NotConfigured,
    /// The channel has no API key in the keychain.
    MissingKey,
    /// The provider answered with an error status.
    Rejected { status: u16, body: String },
    /// The request never completed.
    Transport(String),
    /// The provider answered, but not in the protocol's shape.
    Malformed(String),
}

impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured => f.write_str("no AI channel and model are chosen for this"),
            Self::MissingKey => f.write_str("the AI channel has no API key"),
            Self::Rejected { status, body } => {
                write!(f, "the provider answered HTTP {status}: {body}")
            }
            Self::Transport(message) => write!(f, "the request failed: {message}"),
            Self::Malformed(message) => write!(f, "unexpected provider response: {message}"),
        }
    }
}

impl std::error::Error for AiError {}

pub struct ChatRequest<'a> {
    pub system: &'a str,
    pub user: &'a str,
    /// Output cap; `None` leaves it to the provider where the API allows.
    pub max_tokens: Option<u32>,
}

/// Anthropic Messages requires `max_tokens`; this is what an uncapped
/// request sends, low enough for every Claude model to accept.
const ANTHROPIC_DEFAULT_MAX_TOKENS: u32 = 4096;

/// Trim trailing slashes and accept only http(s).
pub fn normalize_base_url(base_url: &str) -> Result<String, String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    let parsed =
        reqwest::Url::parse(trimmed).map_err(|_| format!("`{base_url}` is not a valid URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(format!("`{base_url}` must be an http(s) URL"));
    }
    Ok(trimmed.to_owned())
}

fn anthropic_root(base: &str) -> String {
    if base.ends_with("/v1") {
        base.to_owned()
    } else {
        format!("{base}/v1")
    }
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .unwrap_or_default()
}

fn request(
    client: &reqwest::Client,
    channel: &AiChannel,
    key: &str,
    method: reqwest::Method,
    url: String,
) -> reqwest::RequestBuilder {
    let mut builder = client.request(method, url);
    builder = match channel.api {
        AiApi::AnthropicMessages => builder
            .header("x-api-key", key)
            .header("anthropic-version", ANTHROPIC_VERSION),
        AiApi::OpenaiCompletions | AiApi::OpenaiResponses => builder.bearer_auth(key),
    };
    for (name, value) in &channel.headers {
        builder = builder.header(name, value);
    }
    builder
}

async fn send(builder: reqwest::RequestBuilder) -> Result<Value, AiError> {
    let response = builder
        .send()
        .await
        .map_err(|error| AiError::Transport(error.to_string()))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| AiError::Transport(error.to_string()))?;
    if !status.is_success() {
        return Err(AiError::Rejected {
            status: status.as_u16(),
            body: body.chars().take(ERROR_EXCERPT).collect(),
        });
    }
    serde_json::from_str(&body).map_err(|_| {
        AiError::Malformed(format!(
            "not JSON: {}",
            body.chars().take(ERROR_EXCERPT).collect::<String>()
        ))
    })
}

/// Ask `model` once and return its text answer.
pub async fn complete(
    channel: &AiChannel,
    key: &str,
    model: &str,
    chat: ChatRequest<'_>,
) -> Result<String, AiError> {
    let base = normalize_base_url(&channel.base_url).map_err(AiError::Malformed)?;
    let client = http();
    let post = |url: String| request(&client, channel, key, reqwest::Method::POST, url);
    let text = match channel.api {
        AiApi::OpenaiCompletions => {
            let mut body = json!({
                "model": model,
                "messages": [
                    { "role": "system", "content": chat.system },
                    { "role": "user", "content": chat.user }
                ]
            });
            if let Some(max_tokens) = chat.max_tokens {
                body["max_tokens"] = json!(max_tokens);
            }
            let reply = send(post(format!("{base}/chat/completions")).json(&body)).await?;
            completions_text(&reply)
        }
        AiApi::OpenaiResponses => {
            let mut body = json!({
                "model": model,
                "instructions": chat.system,
                "input": chat.user
            });
            if let Some(max_tokens) = chat.max_tokens {
                body["max_output_tokens"] = json!(max_tokens);
            }
            let reply = send(post(format!("{base}/responses")).json(&body)).await?;
            responses_text(&reply)
        }
        AiApi::AnthropicMessages => {
            let body = json!({
                "model": model,
                "max_tokens": chat.max_tokens.unwrap_or(ANTHROPIC_DEFAULT_MAX_TOKENS),
                "system": chat.system,
                "messages": [{ "role": "user", "content": chat.user }]
            });
            let reply =
                send(post(format!("{}/messages", anthropic_root(&base))).json(&body)).await?;
            anthropic_text(&reply)
        }
    };
    text.filter(|text| !text.trim().is_empty())
        .ok_or_else(|| AiError::Malformed("the answer has no text".into()))
}

/// Models the endpoint serves (`GET …/models`), sorted.
pub async fn list_models(channel: &AiChannel, key: &str) -> Result<Vec<String>, AiError> {
    let base = normalize_base_url(&channel.base_url).map_err(AiError::Malformed)?;
    let url = match channel.api {
        AiApi::AnthropicMessages => format!("{}/models?limit=1000", anthropic_root(&base)),
        AiApi::OpenaiCompletions | AiApi::OpenaiResponses => format!("{base}/models"),
    };
    let reply = send(request(&http(), channel, key, reqwest::Method::GET, url)).await?;
    let entries = reply
        .get("data")
        .or_else(|| reply.get("models"))
        .and_then(Value::as_array)
        .or_else(|| reply.as_array())
        .ok_or_else(|| AiError::Malformed("no model list".into()))?;
    let mut models = entries
        .iter()
        .filter_map(|entry| {
            entry
                .get("id")
                .or_else(|| entry.get("name"))
                .and_then(Value::as_str)
                .or_else(|| entry.as_str())
                .map(ToOwned::to_owned)
        })
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    Ok(models)
}

fn completions_text(reply: &Value) -> Option<String> {
    let content = reply.pointer("/choices/0/message/content")?;
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => Some(
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(""),
        ),
        _ => None,
    }
}

fn responses_text(reply: &Value) -> Option<String> {
    if let Some(text) = reply.get("output_text").and_then(Value::as_str) {
        return Some(text.to_owned());
    }
    let text = reply
        .get("output")?
        .as_array()?
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
        .flat_map(|item| {
            item.get("content")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("output_text"))
        .filter_map(|part| {
            part.get("text")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .collect::<Vec<_>>()
        .join("");
    Some(text)
}

fn anthropic_text(reply: &Value) -> Option<String> {
    Some(
        reply
            .get("content")?
            .as_array()?
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use axum::{
        http::{HeaderMap, StatusCode, Uri},
        response::IntoResponse,
        routing::any,
        Json, Router,
    };
    use std::sync::{Arc, Mutex};

    type Seen = Arc<Mutex<Vec<(String, HeaderMap, Value)>>>;

    /// A provider that records each request and answers by path.
    pub(crate) async fn provider(answers: Vec<(&'static str, Value)>) -> (String, Seen) {
        let seen: Seen = Arc::default();
        let record = Arc::clone(&seen);
        let answers = Arc::new(answers);
        let router = Router::new().fallback(any(
            move |uri: Uri, headers: HeaderMap, body: axum::body::Bytes| {
                let record = Arc::clone(&record);
                let answers = Arc::clone(&answers);
                async move {
                    let path = uri.path().to_owned();
                    let body = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    record.lock().unwrap().push((path.clone(), headers, body));
                    match answers.iter().find(|(route, _)| *route == path) {
                        Some((_, answer)) => Json(answer.clone()).into_response(),
                        None => (StatusCode::NOT_FOUND, "no such route").into_response(),
                    }
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (format!("http://{address}"), seen)
    }

    pub(crate) fn channel(api: AiApi, base_url: String) -> AiChannel {
        AiChannel {
            id: "c".into(),
            name: "c".into(),
            api,
            base_url,
            enabled: true,
            models: Vec::new(),
            headers: [("x-relay".to_owned(), "1".to_owned())].into(),
        }
    }

    fn chat() -> ChatRequest<'static> {
        ChatRequest {
            system: "be brief",
            user: "hi",
            max_tokens: Some(64),
        }
    }

    fn uncapped() -> ChatRequest<'static> {
        ChatRequest {
            max_tokens: None,
            ..chat()
        }
    }

    #[tokio::test]
    async fn openai_chat_completions_round_trip() {
        let (base, seen) = provider(vec![(
            "/v1/chat/completions",
            json!({"choices": [{"message": {"content": "hello"}}]}),
        )])
        .await;
        let channel = channel(AiApi::OpenaiCompletions, format!("{base}/v1/"));
        let text = complete(&channel, "sk-1", "gpt-5", chat()).await.unwrap();
        assert_eq!(text, "hello");
        let (_, headers, body) = seen.lock().unwrap()[0].clone();
        assert_eq!(headers["authorization"], "Bearer sk-1");
        assert_eq!(headers["x-relay"], "1");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "hi");
        assert_eq!(body["model"], "gpt-5");
        assert_eq!(body["max_tokens"], 64);

        complete(&channel, "sk-1", "gpt-5", uncapped())
            .await
            .unwrap();
        let (_, _, body) = seen.lock().unwrap()[1].clone();
        assert!(body.get("max_tokens").is_none(), "{body}");
    }

    #[tokio::test]
    async fn openai_responses_round_trip() {
        let (base, seen) = provider(vec![(
            "/v1/responses",
            json!({"output": [
                {"type": "reasoning", "content": []},
                {"type": "message", "content": [{"type": "output_text", "text": "hel"}, {"type": "output_text", "text": "lo"}]}
            ]}),
        )])
        .await;
        let channel = channel(AiApi::OpenaiResponses, format!("{base}/v1"));
        assert_eq!(
            complete(&channel, "sk-1", "gpt-5", chat()).await.unwrap(),
            "hello"
        );
        let (_, _, body) = seen.lock().unwrap()[0].clone();
        assert_eq!(body["instructions"], "be brief");
        assert_eq!(body["input"], "hi");
        assert_eq!(body["max_output_tokens"], 64);

        complete(&channel, "sk-1", "gpt-5", uncapped())
            .await
            .unwrap();
        let (_, _, body) = seen.lock().unwrap()[1].clone();
        assert!(body.get("max_output_tokens").is_none(), "{body}");
    }

    #[tokio::test]
    async fn anthropic_messages_round_trip_with_and_without_v1() {
        let (base, seen) = provider(vec![(
            "/v1/messages",
            json!({"content": [{"type": "thinking", "thinking": "…"}, {"type": "text", "text": "hello"}]}),
        )])
        .await;
        for base_url in [base.clone(), format!("{base}/v1")] {
            let channel = channel(AiApi::AnthropicMessages, base_url);
            assert_eq!(
                complete(&channel, "ak-1", "claude-sonnet-5", chat())
                    .await
                    .unwrap(),
                "hello"
            );
        }
        let (_, headers, body) = seen.lock().unwrap()[0].clone();
        assert_eq!(headers["x-api-key"], "ak-1");
        assert_eq!(headers["anthropic-version"], ANTHROPIC_VERSION);
        assert!(headers.get("authorization").is_none());
        assert_eq!(body["system"], "be brief");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["max_tokens"], 64);

        let channel = channel(AiApi::AnthropicMessages, base);
        complete(&channel, "ak-1", "claude-sonnet-5", uncapped())
            .await
            .unwrap();
        let (_, _, body) = seen.lock().unwrap().last().unwrap().clone();
        assert_eq!(body["max_tokens"], ANTHROPIC_DEFAULT_MAX_TOKENS);
    }

    #[tokio::test]
    async fn lists_models_in_every_common_shape() {
        let (base, _) = provider(vec![
            (
                "/v1/models",
                json!({"data": [{"id": "b"}, {"id": "a"}, {"id": "a"}]}),
            ),
            ("/alt/models", json!({"models": [{"name": "z"}]})),
        ])
        .await;
        let openai = channel(AiApi::OpenaiCompletions, format!("{base}/v1"));
        assert_eq!(list_models(&openai, "k").await.unwrap(), ["a", "b"]);
        let anthropic = channel(AiApi::AnthropicMessages, base.clone());
        assert_eq!(list_models(&anthropic, "k").await.unwrap(), ["a", "b"]);
        let other = channel(AiApi::OpenaiResponses, format!("{base}/alt"));
        assert_eq!(list_models(&other, "k").await.unwrap(), ["z"]);
    }

    #[tokio::test]
    async fn provider_errors_keep_status_and_a_body_excerpt() {
        let (base, _) = provider(Vec::new()).await;
        let channel = channel(AiApi::OpenaiCompletions, base);
        let error = complete(&channel, "k", "m", chat()).await.unwrap_err();
        assert_eq!(
            error,
            AiError::Rejected {
                status: 404,
                body: "no such route".into()
            }
        );
    }
}
