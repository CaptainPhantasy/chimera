//! chimera-core::agent — The real LLM-driven agent loop.
//!
//! Composes an [`LlmProvider`] with a [`ToolRouter`] to run a ReAct-style
//! loop: prompt the model, execute any tool calls it emits, feed results
//! back, repeat until the model produces a final text answer with no tool
//! calls.


use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use chimera_llm::{
    CompletionRequest, CompletionResponse, FunctionDefinition, LlmProvider, Message,
    StreamDelta, ToolCall, ToolDefinition,
};
use chimera_tools::{ToolCall as RouterToolCall, ToolRouter};

use crate::CreatureId;

const MAX_CONVERSATION_BYTES: usize = 4 * 1024 * 1024;
const MAX_MODEL_BYTES: usize = 1024 * 1024;
const MAX_TOOL_RESULT_BYTES: usize = 1024 * 1024;
const MAX_MODEL_TOOL_CALLS: usize = 128;

/// Count encoded bytes without allocating a second copy of oversized data.
fn check_encoded_size<T: Serialize + ?Sized>(value: &T, limit: usize) -> anyhow::Result<()> {
    struct Counter { size: usize, limit: usize }
    impl std::io::Write for Counter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            if data.len() > self.limit.saturating_sub(self.size) {
                return Err(std::io::Error::other("encoded data exceeds its byte limit"));
            }
            self.size += data.len(); Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    serde_json::to_writer(Counter {size:0,limit}, value)?;
    Ok(())
}

fn append_bounded_message(messages: &mut Vec<Message>, message: Message) -> bool {
    if messages.len() >= 4096 { return false; }
    messages.push(message);
    if check_encoded_size(messages, MAX_CONVERSATION_BYTES).is_err() {
        messages.pop();
        false
    } else { true }
}

// ---------------------------------------------------------------------------
// Loop configuration
// ---------------------------------------------------------------------------

/// Configuration for an agent loop run.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Maximum tool-call rounds before forcing a stop.
    pub max_iterations: usize,
    /// System prompt prepended to every conversation.
    pub system_prompt: String,
    /// Sampling temperature.
    pub temperature: Option<f32>,
    /// Max tokens per completion.
    pub max_tokens: Option<u32>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_iterations: 40,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            temperature: Some(0.0),
            max_tokens: Some(8192),
        }
    }
}

pub const DEFAULT_SYSTEM_PROMPT: &str = "\
You are CHIMERA, an autonomous coding agent operating in a real repository. \
You solve objectives by decomposing them, inspecting the codebase with your tools, \
making surgical edits, and verifying your work. \
Always prefer reading before writing. Make the smallest change that satisfies the objective. \
When you have completed the objective, respond with a concise final summary and no tool calls.";

// ---------------------------------------------------------------------------
// Loop step log
// ---------------------------------------------------------------------------

/// A recorded step in the agent loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentStep {
    /// The model produced a text response (possibly with tool calls).
    Model {
        content: Option<String>,
        tool_calls: Vec<RecordedToolCall>,
        finish_reason: Option<String>,
    },
    /// A tool was executed.
    Tool {
        call: RecordedToolCall,
        ok: bool,
        output: serde_json::Value,
    },
    /// The loop terminated.
    Done {
        reason: DoneReason,
        iterations: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoneReason {
    /// Model produced a final answer with no tool calls.
    FinalAnswer,
    /// Hit the iteration cap.
    MaxIterations,
    /// Encountered an unrecoverable error.
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

impl RecordedToolCall {
    fn from_llm(tc: &ToolCall) -> Self {
        let args = if tc.function.arguments.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&tc.function.arguments).unwrap_or(serde_json::Value::Null)
        };
        Self {
            id: tc.id.clone(),
            name: tc.function.name.clone(),
            arguments: args,
        }
    }
}

// ---------------------------------------------------------------------------
// Loop result
// ---------------------------------------------------------------------------

/// The result of running the agent loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunResult {
    /// Final assistant text (the last non-tool message), if any.
    pub final_answer: Option<String>,
    /// Full step log.
    pub steps: Vec<AgentStep>,
    /// Why the loop stopped.
    pub done_reason: DoneReason,
    /// Number of model rounds executed.
    pub iterations: usize,
    /// Token usage accumulated across all rounds.
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    /// True only when every attempted request reported token counts.
    #[serde(default)]
    pub usage_available: bool,
    /// Conversation including the new user input once, suitable for persistence and follow-ups.
    #[serde(default)]
    pub messages: Vec<Message>,
    /// A concrete failure or incomplete reason; absent only on a completed answer.
    #[serde(default)]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// The loop
// ---------------------------------------------------------------------------

