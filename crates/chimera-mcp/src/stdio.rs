//! Local MCP transport. Server text and tool descriptions are untrusted data;
//! callers must enforce their own permissions before starting a server or tool.
//! Implements the 2025-11-25 stdio lifecycle with compatible older negotiation.
//! Verified against the official specification on 2026-10-11:
//! https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle
//! https://modelcontextprotocol.io/specification/2025-11-25/basic/transports
//! https://modelcontextprotocol.io/specification/2025-11-25/server/tools
//! https://modelcontextprotocol.io/specification/2025-11-25/server/resources

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::{McpClient, McpContent, McpResource, McpResourceContents, McpServer,
    McpToolDescriptor, McpToolResult, McpTransport};

const PROTOCOL_VERSION: &str = "2025-11-25";
const SUPPORTED_VERSIONS: &[&str] = &[PROTOCOL_VERSION, "2025-06-18", "2025-03-26", "2024-11-05"];

#[derive(Debug, Clone)]
pub struct StdioOptions {
    pub request_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub max_message_bytes: usize,
    pub max_pages: usize,
    pub working_directory: Option<PathBuf>,
}

impl Default for StdioOptions {
    fn default() -> Self {
        Self { request_timeout: Duration::from_secs(30), shutdown_timeout: Duration::from_millis(250),
            max_message_bytes: 4 * 1024 * 1024, max_pages: 100, working_directory: None }
    }
}

struct ServerEntry {
    config: McpServer,
    connection: Mutex<Option<Connection>>,
}

/// Connections are owned by each in-flight request, so cancelling its future
/// drops and terminates the child. Failed tool calls are never automatically replayed.
pub struct StdioMcpClient {
    servers: HashMap<String, ServerEntry>,
    options: StdioOptions,
}

impl StdioMcpClient {
    /// Registration does not start commands. The first explicit operation connects.
    pub fn new(servers: Vec<McpServer>, options: StdioOptions) -> Result<Self> {
        ensure!(!options.request_timeout.is_zero(), "MCP request timeout must be positive");
        ensure!(!options.shutdown_timeout.is_zero(), "MCP shutdown timeout must be positive");
        ensure!(options.max_message_bytes > 0 && options.max_message_bytes <= 64 * 1024 * 1024, "invalid MCP message byte limit");
        ensure!(options.max_pages > 0 && options.max_pages <= 1000, "invalid MCP page limit");
        let mut entries = HashMap::new();
        for config in servers {
            ensure!(!config.name.trim().is_empty(), "MCP server name must not be empty");
            ensure!(matches!(config.transport, McpTransport::Stdio), "MCP server {}: only stdio transport is implemented", config.name);
            ensure!(config.command.as_deref().is_some_and(|command| !command.trim().is_empty()), "MCP server {} needs a command", config.name);
            let name = config.name.clone();
            ensure!(!entries.contains_key(&name), "duplicate MCP server {name}");
            entries.insert(name, ServerEntry { config, connection: Mutex::new(None) });
        }
        Ok(Self { servers: entries, options })
    }

