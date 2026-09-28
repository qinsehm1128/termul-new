use std::{collections::BTreeMap, time::Duration};

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{AiChannelsDocument, AiRoutePurpose};

const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 8_192;

#[derive(Debug, Clone)]
pub struct AiCompletionRequest {
    pub purpose: AiRoutePurpose,
    pub system: String,
    pub user: String,
    pub response_format: Option<Value>,
    pub cancellation: CancellationToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiCompletionResponse {
    pub text: String,
    pub profile_id: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiRouterError {
    InvalidRoute,
    MissingCredential(String),
    ProviderRejected { status: u16 },
    Timeout,
    Cancelled,
    Transport,
    MalformedResponse,
    NoProviderSucceeded { attempted_profiles: Vec<String> },
}

impl std::fmt::Display for AiRouterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRoute => f.write_str("AI route is invalid"),
            Self::MissingCredential(_) => f.write_str("AI provider credential is missing"),
            Self::ProviderRejected { status } => {
                write!(f, "AI provider rejected the request ({status})")
            }
            Self::Timeout => f.write_str("AI provider request timed out"),
            Self::Cancelled => f.write_str("AI request was cancelled"),
            Self::Transport => f.write_str("AI provider transport failed"),
            Self::MalformedResponse => f.write_str("AI provider response was malformed"),
            Self::NoProviderSucceeded { .. } => f.write_str("no AI provider succeeded"),
        }
    }
}

impl std::error::Error for AiRouterError {}

#[derive(Debug, Clone)]
pub struct AiRouter {
    client: Client,
    max_response_bytes: usize,
}

impl Default for AiRouter {
    fn default() -> Self {
        Self::new(Client::new(), 4 * 1024 * 1024)
    }
}

impl AiRouter {
    pub fn new(client: Client, max_response_bytes: usize) -> Self {
        Self {
            client,
            max_response_bytes: max_response_bytes.max(1),
        }
    }

    pub async fn complete(
        &self,
        document: &AiChannelsDocument,
        credentials: &BTreeMap<String, String>,
        request: AiCompletionRequest,
    ) -> Result<AiCompletionResponse, AiRouterError> {
        document
            .validate()
            .map_err(|_| AiRouterError::InvalidRoute)?;
        let route = document
            .routes
            .iter()
            .find(|route| route.purpose == request.purpose)
            .ok_or(AiRouterError::InvalidRoute)?;
        let profiles = route
            .profile_ids
            .iter()
            .filter_map(|id| document.profiles.iter().find(|profile| &profile.id == id))
            .filter(|profile| profile.enabled)
            .collect::<Vec<_>>();
        if profiles.is_empty() {
            return Err(AiRouterError::InvalidRoute);
        }

        let mut attempted = Vec::new();
        let max_attempts = route.max_attempts.min(profiles.len() as u32) as usize;
        for profile in profiles.into_iter().take(max_attempts) {
            attempted.push(profile.id.clone());
            let channel = document
                .channels
                .iter()
                .find(|channel| channel.id == profile.channel_id && channel.enabled)
                .ok_or(AiRouterError::InvalidRoute)?;
            let credential = credentials
                .get(&channel.id)
                .ok_or_else(|| AiRouterError::MissingCredential(channel.id.clone()))?;
            match self
                .request_profile(channel, profile, credential, &request, route.timeout_ms)
                .await
            {
                Ok(response) => return Ok(response),
                Err(AiRouterError::ProviderRejected { status })
                    if is_retryable_status(
                        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    ) => {}
                Err(AiRouterError::Transport | AiRouterError::Timeout) => {}
                Err(error) => return Err(error),
            }
        }
        Err(AiRouterError::NoProviderSucceeded {
            attempted_profiles: attempted,
        })
    }

    async fn request_profile(
        &self,
        channel: &super::AiChannel,
        profile: &super::AiModelProfile,
        credential: &str,
        request: &AiCompletionRequest,
        timeout_ms: u32,
    ) -> Result<AiCompletionResponse, AiRouterError> {
        let endpoint = endpoint_for(channel)?;
        let body = ChatRequest {
            model: profile.model_id.clone(),
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: request.system.clone(),
                },
                ChatMessage {
                    role: "user",
                    content: request.user.clone(),
                },
            ],
            temperature: profile.temperature,
            max_tokens: Some(
                profile
                    .max_output_tokens
                    .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS),
            ),
            response_format: request.response_format.clone(),
        };
        let send = self
            .client
            .post(endpoint)
            .bearer_auth(credential)
            .json(&body)
            .send();
        let response = tokio::select! {
            _ = request.cancellation.cancelled() => return Err(AiRouterError::Cancelled),
            result = tokio::time::timeout(Duration::from_millis(timeout_ms as u64), send) => {
                result.map_err(|_| AiRouterError::Timeout)?.map_err(|_| AiRouterError::Transport)?
            }
        };
        let status = response.status();
        let bytes = tokio::select! {
            _ = request.cancellation.cancelled() => return Err(AiRouterError::Cancelled),
            result = response.bytes() => result.map_err(|_| AiRouterError::Transport)?,
        };
        if bytes.len() > self.max_response_bytes {
            return Err(AiRouterError::MalformedResponse);
        }
        if !status.is_success() {
            return Err(AiRouterError::ProviderRejected {
                status: status.as_u16(),
            });
        }
        let parsed: ChatResponse =
            serde_json::from_slice(&bytes).map_err(|_| AiRouterError::MalformedResponse)?;
        let text = parsed
            .choices
            .first()
            .and_then(|choice| choice.message.content.clone())
            .filter(|value| !value.is_empty())
            .ok_or(AiRouterError::MalformedResponse)?;
        Ok(AiCompletionResponse {
            text,
            profile_id: profile.id.clone(),
            model: profile.model_id.clone(),
        })
    }
}