/// Run the agent loop.
///
/// - `provider`: the LLM backend.
/// - `tools`: the tool router for executing tool calls.
/// - `objective`: the user's goal.
/// - `creature_id`: which creature is running (for tool-call attribution).
/// - `config`: loop tuning.
/// - `stream_callback`: optional closure invoked for each streaming delta
///   (e.g. to print tokens to the terminal). Pass `&mut |_, _| {}` to ignore.
///
/// The loop builds a conversation: [system, user(objective), ...]. Each round
/// it calls the provider. If the model emits tool calls, it executes them via
/// the router and appends tool-result messages. If the model emits only text,
/// the loop ends with [`DoneReason::FinalAnswer`].
#[allow(clippy::too_many_arguments)]
pub async fn run_agent_loop<F>(
    provider: &dyn LlmProvider,
    tools: &dyn ToolRouter,
    model: &str,
    objective: &str,
    creature_id: CreatureId,
    config: &AgentConfig,
    available_tools: &[ToolDefinition],
    stream_callback: &mut F,
) -> anyhow::Result<AgentRunResult>
where
    F: FnMut(String, serde_json::Value) + Send,
{
    run_agent_loop_with_history(provider, tools, model, objective, AgentLoopContext { creature_id, config, available_tools }, &[], stream_callback).await
}

/// Per-run identity, limits, and available tool definitions.
pub struct AgentLoopContext<'a> {
    pub creature_id: CreatureId,
    pub config: &'a AgentConfig,
    pub available_tools: &'a [ToolDefinition],
}