    pub fn server_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.servers.keys().cloned().collect();
        names.sort();
        names
    }

    /// Connect explicitly and return negotiated capabilities. Callers must obtain
    /// permission to start the configured process before using this method.
    pub async fn server_capabilities(&self, server: &str) -> Result<Value> {
        let entry = self.entry(server)?;
        let mut slot = timeout(self.options.request_timeout, entry.connection.lock()).await
            .context("timed out waiting for server capabilities")?;
        if slot.is_none() { *slot = Some(Connection::connect(&entry.config, &self.options).await?); }
        Ok(slot.as_ref().expect("connected above").capabilities.clone())
    }

    fn entry(&self, server: &str) -> Result<&ServerEntry> {
        self.servers.get(server).with_context(|| format!("unknown MCP server {server}"))
    }

    async fn request(&self, server: &str, method: &str, params: Value) -> Result<Value> {
        let entry = self.entry(server)?;
        let mut slot = timeout(self.options.request_timeout, entry.connection.lock()).await
            .with_context(|| format!("MCP server {server}: timed out waiting for another request"))?;
        let mut connection = match slot.take() {
            Some(connection) => connection,
            None => Connection::connect(&entry.config, &self.options).await?,
        };
        if method.starts_with("tools/") && connection.capabilities.get("tools").is_none() {
            *slot = Some(connection);
            bail!("MCP server {server} did not advertise tools");
        }
        if method.starts_with("resources/") && connection.capabilities.get("resources").is_none() {
            *slot = Some(connection);
            bail!("MCP server {server} did not advertise resources");
        }
        let request_id = connection.next_id;
        let result = timeout(self.options.request_timeout, connection.rpc(method, params, self.options.max_message_bytes)).await;
        match result {
            Ok(Ok(value)) => { *slot = Some(connection); Ok(value) }
            Ok(Err(error)) => {
                connection.close(self.options.shutdown_timeout).await;
                Err(error).with_context(|| format!("MCP server {server}, {method}; request was not retried"))
            }
            Err(_) => {
                // Best effort notification is bounded too; stop the child afterward.
                let _ = timeout(Duration::from_millis(50), connection.send(&json!({"jsonrpc":"2.0", "method":"notifications/cancelled", "params":{"requestId":request_id,"reason":"request timeout"}}), self.options.max_message_bytes)).await;
                connection.close(self.options.shutdown_timeout).await;
                bail!("MCP server {server}, {method}: request timed out; request was not retried");
            }
        }
    }

    /// Explicit reconnection does not repeat the interrupted operation.
    pub async fn reconnect(&self, server: &str) -> Result<()> {
        self.disconnect(server).await?;
        ensure!(self.ping(server).await?, "MCP server {server} did not answer ping");
        Ok(())
    }

    pub async fn disconnect(&self, server: &str) -> Result<()> {
        let entry = self.entry(server)?;
        let mut slot = timeout(self.options.request_timeout, entry.connection.lock()).await
            .context("timed out waiting to disconnect MCP server")?;
        if let Some(mut connection) = slot.take() { connection.close(self.options.shutdown_timeout).await; }
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<()> {
        for name in self.server_names() { self.disconnect(&name).await?; }
        Ok(())
    }

    async fn paginated(&self, server: &str, method: &str, field: &str) -> Result<Vec<Value>> {
        let mut all = Vec::new();
        let mut cursor = None;
        let mut seen = HashSet::new();
        let mut catalog_bytes = 0usize;
        for _ in 0..self.options.max_pages {
            let params = cursor.as_ref().map(|cursor| json!({"cursor":cursor})).unwrap_or_else(|| json!({}));
            let result = self.request(server, method, params).await?;
            let values = result.get(field).and_then(Value::as_array).context("MCP list result is missing its array")?;
            catalog_bytes = catalog_bytes.saturating_add(serde_json::to_vec(values)?.len());
            ensure!(catalog_bytes <= self.options.max_message_bytes.saturating_mul(4).min(64 * 1024 * 1024), "MCP catalog exceeds aggregate byte limit");
            all.extend(values.iter().cloned());
            ensure!(all.len() <= 10000, "MCP catalog exceeds 10000 entries");
            match result.get("nextCursor") {
                None | Some(Value::Null) => return Ok(all),
                Some(Value::String(next)) if !next.is_empty() => {
                    ensure!(seen.insert(next.clone()), "MCP server repeated its pagination cursor");
                    cursor = Some(next.clone());
                }
                _ => bail!("MCP server returned an invalid pagination cursor"),
            }
        }
        bail!("MCP catalog exceeds configured page limit")
    }

    /// Preserve all server-provided metadata, including optional schemas and annotations.
    /// None of these fields grants permission to invoke a tool.
    pub async fn list_tools_raw(&self, server: &str) -> Result<Vec<Value>> {
        self.paginated(server, "tools/list", "tools").await
    }

    pub async fn tool_details(&self, server: &str, name: &str) -> Result<Value> {
        self.list_tools_raw(server).await?.into_iter()
            .find(|value| value.get("name").and_then(Value::as_str) == Some(name))
            .with_context(|| format!("unknown MCP tool {server}/{name}"))
    }

    pub async fn inspect_tool(&self, server: &str, name: &str) -> Result<McpToolDescriptor> {
        self.list_tools(server).await?.into_iter().find(|tool| tool.name == name)
            .with_context(|| format!("unknown MCP tool {server}/{name}"))
    }

    pub async fn read_resource_contents(&self, server: &str, uri: &str) -> Result<Vec<McpResourceContents>> {
        ensure!(!uri.trim().is_empty(), "MCP resource URI must not be empty");
        let result = self.request(server, "resources/read", json!({"uri":uri})).await?;
        let values = result.get("contents").and_then(Value::as_array).context("MCP resource result has no contents")?;
        ensure!(!values.is_empty(), "MCP resource result is empty");
        values.iter().map(|value| {
            let contents: McpResourceContents = serde_json::from_value(value.clone()).context("invalid MCP resource contents")?;
            ensure!(contents.text.is_some() != contents.blob.is_some(), "MCP resource needs exactly one of text or blob");
            ensure!(!contents.uri.is_empty(), "MCP resource URI must not be empty");
            Ok(contents)
        }).collect()
    }
}

