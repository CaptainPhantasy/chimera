use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// MCP server descriptor
// ---------------------------------------------------------------------------

/// Describes an MCP server and its capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    /// Server name or identifier.
    pub name: String,
    /// Transport type (stdio, sse, streamable-http).
    pub transport: McpTransport,
    /// Command to start the server (for stdio transport).
    pub command: Option<String>,
    /// Arguments for the command.
    pub args: Vec<String>,
    /// Environment variables for the server process.
    pub env: std::collections::HashMap<String, String>,
}

/// MCP transport mechanism.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpTransport {
    /// Standard I/O transport.
    Stdio,
    /// Server-Sent Events transport.
    Sse { url: String },
    /// Streamable HTTP transport.
    StreamableHttp { url: String },
}

// ---------------------------------------------------------------------------
// MCP tool descriptor
// ---------------------------------------------------------------------------

/// Describes a tool available through MCP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDescriptor {
    /// Tool name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema for the tool's input parameters.
    pub input_schema: serde_json::Value,
    /// Which MCP server provides this tool.
    pub server: String,
}

// ---------------------------------------------------------------------------
// MCP resource
// ---------------------------------------------------------------------------

/// An MCP resource exposed by a server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    /// Resource URI.
    pub uri: String,
    /// Resource name.
    pub name: String,
    /// MIME type.
    pub mime_type: Option<String>,
    /// Description.
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// MCP invocation
// ---------------------------------------------------------------------------

/// Result of an MCP tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolResult {
    /// Whether the call succeeded.
    pub is_error: bool,
    /// Content blocks returned by the tool.
    pub content: Vec<McpContent>,
}

/// A content block in an MCP response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpContent {
    /// Text content.
    Text { text: String },
    /// Image content (base64).
    Image { data: String, mime_type: String },
    /// Resource reference.
    Resource { uri: String },
}

// ---------------------------------------------------------------------------
// McpClient trait
// ---------------------------------------------------------------------------

/// Client for MCP server communication: tool enumeration and invocation.
#[async_trait::async_trait]
pub trait McpClient: Send + Sync {
    /// List tools available from an MCP server.
    async fn list_tools(&self, server: &str) -> anyhow::Result<Vec<McpToolDescriptor>>;

    /// Invoke a tool on an MCP server.
    async fn invoke(
        &self,
        server: &str,
        tool: &str,
        args: serde_json::Value,
    ) -> anyhow::Result<McpToolResult>;

    /// List resources from an MCP server.
    async fn list_resources(&self, server: &str) -> anyhow::Result<Vec<McpResource>>;

    /// Read a resource by URI.
    async fn read_resource(&self, server: &str, uri: &str) -> anyhow::Result<McpContent>;

    /// Check connectivity to an MCP server.
    async fn ping(&self, server: &str) -> anyhow::Result<bool>;
}