/// Resume a conversation and append `objective` exactly once. The current system
/// prompt replaces prior system prompts. Dropping the future cancels current work.
/// The `messages` callback provides complete snapshots after input/model/tool events.
pub async fn run_agent_loop_with_history<F>(
    provider: &dyn LlmProvider,
    tools: &dyn ToolRouter,
    model: &str,
    objective: &str,
    context: AgentLoopContext<'_>,
    prior_messages: &[Message],
    stream_callback: &mut F,
) -> anyhow::Result<AgentRunResult>
where
    F: FnMut(String, serde_json::Value) + Send,
{
    let AgentLoopContext { creature_id, config, available_tools } = context;
    anyhow::ensure!(!objective.trim().is_empty(), "objective must not be empty");
    anyhow::ensure!(config.max_iterations <= 1000, "max_iterations must be at most 1000");
    anyhow::ensure!(config.max_tokens != Some(0), "max_tokens must be positive when specified");
    anyhow::ensure!(config.temperature.is_none_or(|t| t.is_finite() && (0.0..=2.0).contains(&t)), "temperature must be finite and between 0 and 2");
    anyhow::ensure!(available_tools.len() <= 128, "tool catalog exceeds the 128-tool limit");
    check_encoded_size(available_tools, 256 * 1024).map_err(|_| anyhow::anyhow!("tool definitions exceed the 256 KiB limit"))?;
    anyhow::ensure!(prior_messages.len() <= 4096, "conversation exceeds the 4096-message limit");
    check_encoded_size(prior_messages, MAX_CONVERSATION_BYTES)?;
    anyhow::ensure!(objective.len().saturating_add(config.system_prompt.len()) <= MAX_CONVERSATION_BYTES, "new input and system context exceed the 4 MiB conversation limit");
    let mut messages = vec![Message::system(&config.system_prompt)];
    messages.extend(prior_messages.iter().filter(|m| m.role != chimera_llm::Role::System).cloned());
    messages.push(Message::user(objective));
    anyhow::ensure!(messages.len() <= 4096, "conversation exceeds the 4096-message limit");
    check_encoded_size(&messages, MAX_CONVERSATION_BYTES).map_err(|_| anyhow::anyhow!("conversation exceeds the 4 MiB limit"))?;
    stream_callback("messages".into(), serde_json::to_value(&messages)?);
    let mut steps = Vec::new();
    let mut total_prompt_tokens = 0u64;
    let mut total_completion_tokens = 0u64;
    let mut usage_available = true;
    let mut final_answer = None;
    let mut error = None;
    let mut iteration = 0usize;

    let done_reason = 'run: loop {
        if iteration >= config.max_iterations {
            error = Some(format!("iteration limit reached after {iteration} model requests"));
            break DoneReason::MaxIterations;
        }
        // Bound history before cloning it into another request. Do not silently discard context.
        if messages.len() > 4096 || check_encoded_size(&messages, MAX_CONVERSATION_BYTES).is_err() {
            error = Some("conversation exceeds the 4096-message or 4 MiB limit; start a new session or shorten the retained history".into());
            break DoneReason::Error;
        }
        iteration += 1;
        stream_callback("iteration".into(), serde_json::json!({"n": iteration}));
        let req = CompletionRequest {
            model: model.to_string(), messages: messages.clone(), tools: available_tools.to_vec(),
            tool_choice: None, temperature: config.temperature, max_tokens: config.max_tokens,
            top_p: None, stop: vec![], extra: Default::default(),
        };
        debug!(iteration, model, "requesting completion");
        let mut content = String::new();
        let mut tool_calls = Vec::new();
        let mut finish_reason = None;
        let mut stream_error = None;
        let mut saw_done = false;
        let mut round_usage_available = false;
        let mut model_bytes = 0usize;
        let stop_stream = tokio::sync::Notify::new();
        let mut on_delta = |delta| {
            if stream_error.is_some() { return; }
            if saw_done {
                stream_error = Some("provider emitted data after completion".to_string());
                stop_stream.notify_one();
                return;
            }
            match delta {
                StreamDelta::Text(text) => {
                    if text.len() > MAX_MODEL_BYTES.saturating_sub(model_bytes) {
                        stream_error = Some("model output exceeds the 1 MiB limit; partial text was retained and no tool from this response was run".into());
                        stop_stream.notify_one(); return;
                    }
                    model_bytes += text.len();
                    content.push_str(&text);
                    stream_callback("text".into(), serde_json::Value::String(text));
                }
                StreamDelta::ToolCall(call) => {
                    let bytes = call.id.len().saturating_add(call.kind.len()).saturating_add(call.function.name.len()).saturating_add(call.function.arguments.len());
                    if tool_calls.len() >= MAX_MODEL_TOOL_CALLS || bytes > MAX_MODEL_BYTES.saturating_sub(model_bytes) {
                        stream_error = Some("model tool calls exceed the 128-call or 1 MiB limit; no tool from this response was run".into());
                        stop_stream.notify_one(); return;
                    }
                    model_bytes += bytes;
                    tool_calls.push(call);
                }
                StreamDelta::Done { finish_reason: reason, usage } => {
                    saw_done = true;
                    finish_reason = reason;
                    if let Some(usage) = usage {
                        if config.max_tokens.is_some_and(|limit| usage.completion_tokens > u64::from(limit)) {
                            stream_error = Some("provider exceeded the requested completion token limit; no tool from this response was run".into());
                            stop_stream.notify_one();
                        }
                        round_usage_available = true;
                        match (total_prompt_tokens.checked_add(usage.prompt_tokens), total_completion_tokens.checked_add(usage.completion_tokens)) {
                            (Some(prompt), Some(completion)) => { total_prompt_tokens = prompt; total_completion_tokens = completion; }
                            _ => { stream_error = Some("provider token counts overflowed".into()); round_usage_available = false; }
                        }
                    }
                }
                StreamDelta::Error(problem) => { stream_error = Some(problem.to_string()); stop_stream.notify_one(); },
            }
        };
        let streamed = {
            let stream = provider.stream_live(&req, &mut on_delta);
            tokio::pin!(stream);
            tokio::select! {
                biased;
                _ = stop_stream.notified() => Ok(()),
                result = &mut stream => result,
            }
        };
        usage_available &= round_usage_available;
        stream_callback("tokens".into(), serde_json::json!({"prompt": total_prompt_tokens, "completion": total_completion_tokens, "available": usage_available}));
        if let Err(problem) = streamed { stream_error = Some(problem.to_string()); }
        if !saw_done && stream_error.is_none() { stream_error = Some("provider stream ended without completion".into()); }
        let response = CompletionResponse {
            content: if content.is_empty() { None } else { Some(content) },
            tool_calls, finish_reason, usage: None,
        };
        if stream_error.is_none() {
            if let Err(problem) = chimera_llm::validate_response(&response) {
                stream_error = Some(problem.to_string());
            }
        }
        if let Some(problem) = stream_error {
            // Preserve text already displayed, but never run partial or invalid tool calls.
            if let Some(text) = response.content {
                append_bounded_message(&mut messages, Message::assistant(&text));
                steps.push(AgentStep::Model { content: Some(text), tool_calls: vec![], finish_reason: response.finish_reason });
                stream_callback("messages".into(), serde_json::to_value(&messages)?);
            }
            warn!(error = %problem, "provider request failed");
            error = Some(problem);
            break DoneReason::Error;
        }
        let recorded: Vec<RecordedToolCall> = response.tool_calls.iter().map(RecordedToolCall::from_llm).collect();
        steps.push(AgentStep::Model { content: response.content.clone(), tool_calls: recorded.clone(), finish_reason: response.finish_reason.clone() });
        if !append_bounded_message(&mut messages, Message::assistant_with_tools(response.content.clone(), response.tool_calls.clone())) {
            error = Some("model response exceeds the remaining 4096-message or 4 MiB conversation limit; response remains in the run steps and no tool from it was executed".into());
            break DoneReason::Error;
        }
        stream_callback("messages".into(), serde_json::to_value(&messages)?);
        stream_callback("text_done".into(), serde_json::Value::Null);
        if response.tool_calls.is_empty() {
            final_answer = response.content;
            break DoneReason::FinalAnswer;
        }
        for (call, recorded_call) in response.tool_calls.iter().zip(recorded.iter()) {
            if messages.len() >= 4096 {
                error = Some("conversation reached its 4096-message limit; remaining tool calls were not executed".into());
                break 'run DoneReason::Error;
            }
            // validate_response has already checked every call before any side effects.
            let args: serde_json::Value = serde_json::from_str(&call.function.arguments)?;
            stream_callback("tool_call".into(), serde_json::json!({"id": call.id, "name": call.function.name, "arguments": args}));
            info!(tool = %call.function.name, "executing tool");
            let started = std::time::Instant::now();
            let request = RouterToolCall { tool: call.function.name.clone(), args, world: None, requested_by: creature_id };
            let mut result = match tools.call(request).await {
                Ok(result) => result,
                Err(problem) => chimera_tools::ToolResult { ok: false,
                    output: serde_json::json!({"error": problem.to_string()}), evidence: vec![] },
            };
            if check_encoded_size(&result.output, MAX_TOOL_RESULT_BYTES).is_err() {
                result = chimera_tools::ToolResult { ok:false, output:serde_json::json!({"error":"tool output exceeds the 1 MiB result limit; its action may have occurred; inspect saved state before any retry"}), evidence:vec![] };
            }
            if !append_bounded_message(&mut messages, Message::tool_result(&call.id, serde_json::to_string(&result.output)?)) {
                result = chimera_tools::ToolResult { ok:false, output:serde_json::json!({"error":"tool result exceeds the remaining conversation limit; its action may have occurred; inspect current state before any retry"}), evidence:vec![] };
                append_bounded_message(&mut messages, Message::tool_result(&call.id, serde_json::to_string(&result.output)?));
            }
            steps.push(AgentStep::Tool { call: recorded_call.clone(), ok: result.ok, output: result.output.clone() });
            stream_callback("tool_result".into(), serde_json::json!({
                "id": call.id, "name": call.function.name, "output": result.output,
                "ok": result.ok, "duration_ms": started.elapsed().as_millis() as u64,
            }));
            stream_callback("messages".into(), serde_json::to_value(&messages)?);
            if !result.ok {
                error = Some(format!("tool {} failed: {}", call.function.name, result.output));
                break 'run DoneReason::Error;
            }
        }
    };
    steps.push(AgentStep::Done { reason: done_reason.clone(), iterations: iteration });
    if let Some(problem) = &error { stream_callback("error".into(), serde_json::Value::String(problem.clone())); }
    Ok(AgentRunResult { final_answer, steps, done_reason, iterations: iteration,
        total_prompt_tokens, total_completion_tokens, usage_available: usage_available && iteration > 0, messages, error })
}