fn endpoint_for(channel: &super::AiChannel) -> Result<String, AiRouterError> {
    let base = match channel.base_url.as_deref() {
        Some(base) => base,
        None => match channel.provider {
            super::AiProviderKind::VercelGateway => "https://ai-gateway.vercel.sh/v1",
            super::AiProviderKind::OpenRouter => "https://openrouter.ai/api/v1",
            super::AiProviderKind::Ollama => "http://127.0.0.1:11434/v1",
            _ => return Err(AiRouterError::InvalidRoute),
        },
    };
    if base.contains('?') || base.contains('#') || base.contains('@') {
        return Err(AiRouterError::InvalidRoute);
    }
    Ok(format!("{}/chat/completions", base.trim_end_matches('/')))
}

fn is_retryable_status(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(rename = "max_tokens", skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<Value>,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: &'static str,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessageResponse,
}

#[derive(Debug, Deserialize)]
struct ChatMessageResponse {
    content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::JoinHandle,
    };

    async fn spawn_response(
        status: u16,
        body: &'static str,
        delay: Duration,
    ) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::with_capacity(4096);
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            let header_end = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4)
                .unwrap_or(request.len());
            let header_text = String::from_utf8_lossy(&request[..header_end]);
            let content_length = header_text
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length:")
                        .or_else(|| line.strip_prefix("Content-Length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            while request.len() < header_end + content_length {
                let read = stream.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            tokio::time::sleep(delay).await;
            let response = format!(
                "HTTP/1.1 {status} test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        });
        (format!("http://{address}/v1"), task)
    }

    fn document(endpoints: Vec<String>, max_attempts: u32) -> AiChannelsDocument {
        let mut channels = Vec::new();
        let mut profiles = Vec::new();
        let mut profile_ids = Vec::new();
        for (index, endpoint) in endpoints.into_iter().enumerate() {
            let channel_id = format!("channel{index}");
            let profile_id = format!("profile{index}");
            channels.push(super::super::AiChannel {
                id: channel_id.clone(),
                display_name: channel_id.clone(),
                provider: super::super::AiProviderKind::Custom,
                base_url: Some(endpoint),
                enabled: true,
                credential_ref: super::super::AiCredentialRef {
                    kind: super::super::AiCredentialKind::Keyring,
                    reference: format!("ai/{channel_id}"),
                    has_credential: true,
                },
                model_ids: vec!["model".into()],
            });
            profiles.push(super::super::AiModelProfile {
                id: profile_id.clone(),
                channel_id,
                model_id: "model".into(),
                enabled: true,
                capabilities: super::super::AiModelCapabilities {
                    description_analysis: true,
                    fx_runtime: true,
                    structured_output: true,
                    tool_calling: false,
                },
                max_output_tokens: Some(32),
                temperature: Some(0.0),
            });
            profile_ids.push(profile_id);
        }
        AiChannelsDocument {
            schema_version: super::super::AI_CHANNELS_SCHEMA_VERSION,
            revision: 1,
            channels,
            profiles,
            routes: vec![
                super::super::AiRoute {
                    purpose: AiRoutePurpose::DescriptionAnalysis,
                    profile_ids,
                    max_attempts,
                    timeout_ms: 100,
                },
                super::super::AiRoute {
                    purpose: AiRoutePurpose::FxRuntime,
                    profile_ids: vec!["profile0".into()],
                    max_attempts: 1,
                    timeout_ms: 100,
                },
            ],
        }
    }

    fn request(purpose: AiRoutePurpose) -> AiCompletionRequest {
        AiCompletionRequest {
            purpose,
            system: "system".into(),
            user: "user".into(),
            response_format: None,
            cancellation: CancellationToken::new(),
        }
    }

    #[test]
    fn rejects_unsafe_endpoint_forms() {
        let channel = super::super::AiChannel {
            id: "custom".into(),
            display_name: "Custom".into(),
            provider: super::super::AiProviderKind::Custom,
            base_url: Some("https://user:pass@example.test/v1".into()),
            enabled: true,
            credential_ref: super::super::AiCredentialRef {
                kind: super::super::AiCredentialKind::Keyring,
                reference: "ai/custom".into(),
                has_credential: true,
            },
            model_ids: vec!["model".into()],
        };
        assert!(matches!(
            endpoint_for(&channel),
            Err(AiRouterError::InvalidRoute)
        ));
    }

    #[test]
    fn retryable_statuses_are_limited() {
        assert!(is_retryable_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_retryable_status(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(!is_retryable_status(StatusCode::BAD_REQUEST));
        assert!(!is_retryable_status(StatusCode::UNAUTHORIZED));
    }

    #[tokio::test]
    async fn falls_back_after_retryable_provider_failure() {
        let (first, first_task) =
            spawn_response(500, r#"{"error":"temporary"}"#, Duration::ZERO).await;
        let (second, second_task) = spawn_response(
            200,
            r#"{"choices":[{"message":{"content":"ok"}}]}"#,
            Duration::ZERO,
        )
        .await;
        let router = AiRouter::new(Client::new(), 4 * 1024 * 1024);
        let mut credentials = BTreeMap::new();
        credentials.insert("channel0".into(), "credential-0".into());
        credentials.insert("channel1".into(), "credential-1".into());
        let document = document(vec![first, second], 2);
        document
            .validate()
            .unwrap_or_else(|error| panic!("document invalid: {error:?}"));
        let result = router
            .complete(
                &document,
                &credentials,
                request(AiRoutePurpose::DescriptionAnalysis),
            )
            .await
            .unwrap();
        assert_eq!(result.profile_id, "profile1");
        assert_eq!(result.text, "ok");
        first_task.await.unwrap();
        second_task.await.unwrap();
    }

    #[tokio::test]
    async fn classifies_auth_invalid_request_rate_limit_timeout_and_malformed_output() {
        let router = AiRouter::new(Client::new(), 4 * 1024 * 1024);
        for (status, expected) in [
            (401, AiRouterError::ProviderRejected { status: 401 }),
            (400, AiRouterError::ProviderRejected { status: 400 }),
        ] {
            let (endpoint, server_task) =
                spawn_response(status, r#"{"error":"rejected"}"#, Duration::ZERO).await;
            let mut credentials = BTreeMap::new();
            credentials.insert("channel0".into(), "credential-0".into());
            let result = router
                .complete(
                    &document(vec![endpoint], 1),
                    &credentials,
                    request(AiRoutePurpose::DescriptionAnalysis),
                )
                .await;
            assert_eq!(result, Err(expected));
            server_task.await.unwrap();
        }
        let (malformed_endpoint, malformed_task) =
            spawn_response(200, r#"{"choices":[]}"#, Duration::ZERO).await;
        let mut credentials = BTreeMap::new();
        credentials.insert("channel0".into(), "credential-0".into());
        let malformed = router
            .complete(
                &document(vec![malformed_endpoint], 1),
                &credentials,
                request(AiRoutePurpose::DescriptionAnalysis),
            )
            .await;
        assert_eq!(malformed, Err(AiRouterError::MalformedResponse));
        malformed_task.await.unwrap();

        let (timeout_endpoint, timeout_task) = spawn_response(
            200,
            r#"{"choices":[{"message":{"content":"late"}}]}"#,
            Duration::from_millis(200),
        )
        .await;
        let timeout = router
            .complete(
                &document(vec![timeout_endpoint], 1),
                &credentials,
                request(AiRoutePurpose::DescriptionAnalysis),
            )
            .await;
        assert_eq!(
            timeout,
            Err(AiRouterError::NoProviderSucceeded {
                attempted_profiles: vec!["profile0".into()]
            })
        );
        timeout_task.await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_interrupts_response_body() {
        let (endpoint, server_task) = spawn_response(
            200,
            r#"{"choices":[{"message":{"content":"late"}}]}"#,
            Duration::from_millis(200),
        )
        .await;
        let router = AiRouter::new(Client::new(), 4 * 1024 * 1024);
        let token = CancellationToken::new();
        let cancellation = token.clone();
        let document = document(vec![endpoint], 1);
        let mut credentials = BTreeMap::new();
        credentials.insert("channel0".into(), "credential-0".into());
        let task = tokio::spawn(async move {
            router
                .complete(
                    &document,
                    &credentials,
                    AiCompletionRequest {
                        cancellation,
                        ..request(AiRoutePurpose::DescriptionAnalysis)
                    },
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        token.cancel();
        assert!(matches!(task.await.unwrap(), Err(AiRouterError::Cancelled)));
        server_task.await.unwrap();
    }
}