#[async_trait::async_trait]
impl McpClient for StdioMcpClient {
    async fn list_tools(&self, server: &str) -> Result<Vec<McpToolDescriptor>> {
        let raw = self.list_tools_raw(server).await?;
        let mut names = HashSet::new();
        raw.into_iter().map(|value| {
            let name = value.get("name").and_then(Value::as_str).filter(|name| !name.is_empty()).context("MCP tool name missing")?;
            ensure!(names.insert(name.to_string()), "duplicate MCP tool name {name}");
            let input_schema = value.get("inputSchema").filter(|schema| schema.is_object()).context("MCP tool inputSchema missing")?.clone();
            Ok(McpToolDescriptor { name: name.to_string(), description: value.get("description").and_then(Value::as_str).unwrap_or_default().into(), input_schema, server: server.to_string() })
        }).collect()
    }

    async fn invoke(&self, server: &str, tool: &str, args: Value) -> Result<McpToolResult> {
        ensure!(args.is_object(), "MCP tool arguments must be an object");
        self.inspect_tool(server, tool).await?;
        let result = self.request(server, "tools/call", json!({"name":tool,"arguments":args})).await?;
        let is_error = match result.get("isError") { None => false, Some(value) => value.as_bool().context("invalid MCP isError value")? };
        let mut content: Vec<McpContent> = match result.get("content") {
            Some(Value::Array(blocks)) => blocks.iter().map(parse_content).collect::<Result<_>>()?,
            None if result.get("structuredContent").is_some() => Vec::new(),
            _ => bail!("MCP tool result has no content array"),
        };
        // Preserve structured-only and structured+unstructured results without changing
        // the established McpToolResult struct used by existing callers.
        if let Some(value) = result.get("structuredContent") {
            ensure!(value.is_object(), "MCP structuredContent must be an object");
            content.push(McpContent::Structured { data: value.clone() });
        }
        Ok(McpToolResult { is_error, content })
    }

    async fn list_resources(&self, server: &str) -> Result<Vec<McpResource>> {
        self.paginated(server, "resources/list", "resources").await?.into_iter().map(|value| {
            let resource: McpResource = serde_json::from_value(value).context("invalid MCP resource descriptor")?;
            ensure!(!resource.uri.is_empty() && !resource.name.is_empty(), "MCP resource needs a name and URI");
            Ok(resource)
        }).collect()
    }

    async fn read_resource(&self, server: &str, uri: &str) -> Result<McpContent> {
        let mut contents = self.read_resource_contents(server, uri).await?;
        if contents.len() == 1 { Ok(McpContent::EmbeddedResource { resource: contents.remove(0) }) }
        else { Ok(McpContent::ResourceBundle { contents }) }
    }

    async fn ping(&self, server: &str) -> Result<bool> {
        let result = self.request(server, "ping", json!({})).await?;
        ensure!(result.is_object(), "MCP ping returned an invalid result");
        Ok(true)
    }
}

