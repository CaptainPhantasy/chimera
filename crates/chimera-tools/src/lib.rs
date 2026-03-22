use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// ID types
// ---------------------------------------------------------------------------

pub type CreatureId = Uuid;
pub type WorldId = Uuid;

// ---------------------------------------------------------------------------
// Tool descriptor
// ---------------------------------------------------------------------------

/// Describes a tool available in the harness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {
    /// Unique tool name (e.g. "bash", "git", "mcp:search").
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Whether the tool is read-only.
    pub read_only: bool,
    /// Tool category for policy classification.
    pub category: ToolCategory,
}

/// Category of tool for policy and routing purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    LocalExecution,
    VersionControl,
    Mcp,
    BrowserControl,
    MobileControl,
    DesktopControl,
    NetworkedComms,
    Eval,
    Observability,
    Memory,
}

// ---------------------------------------------------------------------------
// Tool call / result
// ---------------------------------------------------------------------------

/// A request to invoke a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Which tool to invoke.
    pub tool: String,
    /// Arguments as structured JSON.
    pub args: serde_json::Value,
    /// Optional world to execute in.
    pub world: Option<WorldId>,
    /// Which creature is making the call.
    pub requested_by: CreatureId,
}

/// Evidence reference attached to a tool result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolEvidence {
    /// Kind label (e.g. "diff", "transcript", "screenshot").
    pub kind: String,
    /// URI or path to the artifact.
    pub uri: String,
}

/// The result of a tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// Whether the call succeeded.
    pub ok: bool,
    /// Structured output.
    pub output: serde_json::Value,
    /// Evidence artifacts produced by the call.
    pub evidence: Vec<ToolEvidence>,
}

// ---------------------------------------------------------------------------
// ToolRouter trait
// ---------------------------------------------------------------------------

/// Routes tool calls to their backends and returns results.
#[async_trait::async_trait]
pub trait ToolRouter: Send + Sync {
    /// Invoke a tool and return the result.
    async fn call(&self, req: ToolCall) -> anyhow::Result<ToolResult>;

    /// List all available tools.
    async fn list(&self) -> anyhow::Result<Vec<ToolDescriptor>>;

    /// Health-check a specific tool by name.
    async fn healthcheck(&self, tool: &str) -> anyhow::Result<bool>;
}
