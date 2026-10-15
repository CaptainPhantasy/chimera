pub mod stdio;
pub use stdio::{StdioMcpClient, StdioOptions};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables for the server process.
    #[serde(default)]
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
    #[serde(default, rename = "mimeType", alias = "mime_type")]
    pub mime_type: Option<String>,
    /// Description.
    #[serde(default)]
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
    Audio { data: String, mime_type: String },
    EmbeddedResource { resource: McpResourceContents },
    ResourceBundle { contents: Vec<McpResourceContents> },
    ResourceLink { uri: String, name: String, metadata: serde_json::Value },
    Structured { data: serde_json::Value },
}

/// Resource content preserves its URI, media type, and either text or base64 data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceContents {
    pub uri: String,
    #[serde(default, rename = "mimeType", alias = "mime_type")]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub blob: Option<String>,
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

// ---------------------------------------------------------------------------
// Saved connection file
// ---------------------------------------------------------------------------

/// Connections live in one regular JSON file so the file can be inspected and
/// backed up like the CLI's `--mcp-config` input.
pub const MAX_CONFIG_BYTES: u64 = 1_048_576;
const MAX_SERVERS: usize = 16;

/// Resolve the saved connection file: `$CHIMERA_MCP_CONFIG` or
/// `~/.floyd-code/mcp.json`.
pub fn default_config_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("CHIMERA_MCP_CONFIG") {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join(".floyd-code").join("mcp.json"))
}

/// Check one entry a user just composed. Only the implemented stdio transport
/// is accepted, so a saved entry always matches what this client can start.
pub fn validate_server(server: &McpServer) -> Result<()> {
    let name = server.name.trim();
    anyhow::ensure!(
        !name.is_empty() && name.len() <= 128 && !name.chars().any(char::is_control),
        "connection name must be 1 to 128 characters without control characters"
    );
    anyhow::ensure!(
        matches!(server.transport, McpTransport::Stdio),
        "only stdio connections (a local program CHIMERA starts) are supported; remote transports would be advertised without working support"
    );
    let command = server.command.as_deref().unwrap_or_default().trim();
    anyhow::ensure!(
        !command.is_empty() && !command.chars().any(char::is_control),
        "connection command must be a non-empty program path without control characters"
    );
    for argument in &server.args {
        anyhow::ensure!(
            !argument.chars().any(char::is_control),
            "connection arguments must not contain control characters"
        );
    }
    anyhow::ensure!(
        server.env.len() <= 64,
        "a connection supports at most 64 environment variables"
    );
    for (key, value) in &server.env {
        anyhow::ensure!(
            !key.is_empty()
                && key.len() <= 256
                && !key.contains('=')
                && !key.chars().any(char::is_control),
            "environment variable name {key:?} must be 1 to 256 characters without '=' or control characters"
        );
        anyhow::ensure!(
            value.len() <= 16_384 && !value.chars().any(char::is_control),
            "environment variable {key:?} must be at most 16384 characters without control characters"
        );
    }
    Ok(())
}

