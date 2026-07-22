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
}

// ---------------------------------------------------------------------------
// OpenAI-compatible provider
// ---------------------------------------------------------------------------

/// A provider that speaks the OpenAI Chat Completions HTTP API.
pub struct OpenAiCompatibleProvider {
    name: String,
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl OpenAiCompatibleProvider {
    /// Create a new OpenAI-compatible provider.
    ///
    /// `base_url` should be the root (e.g. "https://api.openai.com/v1").
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            client: reqwest::Client::new(),
        }
    }

    fn chat_url(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, req: &CompletionRequest) -> Result<CompletionResponse, LlmError> {
        // Build the JSON body ourselves so `extra` fields flatten correctly.
        let mut body = serde_json::to_value(req).map_err(|e| LlmError::Malformed(e.to_string()))?;
        // Force stream off for non-streaming path.
        if let Some(obj) = body.as_object_mut() {
            obj.insert("stream".to_string(), serde_json::Value::Bool(false));
            // tool_choice string -> object compatibility handled by provider; leave as-is.
            // Convert temperature/max_tokens to non-null if present (already optional).
        }

        let resp = self
            .client
            .post(self.chat_url())
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(LlmError::Provider {
                status: status.as_u16(),
                body: body_text,
            });
        }

        let parsed: ChatCompletionResponse = resp.json().await?;
        Ok(parsed.into_response())
    }

    async fn stream(
        &self,
        req: &CompletionRequest,
    ) -> Result<Vec<StreamDelta>, LlmError> {
        let mut body =
            serde_json::to_value(req).map_err(|e| LlmError::Malformed(e.to_string()))?;
        if let Some(obj) = body.as_object_mut() {
            obj.insert("stream".to_string(), serde_json::Value::Bool(true));
        }

        let resp = self
            .client
            .post(self.chat_url())
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(LlmError::Provider {
                status: status.as_u16(),
                body: body_text,
            });
        }

        let mut stream = resp.bytes_stream();
        let mut deltas = Vec::new();
        let mut content_buf = String::new();
        // tool_index -> (id, name, arguments_buf)
        let mut tool_acc: std::collections::BTreeMap<u32, (String, String, String)> =
            std::collections::BTreeMap::new();
        let mut finish_reason: Option<String> = None;
        let mut usage: Option<Usage> = None;
        let mut buf = String::new();

        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(LlmError::Network)?;
            buf.push_str(&String::from_utf8_lossy(&bytes));

            loop {
                let Some(line_end) = buf.find('\n') else {
                    break;
                };
                let line = buf[..line_end].trim().to_string();
                buf = buf[line_end + 1..].to_string();

                if line.is_empty() {
                    continue;
                }
                if !line.starts_with("data:") {
                    continue;
                }
                let data = line["data:".len()..].trim();
                if data == "[DONE]" {
                    continue;
                }
                let Ok(chunk_json) = serde_json::from_str::<serde_json::Value>(data) else {
                    continue;
                };

                // finish_reason
                if let Some(choices) = chunk_json.get("choices").and_then(|c| c.as_array()) {
                    if let Some(choice) = choices.first() {
                        if let Some(fr) =
                            choice.get("finish_reason").and_then(|v| v.as_str())
                        {
                            if fr != "null" {
                                finish_reason = Some(fr.to_string());
                            }
                        }
                        if let Some(delta) = choice.get("delta") {
                            if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                                content_buf.push_str(content);
                                deltas.push(StreamDelta::Text(content.to_string()));
                            }
                            if let Some(tcs) =
                                delta.get("tool_calls").and_then(|c| c.as_array())
                            {
                                for tc in tcs {
                                    let idx = tc
                                        .get("index")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0) as u32;
                                    let entry = tool_acc.entry(idx).or_default();
                                    if let Some(id) =
                                        tc.get("id").and_then(|v| v.as_str())
                                    {
                                        entry.0 = id.to_string();
                                    }
                                    if let Some(f) = tc.get("function") {
                                        if let Some(n) = f.get("name").and_then(|v| v.as_str()) {
                                            entry.1 = n.to_string();
                                        }
                                        if let Some(a) =
                                            f.get("arguments").and_then(|v| v.as_str())
                                        {
                                            entry.2.push_str(a);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // usage (sometimes sent in the final chunk)
                if let Some(u) = chunk_json.get("usage") {
                    usage = Some(parse_usage(u));
                }
            }
        }

        // Flush accumulated tool calls.
        for (_idx, (id, name, args)) in tool_acc.into_iter() {
            if !name.is_empty() {
                let tc = ToolCall {
                    id,
                    kind: "function".to_string(),
                    function: FunctionCall { name, arguments: args },
                };
                deltas.push(StreamDelta::ToolCall(tc));
            }
        }

        let _ = content_buf; // already streamed token-by-token
        deltas.push(StreamDelta::Done { finish_reason, usage });
        Ok(deltas)
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
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
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
        let usage = self.usage.map(|u| Usage {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        });
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
}