fn parse_content(value: &Value) -> Result<McpContent> {
    let field = |name| value.get(name).and_then(Value::as_str).map(str::to_string)
        .with_context(|| format!("MCP content missing {name}"));
    match value.get("type").and_then(Value::as_str) {
        Some("text") => Ok(McpContent::Text { text: field("text")? }),
        Some("image") => Ok(McpContent::Image { data: field("data")?, mime_type: field("mimeType")? }),
        Some("audio") => Ok(McpContent::Audio { data: field("data")?, mime_type: field("mimeType")? }),
        Some("resource") => {
            let resource: McpResourceContents = serde_json::from_value(value.get("resource").context("MCP embedded resource missing")?.clone())?;
            ensure!(resource.text.is_some() != resource.blob.is_some(), "MCP embedded resource needs text or blob");
            Ok(McpContent::EmbeddedResource { resource })
        }
        Some("resource_link") => Ok(McpContent::ResourceLink { uri: field("uri")?, name: field("name")?, metadata: value.clone() }),
        other => bail!("unsupported MCP content type {other:?}"),
    }
}

struct Connection {
    child: Child,
    process_group: Option<u32>,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    next_id: u64,
    capabilities: Value,
}

impl Connection {
    async fn connect(server: &McpServer, options: &StdioOptions) -> Result<Self> {
        let mut command = Command::new(server.command.as_deref().context("MCP server has no command")?);
        command.args(&server.args).env_clear();
        for name in ["PATH", "HOME", "LANG", "LC_ALL", "TMPDIR", "SystemRoot", "WINDIR"] {
            if let Some(value) = std::env::var_os(name) { command.env(name, value); }
        }
        command.envs(&server.env).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
        if let Some(path) = &options.working_directory { command.current_dir(path); }
        #[cfg(unix)]
        { command.process_group(0); }
        let mut child = command.spawn().with_context(|| format!("could not start MCP server {}", server.name))?;
        let process_group = child.id();
        let input = child.stdin.take().context("MCP stdin was not captured")?;
        let output = child.stdout.take().context("MCP stdout was not captured")?;
        let mut connection = Self { child, process_group, input: Some(input), output: BufReader::new(output), next_id: 1, capabilities: json!({}) };
        let initialized = timeout(options.request_timeout, async {
            let result = connection.rpc("initialize", json!({
                "protocolVersion":PROTOCOL_VERSION,"capabilities":{},
                "clientInfo":{"name":"chimera","version":env!("CARGO_PKG_VERSION")}
            }), options.max_message_bytes).await?;
            let version = result.get("protocolVersion").and_then(Value::as_str).context("MCP initialize has no protocolVersion")?;
            ensure!(SUPPORTED_VERSIONS.contains(&version), "unsupported MCP protocol version {version}");
            connection.capabilities = result.get("capabilities").filter(|value| value.is_object()).context("MCP initialize has no capabilities")?.clone();
            for name in ["tools", "resources"] {
                if let Some(capability) = connection.capabilities.get(name) { ensure!(capability.is_object(), "invalid MCP capability {name}"); }
            }
            let info = result.get("serverInfo").context("MCP initialize has no serverInfo")?;
            ensure!(info.get("name").and_then(Value::as_str).is_some() && info.get("version").and_then(Value::as_str).is_some(), "invalid MCP serverInfo");
            connection.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}), options.max_message_bytes).await
        }).await;
        match initialized {
            Ok(Ok(())) => Ok(connection),
            Ok(Err(error)) => { connection.close(options.shutdown_timeout).await; Err(error).context("MCP initialization failed") }
            Err(_) => { connection.close(options.shutdown_timeout).await; bail!("MCP initialization timed out") }
        }
    }

    async fn send(&mut self, value: &Value, limit: usize) -> Result<()> {
        let mut bytes = serde_json::to_vec(value)?;
        ensure!(bytes.len() <= limit, "MCP outgoing message exceeds byte limit");
        bytes.push(b'\n');
        let input = self.input.as_mut().context("MCP input is closed")?;
        input.write_all(&bytes).await.context("MCP input disconnected")?;
        input.flush().await.context("MCP flush failed")
    }

    async fn read(&mut self, limit: usize) -> Result<Value> {
        let mut bytes = Vec::new();
        loop {
            let chunk = self.output.fill_buf().await.context("MCP output read failed")?;
            ensure!(!chunk.is_empty(), "MCP server disconnected before responding");
            let end = chunk.iter().position(|byte| *byte == b'\n');
            let count = end.map(|index| index + 1).unwrap_or(chunk.len());
            ensure!(bytes.len().saturating_add(count) <= limit, "MCP response exceeds message byte limit");
            bytes.extend_from_slice(&chunk[..count]);
            self.output.consume(count);
            if end.is_some() { break; }
        }
        let message: Value = serde_json::from_slice(&bytes).context("MCP server sent invalid JSON")?;
        ensure!(message.get("jsonrpc").and_then(Value::as_str) == Some("2.0"), "MCP server sent an invalid JSON-RPC version");
        Ok(message)
    }

    async fn rpc(&mut self, method: &str, params: Value, limit: usize) -> Result<Value> {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).context("MCP request identifier limit reached")?;
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}), limit).await?;
        for _ in 0..1024 {
            let message = self.read(limit).await?;
            if let Some(server_method) = message.get("method").and_then(Value::as_str) {
                if let Some(request_id) = message.get("id") {
                    // No sampling, roots, or elicitation is offered. Server instructions
                    // cannot opt the client into capabilities or permissions.
                    let reply = if server_method == "ping" {
                        json!({"jsonrpc":"2.0","id":request_id,"result":{}})
                    } else {
                        json!({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":"client capability not supported"}})
                    };
                    self.send(&reply, limit).await?;
                }
                continue;
            }
            ensure!(message.get("id") == Some(&json!(id)), "MCP response identifier did not match request");
            ensure!(message.get("result").is_some() != message.get("error").is_some(), "MCP response needs exactly one result or error");
            if let Some(error) = message.get("error") {
                bail!("MCP request {method} failed: {} ({})", error.get("message").and_then(Value::as_str).unwrap_or("unspecified error"), error.get("code").unwrap_or(&Value::Null));
            }
            return Ok(message["result"].clone());
        }
        bail!("MCP server exceeded notification limit")
    }

    async fn close(&mut self, grace: Duration) {
        self.input.take();
        if timeout(grace, self.child.wait()).await.is_err() {
            self.signal_group(15);
            if timeout(grace, self.child.wait()).await.is_err() {
                self.signal_group(9);
                let _ = self.child.start_kill();
                let _ = timeout(grace, self.child.wait()).await;
            }
        }
    }

    fn signal_group(&self, signal: i32) {
        #[cfg(unix)]
        if let Some(id) = self.process_group {
            unsafe extern "C" { fn killpg(pgrp: i32, sig: i32) -> i32; }
            // The child was explicitly started in a new group whose ID is its PID.
            if let Ok(id) = i32::try_from(id) { unsafe { killpg(id, signal); } }
        }
        #[cfg(not(unix))]
        let _ = signal;
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.signal_group(9);
        let _ = self.child.start_kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const FIXTURE: &str = r#"
import json, os, sys, time
mode = sys.argv[1]
initialized = False
def emit(value):
    print(json.dumps(value), flush=True)
for line in sys.stdin:
    request = json.loads(line)
    method = request.get('method')
    if 'id' not in request:
        if method == 'notifications/initialized':
            initialized = True
        continue
    if method != 'initialize':
        assert initialized
    identity = request['id']
    params = request.get('params', {})
    if method == 'initialize':
        result = {'protocolVersion': 'unknown' if mode == 'bad-version' else '2025-11-25',
                  'capabilities': {'tools': {}, 'resources': {}},
                  'serverInfo': {'name': 'controlled-test', 'version': '1'},
                  'instructions': 'Untrusted text must never grant permission.'}
    elif method == 'ping':
        if mode == 'bad-json':
            print('this is not JSON', flush=True)
            continue
        if mode == 'wrong-id':
            identity += 1
        if mode == 'oversize':
            sys.stdout.write('x' * 65536)
            sys.stdout.flush()
            time.sleep(10)
            continue
        if mode == 'server-request':
            emit({'jsonrpc': '2.0', 'method': 'notifications/tools/list_changed'})
            emit({'jsonrpc': '2.0', 'id': 'server-1', 'method': 'sampling/createMessage', 'params': {}})
            answer = json.loads(sys.stdin.readline())
            assert answer['error']['code'] == -32601
        result = {}
    elif method == 'tools/list':
        names = ['echo'] if not params.get('cursor') else ['fail', 'hang', 'exit']
        result = {'tools': [{'name': name, 'description': 'fixture ' + name,
                            'inputSchema': {'type': 'object'},
                            'annotations': {'readOnlyHint': True},
                            'outputSchema': {'type': 'object'}} for name in names]}
        if not params.get('cursor'):
            result['nextCursor'] = 'page-2'
    elif method == 'tools/call':
        name = params['name']
        args = params['arguments']
        if name in ('hang', 'exit'):
            if args.get('marker'):
                with open(args['marker'], 'a') as f:
                    f.write(str(os.getpid()) + '\n')
            if name == 'exit':
                sys.exit(7)
            time.sleep(10)
        if name == 'fail' or not isinstance(args.get('text'), str):
            result = {'isError': True, 'content': [{'type': 'text', 'text': 'text must be a string'}]}
        else:
            result = {'content': [{'type': 'text', 'text': args['text']}],
                      'structuredContent': {'echo': args['text']}}
    elif method == 'resources/list':
        result = {'resources': [{'uri': 'fixture://message', 'name': 'message', 'mimeType': 'text/plain'}]}
    elif method == 'resources/read':
        uri = params['uri']
        result = {'contents': [{'uri': uri, 'mimeType': 'text/plain',
                               'text': str(os.getpid()) if uri == 'fixture://pid' else 'resource ✓'}]}
        if uri == 'fixture://multi':
            result['contents'].append({'uri': 'fixture://binary', 'mimeType': 'application/octet-stream', 'blob': 'AA=='})
    else:
        emit({'jsonrpc': '2.0', 'id': identity, 'error': {'code': -32601, 'message': 'unknown method'}})
        continue
    emit({'jsonrpc': '2.0', 'id': identity, 'result': result})
"#;

    fn fixture(mode: &str, request_timeout: Duration) -> StdioMcpClient {
        let server = McpServer { name: "fixture".into(), transport: McpTransport::Stdio,
            command: Some(std::env::var("CHIMERA_TEST_PYTHON").unwrap_or_else(|_| "python3".into())),
            args: vec!["-u".into(), "-c".into(), FIXTURE.into(), mode.into()], env: HashMap::new() };
        StdioMcpClient::new(vec![server], StdioOptions { request_timeout, ..Default::default() }).unwrap()
    }

    async fn pid(client: &StdioMcpClient) -> u32 {
        client.read_resource_contents("fixture", "fixture://pid").await.unwrap()[0].text.as_ref().unwrap().parse().unwrap()
    }

    fn marker() -> PathBuf {
        std::env::temp_dir().join(format!("chimera-mcp-test-{}.txt", uuid::Uuid::new_v4()))
    }

    #[cfg(unix)]
    async fn assert_child_stopped(pid: u32) {
        unsafe extern "C" { fn kill(pid: i32, sig: i32) -> i32; }
        for _ in 0..100 {
            if unsafe { kill(pid as i32, 0) } != 0 { return; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("MCP child {pid} remained alive after cleanup");
    }

    #[tokio::test]
    async fn real_stdio_initializes_discovers_calls_and_reads_resources() {
        let client = fixture("normal", Duration::from_secs(3));
        assert!(client.ping("fixture").await.unwrap());
        let tools = client.list_tools("fixture").await.unwrap();
        assert_eq!(tools.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(), ["echo", "fail", "hang", "exit"]);
        assert!(tools.iter().all(|tool| tool.server == "fixture"));
        assert!(client.tool_details("fixture", "echo").await.unwrap().get("outputSchema").is_some());
        let result = client.invoke("fixture", "echo", json!({"text":"hello 🌍"})).await.unwrap();
        assert!(!result.is_error);
        assert!(matches!(&result.content[0], McpContent::Text { text } if text == "hello 🌍"));
        assert!(matches!(&result.content[1], McpContent::Structured { data } if data["echo"] == "hello 🌍"));
        let resources = client.list_resources("fixture").await.unwrap();
        assert_eq!(resources[0].mime_type.as_deref(), Some("text/plain"));
        assert_eq!(client.read_resource_contents("fixture", "fixture://message").await.unwrap()[0].text.as_deref(), Some("resource ✓"));
        assert!(matches!(client.read_resource("fixture", "fixture://multi").await.unwrap(), McpContent::ResourceBundle { contents } if contents.len() == 2));
        client.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn invalid_arguments_and_server_tool_failure_are_not_success() {
        let client = fixture("normal", Duration::from_secs(3));
        assert!(client.invoke("fixture", "echo", json!([])).await.is_err());
        assert!(client.invoke("fixture", "not-a-tool", json!({})).await.is_err());
        assert!(client.invoke("fixture", "echo", json!({"text":5})).await.unwrap().is_error);
        assert!(client.invoke("fixture", "fail", json!({"text":"hello"})).await.unwrap().is_error);
        client.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn healthcheck_requires_real_valid_response_and_rejects_bad_handshake() {
        for mode in ["bad-version", "bad-json", "wrong-id"] {
            let client = fixture(mode, Duration::from_secs(3));
            assert!(client.ping("fixture").await.is_err(), "{mode}");
        }
        let client = fixture("normal", Duration::from_secs(3));
        assert!(client.ping("not-configured").await.is_err());
    }

    #[tokio::test]
    async fn server_cannot_enable_unoffered_client_capabilities() {
        let client = fixture("server-request", Duration::from_secs(3));
        assert!(client.ping("fixture").await.unwrap());
        client.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn timeout_disconnect_and_reconnection_do_not_repeat_calls() {
        let client = fixture("normal", Duration::from_millis(500));
        let first_pid = pid(&client).await;
        let timed_out = marker();
        let error = client.invoke("fixture", "hang", json!({"marker":timed_out})).await.unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert_eq!(std::fs::read_to_string(&timed_out).unwrap().lines().count(), 1);
        #[cfg(unix)] assert_child_stopped(first_pid).await;
        assert!(client.ping("fixture").await.unwrap());
        let next_pid = pid(&client).await;
        assert_ne!(first_pid, next_pid);
        let disconnected = marker();
        let error = client.invoke("fixture", "exit", json!({"marker":disconnected})).await.unwrap_err();
        assert!(format!("{error:#}").contains("disconnected"));
        assert_eq!(std::fs::read_to_string(&disconnected).unwrap().lines().count(), 1);
        #[cfg(unix)] assert_child_stopped(next_pid).await;
        client.reconnect("fixture").await.unwrap();
        client.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn explicit_reconnect_and_client_drop_stop_previous_child() {
        let client = fixture("normal", Duration::from_secs(3));
        let first_pid = pid(&client).await;
        client.reconnect("fixture").await.unwrap();
        #[cfg(unix)] assert_child_stopped(first_pid).await;
        let next_pid = pid(&client).await;
        assert_ne!(first_pid, next_pid);
        drop(client);
        #[cfg(unix)] assert_child_stopped(next_pid).await;
    }

    #[tokio::test]
    async fn cancellation_drops_active_child_and_allows_clean_reconnection() {
        let client = Arc::new(fixture("normal", Duration::from_secs(3)));
        let first_pid = pid(&client).await;
        let started = marker();
        let task_client = client.clone();
        let task_marker = started.clone();
        let task = tokio::spawn(async move { task_client.invoke("fixture", "hang", json!({"marker":task_marker})).await });
        timeout(Duration::from_secs(3), async {
            while !started.exists() { tokio::time::sleep(Duration::from_millis(5)).await; }
        }).await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        #[cfg(unix)] assert_child_stopped(first_pid).await;
        assert!(client.ping("fixture").await.unwrap());
        client.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn unterminated_oversized_output_is_bounded() {
        let mut client = fixture("oversize", Duration::from_secs(3));
        client.options.max_message_bytes = 1024;
        let error = client.ping("fixture").await.unwrap_err();
        assert!(format!("{error:#}").contains("byte limit"));
    }

    #[test]
    fn unsupported_transport_is_rejected_before_launch() {
        let server = McpServer { name: "remote".into(), transport: McpTransport::StreamableHttp { url:"https://example.test".into() },
            command: None, args: vec![], env: HashMap::new() };
        assert!(StdioMcpClient::new(vec![server], StdioOptions::default()).is_err());
    }
}
