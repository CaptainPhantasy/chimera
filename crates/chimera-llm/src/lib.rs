//! chimera-llm: LLM provider abstraction with OpenAI-compatible streaming.
//!
//! Provides a provider-agnostic `LlmProvider` trait plus a concrete
//! `OpenAiCompatibleProvider` that works with any OpenAI Chat Completions
//! endpoint (OpenAI, Together, Groq, LM Studio, Ollama, Portkey, etc.).

use std::collections::HashMap;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("provider returned status {status}: {body}")]
    Provider { status: u16, body: String },
    #[error("malformed provider response: {0}")]
    Malformed(String),
    #[error("stream ended unexpectedly")]
    StreamEnd,
    #[error("configuration error: {0}")]
    Config(String),
    #[error("provider request timed out while {0}")]
    Timeout(&'static str),
    #[error("incomplete provider response: {0}")]
    Incomplete(String),
}

impl LlmError {
    pub fn is_retryable(&self) -> bool {
        match self {
            LlmError::Network(_) => true,
            LlmError::Provider { status, .. } => *status >= 500 || *status == 429,
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Message types
// ---------------------------------------------------------------------------

/// A role in the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A single message in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Option<String>,
    /// Tool calls requested by the assistant.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_calls: Vec<ToolCall>,
    /// When role == Tool, the id of the tool call this responds to.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_call_id: Option<String>,
    /// Optional name label for the message source.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant_with_tools(content: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content,
            tool_calls,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn tool_result(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: Some(call_id.into()),
            name: None,
        }
    }
}

/// A tool/function available to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Type discriminator — always "function" for OpenAI compat.
    #[serde(rename = "type", default = "default_function_type")]
    pub kind: String,
    pub function: FunctionDefinition,
}

fn default_function_type() -> String {
    "function".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema for parameters, as a serde_json::Value.
    pub parameters: serde_json::Value,
}

/// A tool call requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique id assigned by the provider.
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// Raw JSON arguments string from the provider.
    pub arguments: String,
}

// ---------------------------------------------------------------------------
// Completion request / response
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tools: Vec<ToolDefinition>,
    /// Optional tool-choice hint ("auto", "none", or a specific tool name).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub top_p: Option<f32>,
    /// Provider-specific stop sequences.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub stop: Vec<String>,
    /// Extra provider-specific fields forwarded verbatim.
    #[serde(flatten, default)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    /// Provider-reported finish reason.
    pub finish_reason: Option<String>,
    /// Usage stats if reported.
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

// ---------------------------------------------------------------------------
// Streaming deltas
// ---------------------------------------------------------------------------

/// A delta emitted during streaming.
#[derive(Debug, Clone)]
pub enum StreamDelta {
    /// Incremental text token.
    Text(String),
    /// The model emitted a complete tool call (accumulated).
    ToolCall(ToolCall),
    /// The model finished. Includes finish reason and optional usage.
    Done {
        finish_reason: Option<String>,
        usage: Option<Usage>,
    },
    /// A raw error mid-stream.
    Error(std::sync::Arc<LlmError>),
}

// ---------------------------------------------------------------------------

/// A large-language-model provider.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Human-readable provider name.
    fn name(&self) -> &str;

    /// Non-streaming completion.
    async fn complete(&self, req: &CompletionRequest) -> Result<CompletionResponse, LlmError>;

    /// Streaming completion. Yields deltas until a `Done` or `Error` is
    /// produced. Default impl falls back to `complete`.
    async fn stream(
        &self,
        req: &CompletionRequest,
    ) -> Result<Vec<StreamDelta>, LlmError> {
        let resp = self.complete(req).await?;
        let mut deltas = Vec::new();
        if let Some(text) = &resp.content {
            if !text.is_empty() {
                deltas.push(StreamDelta::Text(text.clone()));
            }
        }
        for tc in resp.tool_calls.iter().cloned() {
            deltas.push(StreamDelta::ToolCall(tc));
        }
        deltas.push(StreamDelta::Done {
            finish_reason: resp.finish_reason.clone(),
            usage: resp.usage.clone(),
        });
        Ok(deltas)
    }

    /// Deliver each fragment as it arrives. Legacy providers remain compatible.
    /// Dropping this future cancels the request; no detached task is created.
    async fn stream_live(
        &self,
        req: &CompletionRequest,
        on_delta: &mut (dyn FnMut(StreamDelta) + Send),
    ) -> Result<(), LlmError> {
        for delta in self.stream(req).await? {
            on_delta(delta);
        }
        Ok(())
    }

}