// ---------------------------------------------------------------------------
// Helpers: convert ToolRouter descriptors to LLM tool definitions
// ---------------------------------------------------------------------------

/// Fetch the tool router's catalog and convert to LLM tool definitions.
pub async fn build_tool_definitions(
    tools: &dyn ToolRouter,
) -> anyhow::Result<Vec<ToolDefinition>> {
    let descriptors = tools.list().await?;
    anyhow::ensure!(descriptors.len() <= 128, "tool catalog exceeds the 128-tool limit");
    let mut definitions = Vec::with_capacity(descriptors.len());
    for descriptor in descriptors {
        let parameters = tools.input_schema(&descriptor.name).await?
            .unwrap_or_else(|| schema_for_tool(&descriptor.name));
        definitions.push(ToolDefinition {
            kind: "function".into(),
            function: FunctionDefinition {
                name: descriptor.name,
                description: descriptor.description,
                parameters,
            },
        });
    }
    check_encoded_size(&definitions, 256 * 1024).map_err(|_| anyhow::anyhow!("tool definitions exceed the 256 KiB limit"))?;
    Ok(definitions)
}

/// Minimal JSON schemas for built-in tools. Unknown tools get a permissive
/// schema that accepts any object.
fn schema_for_tool(name: &str) -> serde_json::Value {
    match name {
        "read_file" => serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string", "description": "absolute or repo-relative file path"}},
            "required": ["path"]
        }),
        "write_file" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"}
            },
            "required": ["path", "content"]
        }),
        "edit_file" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "old_text": {"type": "string"},
                "new_text": {"type": "string"}
            },
            "required": ["path", "old_text", "new_text"]
        }),
        "bash" => serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "shell command to execute"},
                "cwd": {"type": "string"},
                "timeout_secs": {"type": "integer"}
            },
            "required": ["command"]
        }),
        "glob" => serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string"},
                "path": {"type": "string"}
            },
            "required": ["pattern"]
        }),
        "grep" => serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Rust regex"},
                "path": {"type": "string"},
                "max_results": {"type": "integer"}
            },
            "required": ["pattern"]
        }),
        "git_status" => serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}}
        }),
        _ => serde_json::json!({"type": "object", "additionalProperties": true}),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use async_trait::async_trait;
    use chimera_llm::{LlmError, Usage};
    use chimera_tools::{ToolCategory, ToolDescriptor, ToolEvidence, ToolResult};

    /// A fake provider that emits a scripted sequence of responses.
    struct ScriptedProvider {
        responses: tokio::sync::Mutex<Vec<CompletionResponse>>,
    }

    impl ScriptedProvider {
        fn new(responses: Vec<CompletionResponse>) -> Self {
            Self {
                responses: tokio::sync::Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for ScriptedProvider {
        fn name(&self) -> &str {
            "scripted"
        }
        async fn complete(
            &self,
            _req: &CompletionRequest,
        ) -> Result<CompletionResponse, LlmError> {
            let mut guard = self.responses.lock().await;
            guard.pop().ok_or(LlmError::StreamEnd)
        }
        async fn stream(
            &self,
            _req: &CompletionRequest,
        ) -> Result<Vec<StreamDelta>, LlmError> {
            let mut guard = self.responses.lock().await;
            let resp = guard.pop().ok_or(LlmError::StreamEnd)?;
            let mut deltas = Vec::new();
            if let Some(t) = resp.content {
                deltas.push(StreamDelta::Text(t));
            }
            for tc in resp.tool_calls.clone() {
                deltas.push(StreamDelta::ToolCall(tc));
            }
            deltas.push(StreamDelta::Done {
                finish_reason: resp.finish_reason.clone(),
                usage: resp.usage.clone(),
            });
            Ok(deltas)
        }
    }

    /// A fake tool router that records calls and returns canned results.
    struct FakeRouter {
        calls: tokio::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ToolRouter for FakeRouter {
        async fn call(&self, req: RouterToolCall) -> anyhow::Result<ToolResult> {
            self.calls.lock().await.push(req.tool.clone());
            Ok(ToolResult {
                ok: true,
                output: serde_json::json!({"echo": req.args}),
                evidence: vec![ToolEvidence {
                    kind: "transcript".to_string(),
                    uri: "mem://test".to_string(),
                }],
            })
        }
        async fn list(&self) -> anyhow::Result<Vec<ToolDescriptor>> {
            Ok(vec![ToolDescriptor {
                name: "bash".to_string(),
                description: "run a command".to_string(),
                read_only: false,
                category: ToolCategory::LocalExecution,
            }])
        }
        async fn healthcheck(&self, _tool: &str) -> anyhow::Result<bool> {
            Ok(true)
        }
    }

    #[tokio::test]
    async fn loop_terminates_on_final_answer() {
        let provider = ScriptedProvider::new(vec![CompletionResponse {
            content: Some("done".to_string()),
            tool_calls: vec![],
            finish_reason: Some("stop".to_string()),
            usage: Some(Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
        }]);
        let router = FakeRouter {
            calls: tokio::sync::Mutex::new(vec![]),
        };
        let mut cb = |_, _| {};
        let cfg = AgentConfig::default();
        let result = run_agent_loop(
            &provider,
            &router,
            "test-model",
            "do nothing",
            Uuid::new_v4(),
            &cfg,
            &[],
            &mut cb,
        )
        .await
        .unwrap();

        assert!(matches!(result.done_reason, DoneReason::FinalAnswer));
        assert_eq!(result.final_answer.as_deref(), Some("done"));
        assert_eq!(result.total_prompt_tokens, 10);
        assert_eq!(result.total_completion_tokens, 5);
        assert!(result.usage_available);
    }

    #[tokio::test]
    async fn loop_executes_tool_calls_then_finishes() {
        // Scripted responses are popped in reverse, so push in reverse order:
        // first the tool-call round, then the final answer.
        let tool_call = ToolCall {
            id: "call_1".to_string(),
            kind: "function".to_string(),
            function: chimera_llm::FunctionCall {
                name: "bash".to_string(),
                arguments: r#"{"command":"echo hi"}"#.to_string(),
            },
        };
        let provider = ScriptedProvider::new(vec![
            CompletionResponse {
                content: Some("finished".to_string()),
                tool_calls: vec![],
                finish_reason: Some("stop".to_string()),
                usage: None,
            },
            CompletionResponse {
                content: None,
                tool_calls: vec![tool_call],
                finish_reason: Some("tool_calls".to_string()),
                usage: None,
            },
        ]);
        let router = FakeRouter {
            calls: tokio::sync::Mutex::new(vec![]),
        };
        let mut cb = |_, _| {};
        let cfg = AgentConfig::default();
        let result = run_agent_loop(
            &provider,
            &router,
            "test-model",
            "run echo",
            Uuid::new_v4(),
            &cfg,
            &[],
            &mut cb,
        )
        .await
        .unwrap();

        assert!(matches!(result.done_reason, DoneReason::FinalAnswer));
        assert_eq!(result.iterations, 2);
        assert!(!result.usage_available);
        // Should have: Model(tool), Tool, Model(final), Done
        assert!(result.steps.iter().any(|s| matches!(s, AgentStep::Tool { .. })));
    }

    #[tokio::test]
    async fn loop_stops_on_max_iterations() {
        // Provider always returns a tool call -> never terminates naturally.
        let _tool_call = ToolCall {
            id: "call_1".to_string(),
            kind: "function".to_string(),
            function: chimera_llm::FunctionCall {
                name: "bash".to_string(),
                arguments: r#"{"command":"loop"}"#.to_string(),
            },
        };
        let provider = ScriptedProvider::new(vec![]); // stream returns StreamEnd -> Error
        let router = FakeRouter {
            calls: tokio::sync::Mutex::new(vec![]),
        };
        let mut cb = |_, _| {};
        let cfg = AgentConfig {
            max_iterations: 2,
            ..Default::default()
        };
        let result = run_agent_loop(
            &provider,
            &router,
            "test-model",
            "loop",
            Uuid::new_v4(),
            &cfg,
            &[],
            &mut cb,
        )
        .await
        .unwrap();

        // Provider has no responses -> first round errors out -> DoneReason::Error
        assert!(matches!(
            result.done_reason,
            DoneReason::Error | DoneReason::MaxIterations
        ));
    }

    #[test]
    fn schema_for_known_tools_has_required_fields() {
        let s = schema_for_tool("bash");
        assert!(s.get("properties").is_some());
        let reqs = s.get("required").unwrap().as_array().unwrap();
        assert!(reqs.iter().any(|r| r.as_str() == Some("command")));
    }

    #[test]
    fn schema_for_unknown_tool_is_permissive() {
        let s = schema_for_tool("custom_tool");
        assert_eq!(
            s.get("additionalProperties").and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[tokio::test]
    async fn build_tool_definitions_converts_descriptors() {
        let router = FakeRouter {
            calls: tokio::sync::Mutex::new(vec![]),
        };
        let defs = build_tool_definitions(&router).await.unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].function.name, "bash");
    }

    // Silence unused warnings for the scripted tool_call in the max_iters test.
    #[allow(dead_code)]
    fn _suppress() {
        let _ = ToolCall {
            id: String::new(),
            kind: String::new(),
            function: chimera_llm::FunctionCall {
                name: String::new(),
                arguments: String::new(),
            },
        };
    }

    #[tokio::test]
    async fn zero_and_one_iteration_limits_count_actual_requests() {
        let call = ToolCall { id: "one".into(), kind: "function".into(), function: chimera_llm::FunctionCall {
            name: "bash".into(), arguments: "{\"command\":\"echo ok\"}".into(),
        }};
        for max_iterations in [0, 1] {
            let provider = ScriptedProvider::new(vec![CompletionResponse { content: None, tool_calls: vec![call.clone()],
                finish_reason: Some("tool_calls".into()), usage: None }]);
            let router = FakeRouter { calls: tokio::sync::Mutex::new(vec![]) };
            let cfg = AgentConfig { max_iterations, ..Default::default() };
            let result = run_agent_loop(&provider, &router, "test", "run", Uuid::new_v4(), &cfg, &[], &mut |_, _| {}).await.unwrap();
            assert!(matches!(result.done_reason, DoneReason::MaxIterations));
            assert_eq!(result.iterations, max_iterations);
            assert_eq!(provider.responses.lock().await.len(), 1 - max_iterations);
            assert_eq!(router.calls.lock().await.len(), max_iterations);
            assert!(result.error.is_some());
        }
    }

    #[tokio::test]
    async fn empty_truncated_and_malformed_tools_are_errors_without_side_effects() {
        let invalid_call = ToolCall { id: "one".into(), kind: "function".into(), function: chimera_llm::FunctionCall {
            name: "bash".into(), arguments: "{\"command\":".into(),
        }};
        for response in [
            CompletionResponse { content: None, tool_calls: vec![], finish_reason: Some("stop".into()), usage: None },
            CompletionResponse { content: Some("partial".into()), tool_calls: vec![], finish_reason: Some("length".into()), usage: None },
            CompletionResponse { content: None, tool_calls: vec![invalid_call], finish_reason: Some("tool_calls".into()), usage: None },
        ] {
            let provider = ScriptedProvider::new(vec![response]);
            let router = FakeRouter { calls: tokio::sync::Mutex::new(vec![]) };
            let result = run_agent_loop(&provider, &router, "test", "run", Uuid::new_v4(), &AgentConfig::default(), &[], &mut |_, _| {}).await.unwrap();
            assert!(matches!(result.done_reason, DoneReason::Error));
            assert!(result.error.as_deref().is_some_and(|error| !error.is_empty()));
            assert!(result.final_answer.is_none());
            assert!(router.calls.lock().await.is_empty());
        }
    }

    #[tokio::test]
    async fn history_is_preserved_and_messages_callback_tracks_new_input_once() {
        let provider = ScriptedProvider::new(vec![CompletionResponse { content: Some("new answer".into()), tool_calls: vec![],
            finish_reason: Some("stop".into()), usage: None }]);
        let router = FakeRouter { calls: tokio::sync::Mutex::new(vec![]) };
        let history = vec![Message::system("old system"), Message::user("old question"), Message::assistant("old answer")];
        let mut snapshots = Vec::new();
        let result = run_agent_loop_with_history(&provider, &router, "test", "new question", AgentLoopContext { creature_id: Uuid::new_v4(), config: &AgentConfig::default(), available_tools: &[] }, &history, &mut |kind, value| {
                if kind == "messages" { snapshots.push(value); }
            }).await.unwrap();
        assert_eq!(result.messages.len(), 5);
        assert_eq!(result.messages.iter().filter(|m| m.content.as_deref() == Some("new question")).count(), 1);
        assert_eq!(result.messages[1].content.as_deref(), Some("old question"));
        assert_eq!(result.messages[0].content.as_deref(), Some(DEFAULT_SYSTEM_PROMPT));
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots.last().unwrap(), &serde_json::to_value(&result.messages).unwrap());
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn dropping_agent_future_cancels_provider_work() {
        use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
        struct PendingProvider { entered: tokio::sync::Notify, dropped: Arc<AtomicBool> }
        struct Guard(Arc<AtomicBool>);
        impl Drop for Guard { fn drop(&mut self) { self.0.store(true, Ordering::SeqCst); } }
        #[async_trait]
        impl LlmProvider for PendingProvider {
            fn name(&self) -> &str { "pending" }
            async fn complete(&self, _: &CompletionRequest) -> Result<CompletionResponse, LlmError> { unreachable!() }
            async fn stream_live(&self, _: &CompletionRequest, _: &mut (dyn FnMut(StreamDelta) + Send)) -> Result<(), LlmError> {
                let _guard = Guard(self.dropped.clone());
                self.entered.notify_one();
                std::future::pending().await
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let provider = PendingProvider { entered: tokio::sync::Notify::new(), dropped: dropped.clone() };
        let router = FakeRouter { calls: tokio::sync::Mutex::new(vec![]) };
        let cfg = AgentConfig::default();
        let mut cb = |_, _| {};
        let mut future = Box::pin(run_agent_loop(&provider, &router, "test", "run", Uuid::new_v4(), &cfg, &[], &mut cb));
        tokio::select! { _ = &mut future => panic!("request unexpectedly finished"), _ = provider.entered.notified() => {} }
        drop(future);
        assert!(dropped.load(Ordering::SeqCst));
        assert!(router.calls.lock().await.is_empty());
    }


    #[tokio::test]
    async fn failed_tool_is_saved_and_stops_before_a_false_success_answer() {
        struct DeniedRouter;
        #[async_trait]
        impl ToolRouter for DeniedRouter {
            async fn call(&self, _: RouterToolCall) -> anyhow::Result<ToolResult> {
                Ok(ToolResult { ok: false, output: serde_json::json!({"error":"permission denied"}), evidence: vec![] })
            }
            async fn list(&self) -> anyhow::Result<Vec<ToolDescriptor>> { Ok(vec![]) }
            async fn healthcheck(&self, _: &str) -> anyhow::Result<bool> { Ok(false) }
        }
        let call = ToolCall { id: "one".into(), kind: "function".into(), function: chimera_llm::FunctionCall {
            name: "write_file".into(), arguments: "{}".into(),
        }};
        let provider = ScriptedProvider::new(vec![
            CompletionResponse { content: Some("done".into()), tool_calls: vec![], finish_reason: Some("stop".into()), usage: None },
            CompletionResponse { content: None, tool_calls: vec![call], finish_reason: Some("tool_calls".into()), usage: None },
        ]);
        let result = run_agent_loop(&provider, &DeniedRouter, "test", "run", Uuid::new_v4(), &AgentConfig::default(), &[], &mut |_, _| {}).await.unwrap();
        assert!(matches!(result.done_reason, DoneReason::Error));
        assert!(result.error.as_deref().unwrap().contains("permission denied"));
        assert_eq!(result.messages.last().unwrap().role, chimera_llm::Role::Tool);
        assert_eq!(provider.responses.lock().await.len(), 1);
        assert_eq!(result.iterations, 1);
    }

    #[tokio::test]
    async fn oversized_live_output_cancels_stalled_provider_and_retains_valid_partial_text() {
        use std::sync::{Arc,atomic::{AtomicBool,Ordering}};
        struct Flood {dropped:Arc<AtomicBool>}
        struct Guard(Arc<AtomicBool>);impl Drop for Guard {fn drop(&mut self){self.0.store(true,Ordering::SeqCst);}}
        #[async_trait]
        impl LlmProvider for Flood {
            fn name(&self)->&str {"controlled-flood"}
            async fn complete(&self,_:&CompletionRequest)->Result<CompletionResponse,LlmError>{unreachable!()}
            async fn stream_live(&self,_:&CompletionRequest,emit:&mut(dyn FnMut(StreamDelta)+Send))->Result<(),LlmError>{
                let _guard=Guard(self.dropped.clone());
                emit(StreamDelta::Text("é🍄 partial".into()));
                emit(StreamDelta::Text("x".repeat(MAX_MODEL_BYTES)));
                std::future::pending().await
            }
        }
        let dropped=Arc::new(AtomicBool::new(false));let provider=Flood {dropped:dropped.clone()};
        let router=FakeRouter {calls:tokio::sync::Mutex::new(vec![])};
        let result=tokio::time::timeout(std::time::Duration::from_secs(2),run_agent_loop(&provider,&router,"test","read",Uuid::new_v4(),&AgentConfig::default(),&[],&mut|_,_|{})).await.unwrap().unwrap();
        assert!(matches!(result.done_reason,DoneReason::Error));assert!(result.error.unwrap().contains("1 MiB"));
        assert!(dropped.load(Ordering::SeqCst));assert_eq!(result.messages.last().unwrap().content.as_deref(),Some("é🍄 partial"));
        assert!(router.calls.lock().await.is_empty());assert!(!result.usage_available);
    }

    #[tokio::test]
    async fn excessive_tool_calls_and_reported_token_overrun_never_execute_tools() {
        let call=ToolCall {id:"controlled".into(),kind:"function".into(),function:chimera_llm::FunctionCall{name:"bash".into(),arguments:"{}".into()}};
        let many:Vec<_>=(0..129).map(|n|{let mut call=call.clone();call.id=n.to_string();call}).collect();
        for response in [
            CompletionResponse {content:None,tool_calls:many,finish_reason:Some("tool_calls".into()),usage:None},
            CompletionResponse {content:None,tool_calls:vec![call],finish_reason:Some("tool_calls".into()),usage:Some(Usage{prompt_tokens:10,completion_tokens:3,total_tokens:13})},
        ] {
            let provider=ScriptedProvider::new(vec![response]);let router=FakeRouter {calls:tokio::sync::Mutex::new(vec![])};
            let config=AgentConfig {max_tokens:Some(2),..Default::default()};
            let result=run_agent_loop(&provider,&router,"test","run",Uuid::new_v4(),&config,&[],&mut|_,_|{}).await.unwrap();
            assert!(matches!(result.done_reason,DoneReason::Error));assert!(result.error.is_some());assert!(router.calls.lock().await.is_empty());
        }
    }

    #[tokio::test]
    async fn oversized_tool_result_stops_with_an_explicit_uncertain_action_outcome() {
        struct LargeResult;
        #[async_trait]
        impl ToolRouter for LargeResult {
            async fn call(&self,_:RouterToolCall)->anyhow::Result<ToolResult>{Ok(ToolResult {ok:true,output:serde_json::json!({"data":"x".repeat(MAX_TOOL_RESULT_BYTES)}),evidence:vec![]})}
            async fn list(&self)->anyhow::Result<Vec<ToolDescriptor>>{Ok(vec![])}
            async fn healthcheck(&self,_:&str)->anyhow::Result<bool>{Ok(true)}
        }
        let provider=ScriptedProvider::new(vec![CompletionResponse {content:None,tool_calls:vec![ToolCall{id:"large".into(),kind:"function".into(),function:chimera_llm::FunctionCall{name:"large".into(),arguments:"{}".into()}}],finish_reason:Some("tool_calls".into()),usage:None}]);
        let result=run_agent_loop(&provider,&LargeResult,"test","read",Uuid::new_v4(),&AgentConfig::default(),&[],&mut|_,_|{}).await.unwrap();
        assert!(matches!(result.done_reason,DoneReason::Error));assert!(result.error.unwrap().contains("action may have occurred"));
        assert!(serde_json::to_vec(&result.messages).unwrap().len()<4096);
    }

    #[tokio::test]
    async fn invalid_direct_config_and_large_history_fail_before_model_request() {
        let provider=ScriptedProvider::new(vec![CompletionResponse{content:Some("unused".into()),tool_calls:vec![],finish_reason:Some("stop".into()),usage:None}]);
        let router=FakeRouter {calls:tokio::sync::Mutex::new(vec![])};
        for config in [AgentConfig{max_tokens:Some(0),..Default::default()},AgentConfig{max_iterations:1001,..Default::default()},AgentConfig{temperature:Some(f32::NAN),..Default::default()}] {
            assert!(run_agent_loop(&provider,&router,"test","read",Uuid::new_v4(),&config,&[],&mut|_,_|{}).await.is_err());
        }
        let history=vec![Message::assistant("x".repeat(MAX_CONVERSATION_BYTES))];
        assert!(run_agent_loop_with_history(&provider,&router,"test","read",AgentLoopContext { creature_id: Uuid::new_v4(), config: &AgentConfig::default(), available_tools: &[] },&history,&mut|_,_|{}).await.is_err());
        assert_eq!(provider.responses.lock().await.len(),1);
    }

    #[tokio::test]
    async fn tool_batch_stops_when_history_is_full_without_executing_remaining_calls() {
        struct Results(tokio::sync::Mutex<usize>);
        #[async_trait]
        impl ToolRouter for Results {
            async fn call(&self,_:RouterToolCall)->anyhow::Result<ToolResult>{*self.0.lock().await+=1;Ok(ToolResult{ok:true,output:serde_json::json!("x".repeat(100_000)),evidence:vec![]})}
            async fn list(&self)->anyhow::Result<Vec<ToolDescriptor>>{Ok(vec![])}
            async fn healthcheck(&self,_:&str)->anyhow::Result<bool>{Ok(true)}
        }
        let calls=(0..8).map(|n|ToolCall{id:n.to_string(),kind:"function".into(),function:chimera_llm::FunctionCall{name:"result".into(),arguments:"{}".into()}}).collect();
        let provider=ScriptedProvider::new(vec![CompletionResponse{content:None,tool_calls:calls,finish_reason:Some("tool_calls".into()),usage:None}]);
        let tools=Results(tokio::sync::Mutex::new(0));
        let history=vec![Message::assistant("x".repeat(MAX_CONVERSATION_BYTES-40_000))];
        let result=run_agent_loop_with_history(&provider,&tools,"test","read",AgentLoopContext { creature_id: Uuid::new_v4(), config: &AgentConfig::default(), available_tools: &[] },&history,&mut|_,_|{}).await.unwrap();
        assert!(matches!(result.done_reason,DoneReason::Error));assert!(result.error.unwrap().contains("remaining conversation limit"));
        assert_eq!(*tools.0.lock().await,1);assert!(serde_json::to_vec(&result.messages).unwrap().len()<=MAX_CONVERSATION_BYTES);
    }

}