/// Read the saved connection entries. A missing file means no connections yet.
pub fn load_servers(path: &Path) -> Result<Vec<McpServer>> {
    let meta = std::fs::symlink_metadata(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    anyhow::ensure!(
        meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= MAX_CONFIG_BYTES,
        "connection settings must be a regular file no larger than 1 MiB: {}",
        path.display()
    );
    let servers: Vec<McpServer> =
        serde_json::from_slice(&std::fs::read(path)?).with_context(|| {
            format!(
                "{} is not a valid connection list; fix or remove the file",
                path.display()
            )
        })?;
    anyhow::ensure!(
        servers.len() <= MAX_SERVERS,
        "{} holds more than {MAX_SERVERS} connections",
        path.display()
    );
    Ok(servers)
}

/// Save one entry, replacing only an entry with the same name and leaving every
/// other entry untouched. The write replaces the file atomically.
pub fn save_server(path: &Path, server: McpServer) -> Result<()> {
    validate_server(&server)?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let mut servers = if path.exists() {
        load_servers(path)?
    } else {
        Vec::new()
    };
    // Replace an entry with the same name in place; every other entry,
    // including its position, is preserved.
    match servers.iter_mut().find(|entry| entry.name == server.name) {
        Some(entry) => *entry = server,
        None => {
            anyhow::ensure!(
                servers.len() < MAX_SERVERS,
                "{MAX_SERVERS} connections are already saved; remove one before adding another"
            );
            servers.push(server);
        }
    }
    let body = serde_json::to_vec_pretty(&servers)?;
    let temp = path.with_extension(format!("json.tmp-{}", uuid::Uuid::new_v4()));
    #[cfg(unix)]
    let written = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
    };
    #[cfg(not(unix))]
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp);
    let mut file = written.with_context(|| format!("could not create {}", temp.display()))?;
    {
        use std::io::Write;
        file.write_all(&body)?;
        file.sync_all()?;
    }
    drop(file);
    std::fs::rename(&temp, path).with_context(|| format!("could not update {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio(name: &str, command: &str) -> McpServer {
        McpServer {
            name: name.into(),
            transport: McpTransport::Stdio,
            command: Some(command.into()),
            args: vec!["--serve".into()],
            env: std::collections::HashMap::from([("SERVICE_KEY".into(), "value".into())]),
        }
    }

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

    #[test]
    fn mock_mcp_client_default() {
        let client = MockMcpClient::default();
        assert_eq!(client.tools.len(), 2);
    }

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!("chimera-mcp-config-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn validate_rejects_unsupported_or_empty_entries() {
        assert!(validate_server(&stdio("fixture", "python3")).is_ok());
        let mut server = stdio("  ", "python3");
        assert!(validate_server(&server).is_err());
        server.name = "ok".into();
        server.transport = McpTransport::StreamableHttp {
            url: "https://x.test".into(),
        };
        assert!(validate_server(&server).is_err());
        server.transport = McpTransport::Stdio;
        server.command = Some("  ".into());
        assert!(validate_server(&server).is_err());
        server.command = Some("python3".into());
        server.env.insert("BAD NAME\n".into(), "x".into());
        assert!(validate_server(&server).is_err());
        server.env.clear();
        server.env.insert("A=B".into(), "x".into());
        assert!(validate_server(&server).is_err());
        server.env.clear();
        server.args.push("bad\narg".into());
        assert!(validate_server(&server).is_err());
    }

    #[test]
    fn save_adds_entries_and_replaces_only_the_same_name() {
        let path = temp_path();
        save_server(&path, stdio("alpha", "python3")).unwrap();
        save_server(&path, stdio("beta", "python3")).unwrap();
        let replaced = {
            let mut server = stdio("alpha", "deno");
            server.env.clear();
            server
        };
        save_server(&path, replaced).unwrap();
        let servers = load_servers(&path).unwrap();
        let names = servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names, ["alpha", "beta"]);
        assert_eq!(servers[0].command.as_deref(), Some("deno"));
        assert_eq!(
            servers[1].env.get("SERVICE_KEY").map(String::as_str),
            Some("value")
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_never_touches_a_damaged_or_oversized_file() {
        let path = temp_path();
        std::fs::write(&path, "not json").unwrap();
        let error = save_server(&path, stdio("alpha", "python3")).unwrap_err();
        assert!(error.to_string().contains("not a valid connection list"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json");
        std::fs::write(
            &path,
            serde_json::to_vec(&vec![stdio("a", "x"); 17]).unwrap(),
        )
        .unwrap();
        let error = load_servers(&path).unwrap_err();
        assert!(error.to_string().contains("more than 16"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_reports_a_missing_file_differently_from_a_read_failure() {
        assert!(std::fs::symlink_metadata(temp_path()).is_err());
        let path = temp_path();
        assert!(load_servers(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