// ---------------------------------------------------------------------------
// OpenAI-compatible provider
// ---------------------------------------------------------------------------

/// Limits apply to one provider request, including its complete response body.
#[derive(Debug, Clone, Copy)]
pub struct RequestLimits {
    pub total: std::time::Duration,
    pub idle: std::time::Duration,
    pub max_response_bytes: usize,
}

impl Default for RequestLimits {
    fn default() -> Self {
        Self {
            total: std::time::Duration::from_secs(120),
            idle: std::time::Duration::from_secs(30),
            max_response_bytes: 8 * 1024 * 1024,
        }
    }
}

/// Parse and validate an endpoint without using substring-based local checks.
pub fn base_url_is_local(base: &str) -> Result<bool, LlmError> {
    let url = reqwest::Url::parse(base)
        .map_err(|_| LlmError::Config("base_url must be an absolute HTTP or HTTPS URL".into()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none()
        || !url.username().is_empty() || url.password().is_some()
        || url.query().is_some() || url.fragment().is_some()
    {
        return Err(LlmError::Config("base_url needs an HTTP(S) host without credentials, query, or fragment".into()));
    }
    let host = url.host_str().unwrap_or_default();
    let ip_host = host.trim_start_matches('[').trim_end_matches(']');
    Ok(host.eq_ignore_ascii_case("localhost")
        || ip_host.parse::<std::net::IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false))
}

/// A provider that speaks the OpenAI Chat Completions HTTP API.
pub struct OpenAiCompatibleProvider {
    name: String,
    base_url: String,
    api_key: String,
    client: reqwest::Client,
    limits: RequestLimits,
}

impl OpenAiCompatibleProvider {
    pub fn new(name: impl Into<String>, base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            client: reqwest::Client::new(),
            limits: RequestLimits::default(),
        }
    }

    pub fn with_limits(mut self, limits: RequestLimits) -> Self {
        self.limits = limits;
        self
    }

    fn redact_error(&self, error: LlmError) -> LlmError {
        let clean = |text: String| if self.api_key.is_empty() { text } else { text.replace(&self.api_key, "[REDACTED]") };
        match error {
            LlmError::Provider { status, body } => LlmError::Provider { status, body: clean(body) },
            LlmError::Incomplete(text) => LlmError::Incomplete(clean(text)),
            LlmError::Malformed(text) => LlmError::Malformed(clean(text)),
            other => other,
        }
    }

    fn chat_url(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    async fn send(&self, req: &CompletionRequest, stream: bool) -> Result<reqwest::Response, LlmError> {
        base_url_is_local(&self.base_url)?;
        if self.limits.total.is_zero() || self.limits.idle.is_zero() || self.limits.max_response_bytes == 0 {
            return Err(LlmError::Config("request limits must be positive".into()));
        }
        let mut body = serde_json::to_value(req).map_err(|e| LlmError::Malformed(e.to_string()))?;
        body["stream"] = serde_json::Value::Bool(stream);
        if stream && body.get("stream_options").is_none() {
            body["stream_options"] = serde_json::json!({"include_usage": true});
        }
        let mut request = self.client.post(self.chat_url()).json(&body);
        if !self.api_key.is_empty() {
            request = request.bearer_auth(&self.api_key);
        }
        Ok(request.send().await?)
    }

    async fn body_bytes(&self, resp: reqwest::Response) -> Result<Vec<u8>, LlmError> {
        let mut chunks = resp.bytes_stream();
        let mut result = Vec::new();
        while let Some(chunk) = tokio::time::timeout(self.limits.idle, chunks.next())
            .await.map_err(|_| LlmError::Timeout("waiting for response data"))?
        {
            let chunk = chunk?;
            if result.len().saturating_add(chunk.len()) > self.limits.max_response_bytes {
                return Err(LlmError::Malformed("response exceeds configured byte limit".into()));
            }
            result.extend_from_slice(&chunk);
        }
        Ok(result)
    }

    async fn check_status(&self, resp: reqwest::Response) -> Result<reqwest::Response, LlmError> {
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status().as_u16();
        let bytes = self.body_bytes(resp).await?;
        let body = String::from_utf8_lossy(&bytes).into_owned();
        // A service can echo an authorization token in an error. Never repeat it.
        let body = if self.api_key.is_empty() { body } else { body.replace(&self.api_key, "[REDACTED]") };
        Err(LlmError::Provider { status, body })
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    fn name(&self) -> &str { &self.name }

    async fn complete(&self, req: &CompletionRequest) -> Result<CompletionResponse, LlmError> {
        tokio::time::timeout(self.limits.total, async {
            let resp = self.check_status(self.send(req, false).await?).await?;
            let bytes = self.body_bytes(resp).await?;
            let parsed: ChatCompletionResponse = serde_json::from_slice(&bytes)
                .map_err(|e| LlmError::Malformed(e.to_string()))?;
            let result = parsed.into_response();
            validate_response(&result)?;
            Ok(result)
        }).await.map_err(|_| LlmError::Timeout("completing the request"))?
    }

    async fn stream(&self, req: &CompletionRequest) -> Result<Vec<StreamDelta>, LlmError> {
        let mut deltas = Vec::new();
        self.stream_live(req, &mut |delta| deltas.push(delta)).await?;
        Ok(deltas)
    }

    async fn stream_live(
        &self,
        req: &CompletionRequest,
        on_delta: &mut (dyn FnMut(StreamDelta) + Send),
    ) -> Result<(), LlmError> {
        tokio::time::timeout(self.limits.total, async {
            let resp = self.check_status(self.send(req, true).await?).await?;
            let mut chunks = resp.bytes_stream();
            let mut parser = SseDecoder::default();
            let mut received = 0usize;
            while let Some(chunk) = tokio::time::timeout(self.limits.idle, chunks.next())
                .await.map_err(|_| LlmError::Timeout("waiting for streamed data"))?
            {
                let bytes = chunk?;
                received = received.saturating_add(bytes.len());
                if received > self.limits.max_response_bytes {
                    return Err(LlmError::Malformed("response exceeds configured byte limit".into()));
                }
                parser.push(&bytes, on_delta)?;
                if parser.ended { break; }
            }
            parser.finish(on_delta)
        }).await.map_err(|_| LlmError::Timeout("completing the request"))?
            .map_err(|error| self.redact_error(error))
    }
}

/// Shared validation also protects the agent from custom provider implementations.
pub fn validate_response(response: &CompletionResponse) -> Result<(), LlmError> {
    match response.finish_reason.as_deref() {
        Some("stop") if response.tool_calls.is_empty() => {
            if response.content.as_deref().unwrap_or_default().trim().is_empty() {
                return Err(LlmError::Malformed("provider returned an empty answer".into()));
            }
        }
        Some("tool_calls") if !response.tool_calls.is_empty() => {}
        Some(reason) => return Err(LlmError::Incomplete(format!("finish reason: {reason}"))),
        None => return Err(LlmError::StreamEnd),
    }
    let mut ids = std::collections::HashSet::new();
    for call in &response.tool_calls {
        if call.id.is_empty() || call.function.name.is_empty() || call.kind != "function" || !ids.insert(&call.id) {
            return Err(LlmError::Malformed("invalid or duplicate tool call identity".into()));
        }
        let args: serde_json::Value = serde_json::from_str(&call.function.arguments)
            .map_err(|_| LlmError::Malformed(format!("invalid arguments for tool {}", call.function.name)))?;
        if !args.is_object() {
            return Err(LlmError::Malformed("tool arguments must be an object".into()));
        }
    }
    Ok(())
}

/// Buffers bytes until complete lines, preserving UTF-8 across arbitrary network splits.
#[derive(Default)]
struct SseDecoder {
    pending: Vec<u8>,
    event_data: Vec<String>,
    tools: std::collections::BTreeMap<u32, (String, String, String)>,
    text: String,
    finish_reason: Option<String>,
    usage: Option<Usage>,
    ended: bool,
}

impl SseDecoder {
    fn push(&mut self, bytes: &[u8], emit: &mut (dyn FnMut(StreamDelta) + Send)) -> Result<(), LlmError> {
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let line = std::str::from_utf8(&line[..line.len() - 1])
                .map_err(|_| LlmError::Malformed("stream contains invalid UTF-8".into()))?
                .trim_end_matches('\r');
            if line.is_empty() {
                self.event(emit)?;
                if self.ended { self.pending.clear(); break; }
            } else if let Some(data) = line.strip_prefix("data:") {
                self.event_data.push(data.strip_prefix(' ').unwrap_or(data).to_string());
            }
        }
        Ok(())
    }

    fn event(&mut self, emit: &mut (dyn FnMut(StreamDelta) + Send)) -> Result<(), LlmError> {
        if self.event_data.is_empty() { return Ok(()); }
        let data = std::mem::take(&mut self.event_data).join("\n");
        if data.trim() == "[DONE]" { self.ended = true; return Ok(()); }
        let value: serde_json::Value = serde_json::from_str(&data)
            .map_err(|e| LlmError::Malformed(format!("invalid stream event: {e}")))?;
        if let Some(error) = value.get("error") {
            let detail = error.get("message").and_then(|v| v.as_str()).unwrap_or("provider reported an error during streaming");
            return Err(LlmError::Incomplete(detail.to_string()));
        }
        if let Some(usage) = value.get("usage").filter(|v| v.get("prompt_tokens").and_then(|n| n.as_u64()).is_some() && v.get("completion_tokens").and_then(|n| n.as_u64()).is_some()) { self.usage = Some(parse_usage(usage)); }
        let choices = value.get("choices").and_then(|v| v.as_array())
            .ok_or_else(|| LlmError::Malformed("stream event has no choices".into()))?;
        let Some(choice) = choices.first() else { return Ok(()); };
        if let Some(delta) = choice.get("delta") {
            if let Some(text) = delta.get("content").and_then(|v| v.as_str()) {
                if self.finish_reason.is_some() { return Err(LlmError::Malformed("text after finish reason".into())); }
                self.text.push_str(text);
                emit(StreamDelta::Text(text.to_string()));
            }
            if let Some(calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
                for call in calls {
                    if self.finish_reason.is_some() { return Err(LlmError::Malformed("tool call after finish reason".into())); }
                    let index = call.get("index").and_then(|v| v.as_u64()).filter(|i| *i < 1024)
                        .ok_or_else(|| LlmError::Malformed("missing or excessive tool index".into()))? as u32;
                    let entry = self.tools.entry(index).or_default();
                    if let Some(id) = call.get("id").and_then(|v| v.as_str()) { entry.0.push_str(id); }
                    if let Some(function) = call.get("function") {
                        if let Some(name) = function.get("name").and_then(|v| v.as_str()) { entry.1.push_str(name); }
                        if let Some(args) = function.get("arguments").and_then(|v| v.as_str()) { entry.2.push_str(args); }
                    }
                }
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
            if self.finish_reason.replace(reason.to_string()).is_some() {
                return Err(LlmError::Malformed("duplicate finish reason".into()));
            }
        }
        Ok(())
    }

    fn finish(mut self, emit: &mut (dyn FnMut(StreamDelta) + Send)) -> Result<(), LlmError> {
        if !self.pending.is_empty() || !self.event_data.is_empty() { return Err(LlmError::StreamEnd); }
        let calls = self.tools.into_values().map(|(id, name, arguments)| ToolCall {
            id, kind: "function".into(), function: FunctionCall { name, arguments },
        }).collect();
        let response = CompletionResponse { content: Some(std::mem::take(&mut self.text)), tool_calls: calls,
            finish_reason: self.finish_reason, usage: self.usage };
        validate_response(&response)?;
        for call in response.tool_calls { emit(StreamDelta::ToolCall(call)); }
        emit(StreamDelta::Done { finish_reason: response.finish_reason, usage: response.usage });
        Ok(())
    }
}

fn parse_usage(v: &serde_json::Value) -> Usage {
    Usage {
        prompt_tokens: v.get("prompt_tokens").and_then(|n| n.as_u64()).unwrap_or(0),
        completion_tokens: v
            .get("completion_tokens")
            .and_then(|n| n.as_u64())
            .unwrap_or(0),
        total_tokens: v.get("total_tokens").and_then(|n| n.as_u64()).unwrap_or(0),
    }
}

// ---------------------------------------------------------------------------
// Raw provider response shapes (OpenAI-compatible)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<RawUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: RawMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<RawToolCall>,
}

#[derive(Debug, Deserialize)]
struct RawToolCall {
    id: String,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    function: RawFunction,
}

#[derive(Debug, Deserialize)]
struct RawFunction {
    name: String,
    #[serde(default)]
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct RawUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
    #[serde(default)]
    total_tokens: Option<u64>,
}

impl ChatCompletionResponse {
    fn into_response(self) -> CompletionResponse {
        let choice = self.choices.into_iter().next();
        let (content, tool_calls, finish_reason) = match choice {
            Some(c) => {
                let tcs = c
                    .message
                    .tool_calls
                    .into_iter()
                    .map(|r| ToolCall {
                        id: r.id,
                        kind: r.kind.unwrap_or_else(|| "function".to_string()),
                        function: FunctionCall {
                            name: r.function.name,
                            arguments: r.function.arguments,
                        },
                    })
                    .collect();
                (c.message.content, tcs, c.finish_reason)
            }
            None => (None, Vec::new(), None),
        };
        let usage = self.usage.and_then(|u| Some(Usage {
            prompt_tokens: u.prompt_tokens?,
            completion_tokens: u.completion_tokens?,
            total_tokens: u.total_tokens.unwrap_or(u.prompt_tokens?.saturating_add(u.completion_tokens?)),
        }));
        CompletionResponse {
            content,
            tool_calls,
            finish_reason,
            usage,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_constructors_are_correct() {
        let s = Message::system("be helpful");
        assert_eq!(s.role, Role::System);
        assert_eq!(s.content.as_deref(), Some("be helpful"));

        let u = Message::user("hi");
        assert_eq!(u.role, Role::User);

        let a = Message::assistant_with_tools(Some("thinking".into()), vec![]);
        assert_eq!(a.role, Role::Assistant);

        let t = Message::tool_result("call_1", "42");
        assert_eq!(t.role, Role::Tool);
        assert_eq!(t.tool_call_id.as_deref(), Some("call_1"));
    }

    #[test]
    fn completion_request_serializes_cleanly() {
        let req = CompletionRequest {
            model: "gpt-4o".to_string(),
            messages: vec![Message::user("hello")],
            tools: vec![],
            tool_choice: None,
            temperature: Some(0.0),
            max_tokens: Some(128),
            top_p: None,
            stop: vec![],
            extra: HashMap::new(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"model\":\"gpt-4o\""));
        assert!(!json.contains("tool_choice")); // skipped when None
        assert!(json.contains("\"temperature\":0.0"));
    }

    #[test]
    fn tool_definition_serializes_with_type() {
        let td = ToolDefinition {
            kind: "function".to_string(),
            function: FunctionDefinition {
                name: "add".to_string(),
                description: "add two numbers".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        };
        let json = serde_json::to_string(&td).unwrap();
        assert!(json.contains("\"type\":\"function\""));
        assert!(json.contains("\"name\":\"add\""));
    }

    #[test]
    fn parse_usage_handles_missing_fields() {
        let u = parse_usage(&serde_json::json!({}));
        assert_eq!(u.prompt_tokens, 0);
        let u2 = parse_usage(&serde_json::json!({"prompt_tokens": 10, "completion_tokens": 5}));
        assert_eq!(u2.prompt_tokens, 10);
        assert_eq!(u2.completion_tokens, 5);
    }

    #[test]
    fn llm_error_retryable_classification() {
        let e = LlmError::Provider { status: 500, body: "x".into() };
        assert!(e.is_retryable());
        let e2 = LlmError::Provider { status: 400, body: "x".into() };
        assert!(!e2.is_retryable());
        let e3 = LlmError::Provider { status: 429, body: "rate".into() };
        assert!(e3.is_retryable());
    }

    fn event(value: serde_json::Value) -> Vec<u8> {
        format!("data: {value}\n\n").into_bytes()
    }

    fn text_event(text: &str) -> Vec<u8> {
        event(serde_json::json!({"choices":[{"delta":{"content":text},"finish_reason":null}]}))
    }

    fn stop_event() -> Vec<u8> {
        event(serde_json::json!({"choices":[{"delta":{},"finish_reason":"stop"}]}))
    }

    #[test]
    fn decoder_preserves_every_utf8_split_and_delivers_before_finish() {
        let bytes = text_event("hi 🌍 café 中文");
        for split in 0..=bytes.len() {
            let mut parser = SseDecoder::default();
            let mut deltas = Vec::new();
            parser.push(&bytes[..split], &mut |delta| deltas.push(delta)).unwrap();
            parser.push(&bytes[split..], &mut |delta| deltas.push(delta)).unwrap();
            assert!(matches!(&deltas[..], [StreamDelta::Text(text)] if text == "hi 🌍 café 中文"));
            parser.push(&stop_event(), &mut |delta| deltas.push(delta)).unwrap();
            parser.push(b"data: [DONE]\n\n", &mut |delta| deltas.push(delta)).unwrap();
            parser.finish(&mut |delta| deltas.push(delta)).unwrap();
            assert!(matches!(deltas.last(), Some(StreamDelta::Done { .. })));
        }
    }

    #[test]
    fn decoder_assembles_interleaved_tools_and_usage() {
        let mut parser = SseDecoder::default();
        let mut deltas = Vec::new();
        for data in [
            serde_json::json!({"choices":[{"delta":{"tool_calls":[
                {"index":1,"id":"b","function":{"name":"write_","arguments":"{\"path\":"}},
                {"index":0,"id":"a","function":{"name":"read_file","arguments":"{}"}}
            ]}}]}),
            serde_json::json!({"choices":[{"delta":{"tool_calls":[
                {"index":1,"function":{"name":"file","arguments":"\"x\"}"}}
            ]},"finish_reason":"tool_calls"}]}),
            serde_json::json!({"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":3,"total_tokens":15}}),
        ] { parser.push(&event(data), &mut |delta| deltas.push(delta)).unwrap(); }
        parser.finish(&mut |delta| deltas.push(delta)).unwrap();
        assert!(matches!(&deltas[0], StreamDelta::ToolCall(call) if call.id == "a"));
        assert!(matches!(&deltas[1], StreamDelta::ToolCall(call) if call.function.name == "write_file" && call.function.arguments == "{\"path\":\"x\"}"));
        assert!(matches!(&deltas[2], StreamDelta::Done { usage: Some(usage), .. } if usage.total_tokens == 15));
    }

    #[test]
    fn decoder_rejects_corrupt_empty_truncated_and_error_events() {
        let mut parser = SseDecoder::default();
        assert!(parser.push(b"data: {broken}\n\n", &mut |_| {}).is_err());
        let mut parser = SseDecoder::default();
        assert!(parser.push(b"data: {\"error\":{\"message\":\"overloaded\"}}\n\n", &mut |_| {}).is_err());
        let mut parser = SseDecoder::default();
        parser.push(&text_event("unfinished"), &mut |_| {}).unwrap();
        assert!(matches!(parser.finish(&mut |_| {}), Err(LlmError::StreamEnd)));
        let mut parser = SseDecoder::default();
        parser.push(&stop_event(), &mut |_| {}).unwrap();
        assert!(parser.finish(&mut |_| {}).is_err());
        let mut parser = SseDecoder::default();
        parser.push(&text_event("cut short"), &mut |_| {}).unwrap();
        parser.push(&event(serde_json::json!({"choices":[{"finish_reason":"length"}]})), &mut |_| {}).unwrap();
        assert!(matches!(parser.finish(&mut |_| {}), Err(LlmError::Incomplete(_))));
        let mut parser = SseDecoder::default();
        parser.push(b"data: {", &mut |_| {}).unwrap();
        assert!(matches!(parser.finish(&mut |_| {}), Err(LlmError::StreamEnd)));
    }

    #[test]
    fn local_hosts_are_parsed_not_matched_as_substrings() {
        for url in ["http://localhost:1234/v1", "http://127.0.0.2/v1", "http://[::1]:1234/v1"] {
            assert!(base_url_is_local(url).unwrap(), "{url}");
        }
        for url in ["https://localhost.evil.test/v1", "https://example.test/localhost", "https://example.test/127.0.0.1"] {
            assert!(!base_url_is_local(url).unwrap(), "{url}");
        }
        for url in ["file:///tmp/model", "https://secret@example.test/v1", "https://example.test/v1?token=x", "not a URL"] {
            assert!(base_url_is_local(url).is_err());
        }
    }

    fn request() -> CompletionRequest {
        CompletionRequest { model: "test".into(), messages: vec![Message::user("hi")], tools: vec![],
            tool_choice: None, temperature: None, max_tokens: Some(20), top_p: None, stop: vec![], extra: HashMap::new() }
    }

    async fn send_chunk(socket: &mut tokio::net::TcpStream, bytes: &[u8]) {
        use tokio::io::AsyncWriteExt;
        socket.write_all(format!("{:x}\r\n", bytes.len()).as_bytes()).await.unwrap();
        socket.write_all(bytes).await.unwrap();
        socket.write_all(b"\r\n").await.unwrap();
        socket.flush().await.unwrap();
    }

    async fn read_http_request(socket: &mut tokio::net::TcpStream) {
        use tokio::io::AsyncReadExt;
        let mut request = Vec::new();
        loop {
            let mut chunk = [0; 1024];
            let count = socket.read(&mut chunk).await.unwrap();
            assert!(count > 0, "connection closed before the complete request");
            request.extend_from_slice(&chunk[..count]);
            assert!(request.len() <= 65536, "test request exceeded its size limit");
            if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&request[..end]).unwrap();
                let length = headers.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                }).unwrap_or(0);
                if request.len() >= end + 4 + length { return; }
            }
        }
    }

    #[tokio::test]
    async fn network_stream_delivers_before_server_finishes_and_stops_at_done_marker() {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (first_tx, first_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_http_request(&mut socket).await;
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
            send_chunk(&mut socket, &text_event("first")).await;
            first_rx.await.unwrap(); // The second event cannot exist until the first reached the caller.
            send_chunk(&mut socket, &stop_event()).await;
            send_chunk(&mut socket, b"data: [DONE]\n\n").await;
            release_rx.await.unwrap(); // Keep HTTP open: the completion marker must end the request.
        });
        let provider = OpenAiCompatibleProvider::new("fixture", format!("http://{address}/v1"), "");
        let mut first_tx = Some(first_tx);
        let mut text = String::new();
        tokio::time::timeout(std::time::Duration::from_secs(3), provider.stream_live(&request(), &mut |delta| {
            if let StreamDelta::Text(part) = delta {
                text.push_str(&part);
                if let Some(tx) = first_tx.take() { tx.send(()).unwrap(); }
            }
        })).await.unwrap().unwrap();
        assert_eq!(text, "first");
        release_tx.send(()).unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn request_total_limit_stops_a_server_that_never_replies() {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_http_request(&mut socket).await;
            let mut byte = [0];
            while socket.read(&mut byte).await.unwrap_or(0) != 0 {}
        });
        let provider = OpenAiCompatibleProvider::new("fixture", format!("http://{address}/v1"), "")
            .with_limits(RequestLimits { total: std::time::Duration::from_millis(80), ..Default::default() });
        assert!(matches!(provider.stream_live(&request(), &mut |_| {}).await, Err(LlmError::Timeout(_))));
        tokio::time::timeout(std::time::Duration::from_secs(3), server).await.unwrap().unwrap();
    }


    async fn fixed_http_server(status: &str, body: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let header = format!("HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_http_request(&mut socket).await;
            socket.write_all(header.as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
        });
        (format!("http://{address}/v1"), server)
    }

    #[tokio::test]
    async fn transport_errors_and_limits_are_preserved_without_exposing_credentials() {
        let (url, server) = fixed_http_server("401 Unauthorized", b"rejected private-token".to_vec()).await;
        let provider = OpenAiCompatibleProvider::new("fixture", url, "private-token");
        let error = provider.stream_live(&request(), &mut |_| {}).await.unwrap_err();
        assert!(matches!(error, LlmError::Provider { status: 401, .. }));
        assert!(!error.to_string().contains("private-token"));
        server.await.unwrap();
        let (url, server) = fixed_http_server("200 OK", b"data: {\"error\":{\"message\":\"private-token invalid\"}}\n\n".to_vec()).await;
        let provider = OpenAiCompatibleProvider::new("fixture", url, "private-token");
        let error = provider.stream_live(&request(), &mut |_| {}).await.unwrap_err();
        assert!(!error.to_string().contains("private-token"));
        server.await.unwrap();
        let (url, server) = fixed_http_server("200 OK", text_event("too long")).await;
        let provider = OpenAiCompatibleProvider::new("fixture", url, "")
            .with_limits(RequestLimits { max_response_bytes: 8, ..Default::default() });
        assert!(provider.stream_live(&request(), &mut |_| {}).await.unwrap_err().to_string().contains("byte limit"));
        server.await.unwrap();
    }

    #[test]
    fn absent_usage_is_not_presented_as_zero_usage() {
        let mut parser = SseDecoder::default();
        parser.push(&text_event("ok"), &mut |_| {}).unwrap();
        parser.push(&stop_event(), &mut |_| {}).unwrap();
        parser.push(&event(serde_json::json!({"choices":[],"usage":{}})), &mut |_| {}).unwrap();
        let mut usage = None;
        parser.finish(&mut |delta| if let StreamDelta::Done { usage: counts, .. } = delta { usage = counts }).unwrap();
        assert!(usage.is_none());
    }

}
