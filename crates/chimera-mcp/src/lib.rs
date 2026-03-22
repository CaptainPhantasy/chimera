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

// ---------------------------------------------------------------------------
// Mock MCP client
// ---------------------------------------------------------------------------

/// A mock MCP client that returns canned responses.
pub struct MockMcpClient {
    tools: Vec<McpToolDescriptor>,
    resources: Vec<McpResource>,
}

impl MockMcpClient {
    pub fn new() -> Self {
        Self {
            tools: vec![
                McpToolDescriptor {
                    name: "search".into(),
                    description: "Search the codebase".into(),
                    input_schema: serde_json::json!({"type": "object", "properties": {"query": {"type": "string"}}}),
                    server: "test-server".into(),
                },
                McpToolDescriptor {
                    name: "read_file".into(),
                    description: "Read a file".into(),
                    input_schema: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
                    server: "test-server".into(),
                },
            ],
            resources: vec![McpResource {
                uri: "file:///README.md".into(),
                name: "README".into(),
                mime_type: Some("text/markdown".into()),
                description: Some("Project readme".into()),
            }],
        }
    }
}

impl Default for MockMcpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl McpClient for MockMcpClient {
    async fn list_tools(&self, _server: &str) -> anyhow::Result<Vec<McpToolDescriptor>> {
        Ok(self.tools.clone())
    }

    async fn invoke(&self, _server: &str, tool: &str, args: serde_json::Value) -> anyhow::Result<McpToolResult> {
        Ok(McpToolResult {
            is_error: false,
            content: vec![McpContent::Text {
                text: format!("result of {}({})", tool, args),
            }],
        })
    }

    async fn list_resources(&self, _server: &str) -> anyhow::Result<Vec<McpResource>> {
        Ok(self.resources.clone())
    }

    async fn read_resource(&self, _server: &str, uri: &str) -> anyhow::Result<McpContent> {
        Ok(McpContent::Text {
            text: format!("content of {}", uri),
        })
    }

    async fn ping(&self, _server: &str) -> anyhow::Result<bool> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn list_tools_returns_registered() {
        let client = MockMcpClient::new();
        let tools = client.list_tools("test-server").await.unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "search");
        assert_eq!(tools[1].name, "read_file");
    }

    #[tokio::test]
    async fn invoke_returns_result() {
        let client = MockMcpClient::new();
        let result = client
            .invoke("test-server", "search", serde_json::json!({"query": "auth"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            McpContent::Text { text } => assert!(text.contains("search")),
            _ => panic!("expected text content"),
        }
    }

    #[tokio::test]
    async fn list_resources_returns_registered() {
        let client = MockMcpClient::new();
        let resources = client.list_resources("test-server").await.unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].uri, "file:///README.md");
    }

    #[tokio::test]
    async fn read_resource_returns_content() {
        let client = MockMcpClient::new();
        let content = client.read_resource("test-server", "file:///README.md").await.unwrap();
        match content {
            McpContent::Text { text } => assert!(text.contains("README.md")),
            _ => panic!("expected text content"),
        }
    }

    #[tokio::test]
    async fn ping_returns_true() {
        let client = MockMcpClient::new();
        assert!(client.ping("test-server").await.unwrap());
    }
}
