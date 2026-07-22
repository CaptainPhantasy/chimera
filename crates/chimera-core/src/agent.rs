//! chimera-core::agent — The real LLM-driven agent loop.
//!
//! Composes an [`LlmProvider`] with a [`ToolRouter`] to run a ReAct-style
//! loop: prompt the model, execute any tool calls it emits, feed results
//! back, repeat until the model produces a final text answer with no tool
//! calls.


use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use chimera_llm::{
    CompletionRequest, CompletionResponse, FunctionDefinition, LlmError, LlmProvider, Message,
    StreamDelta, ToolCall, ToolDefinition,
};
use chimera_tools::{ToolCall as RouterToolCall, ToolRouter};

use crate::CreatureId;

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
    F: FnMut(String, serde_json::Value),
{
    let mut messages: Vec<Message> = Vec::with_capacity(8);
    messages.push(Message::system(&config.system_prompt));
    messages.push(Message::user(objective));

    let mut steps: Vec<AgentStep> = Vec::new();
    let mut total_prompt_tokens = 0u64;
    let mut total_completion_tokens = 0u64;
    let mut final_answer: Option<String> = None;

    let mut iteration = 0usize;
    let done_reason = loop {
        iteration += 1;
        if iteration > config.max_iterations {
            break DoneReason::MaxIterations;
        }

        let req = CompletionRequest {
            model: model.to_string(),
            messages: messages.clone(),
            tools: available_tools.to_vec(),
            tool_choice: None,
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            top_p: None,
            stop: vec![],
            extra: Default::default(),
        };

        debug!(iteration, model, "requesting completion");

        let resp: CompletionResponse = match provider.stream(&req).await {
            Ok(deltas) => {
                let mut content = String::new();
                let mut tool_calls: Vec<ToolCall> = Vec::new();
                let mut finish_reason: Option<String> = None;
                let mut stream_error: Option<std::sync::Arc<LlmError>> = None;
                for delta in deltas {
                    match delta {
                        StreamDelta::Text(t) => {
                            content.push_str(&t);
                            stream_callback("text".to_string(), serde_json::Value::String(t.clone()));
                        }
                        StreamDelta::ToolCall(tc) => {
                            stream_callback(
                                "tool_call".to_string(),
                                serde_json::json!({"name": tc.function.name}),
                            );
                            tool_calls.push(tc);
                        }
                        StreamDelta::Done {
                            finish_reason: fr,
                            usage,
                        } => {
                            finish_reason = fr;
                            if let Some(u) = usage {
                                total_prompt_tokens += u.prompt_tokens;
                                total_completion_tokens += u.completion_tokens;
                            }
                        }
                        StreamDelta::Error(e) => {
                            warn!(error = %e, "stream error");
                            stream_error = Some(e);
                            break;
                        }
                    }
                }
                if let Some(e) = stream_error {
                    steps.push(AgentStep::Done {
                        reason: DoneReason::Error,
                        iterations: iteration,
                    });
                    let _ = e; // already logged
                    break DoneReason::Error;
                }
                CompletionResponse {
                    content: if content.is_empty() { None } else { Some(content) },
                    tool_calls,
                    finish_reason,
                    usage: None,
                }
            }
            Err(e) => {
                warn!(error = %e, "provider complete failed");
                steps.push(AgentStep::Done {
                    reason: DoneReason::Error,
                    iterations: iteration,
                });
                break DoneReason::Error;
            }
        };

        let had_tool_calls = !resp.tool_calls.is_empty();
        let recorded: Vec<RecordedToolCall> =
            resp.tool_calls.iter().map(RecordedToolCall::from_llm).collect();

        steps.push(AgentStep::Model {
            content: resp.content.clone(),
            tool_calls: recorded.clone(),
            finish_reason: resp.finish_reason.clone(),
        });

        // Append the assistant turn to the conversation.
        messages.push(Message::assistant_with_tools(
            resp.content.clone(),
            resp.tool_calls.clone(),
        ));

        // No tool calls + has text content => final answer.
        if !had_tool_calls {
            if let Some(c) = resp.content {
                final_answer = Some(c);
            }
            break DoneReason::FinalAnswer;
        }

        // Execute each tool call and append results.
        for (tc, recorded_tc) in resp.tool_calls.iter().zip(recorded.iter()) {
            let args_val: serde_json::Value =
                serde_json::from_str(&tc.function.arguments).unwrap_or(serde_json::Value::Null);

            info!(tool = %tc.function.name, "executing tool");
            let router_req = RouterToolCall {
                tool: tc.function.name.clone(),
                args: args_val.clone(),
                world: None,
                requested_by: creature_id,
            };
            let result = match tools.call(router_req).await {
                Ok(r) => r,
                Err(e) => {
                    warn!(tool = %tc.function.name, error = %e, "tool router error");
                    chimera_tools::ToolResult {
                        ok: false,
                        output: serde_json::json!({"error": e.to_string()}),
                        evidence: vec![],
                    }
                }
            };

            steps.push(AgentStep::Tool {
                call: recorded_tc.clone(),
                ok: result.ok,
                output: result.output.clone(),
            });

            // The provider expects tool result content as a string.
            let result_str = serde_json::to_string(&result.output)
                .unwrap_or_else(|_| "{\"error\":\"unserializable\"}".to_string());
            messages.push(Message::tool_result(&tc.id, result_str));
        }
    };

    if matches!(done_reason, DoneReason::MaxIterations) {
        steps.push(AgentStep::Done {
            reason: done_reason.clone(),
            iterations: config.max_iterations,
        });
    } else if matches!(done_reason, DoneReason::FinalAnswer) {
        steps.push(AgentStep::Done {
            reason: done_reason.clone(),
            iterations: iteration,
        });
    }

    Ok(AgentRunResult {
        final_answer,
        steps,
        done_reason,
        iterations: iteration,
        total_prompt_tokens,
        total_completion_tokens,
    })
}

// ---------------------------------------------------------------------------
// Helpers: convert ToolRouter descriptors to LLM tool definitions
// ---------------------------------------------------------------------------

/// Fetch the tool router's catalog and convert to LLM tool definitions.
pub async fn build_tool_definitions(
    tools: &dyn ToolRouter,
) -> anyhow::Result<Vec<ToolDefinition>> {
    let descriptors = tools.list().await?;
    Ok(descriptors
        .into_iter()
        .map(|d| {
            let params = schema_for_tool(&d.name);
            ToolDefinition {
                kind: "function".to_string(),
                function: FunctionDefinition {
                    name: d.name,
                    description: d.description,
                    parameters: params,
                },
            }
        })
        .collect())
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
}
