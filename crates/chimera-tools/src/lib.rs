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


/// Best-effort constructor for the concrete `DefaultToolRouter`.
///
/// Returns `None` when only the stub router is available.
pub fn try_build_default_tool_router(
    _repo_root: Option<std::path::PathBuf>,
) -> Option<std::sync::Arc<dyn ToolRouter>> {
    default_router::build(_repo_root)
}

#[doc(hidden)]
pub mod default_router {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// Build the concrete default router wired with all builtin tools.
    pub fn build(repo_root: Option<PathBuf>) -> Option<Arc<dyn ToolRouter>> {
        Some(Arc::new(DefaultToolRouter::new(repo_root)))
    }
}

// ---------------------------------------------------------------------------
// DefaultToolRouter — concrete tool execution backend
// ---------------------------------------------------------------------------

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A concrete `ToolRouter` wired with the builtin file, shell, git, glob, and
/// grep tools.
pub struct DefaultToolRouter {
    handlers: HashMap<String, Box<dyn ToolHandler>>,
    repo_root: Option<PathBuf>,
}

/// Internal trait each builtin tool implements.
#[async_trait::async_trait]
pub trait ToolHandler: Send + Sync {
    async fn call(&self, args: serde_json::Value, ctx: &ToolContext) -> anyhow::Result<ToolResult>;
    fn descriptor(&self) -> ToolDescriptor;
}

/// Shared context passed to every tool handler.
pub struct ToolContext {
    pub repo_root: Option<PathBuf>,
}

impl DefaultToolRouter {
    /// Build a router with all builtin tools registered.
    pub fn new(repo_root: Option<PathBuf>) -> Self {
        let mut handlers: HashMap<String, Box<dyn ToolHandler>> = HashMap::new();
        let register = |name: &str, handler: Box<dyn ToolHandler>, map: &mut HashMap<String, Box<dyn ToolHandler>>| {
            map.insert(name.to_string(), handler);
        };
        register("read_file", Box::new(ReadFileTool), &mut handlers);
        register("write_file", Box::new(WriteFileTool), &mut handlers);
        register("edit_file", Box::new(EditFileTool), &mut handlers);
        register("bash", Box::new(BashTool), &mut handlers);
        register("glob", Box::new(GlobTool), &mut handlers);
        register("grep", Box::new(GrepTool), &mut handlers);
        register("git_status", Box::new(GitStatusTool), &mut handlers);

        Self { handlers, repo_root }
    }

    fn ctx(&self) -> ToolContext {
        ToolContext {
            repo_root: self.repo_root.clone(),
        }
    }
}

#[async_trait::async_trait]
impl ToolRouter for DefaultToolRouter {
    async fn call(&self, req: ToolCall) -> anyhow::Result<ToolResult> {
        let handler = self
            .handlers
            .get(&req.tool)
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {}", req.tool))?;
        let ctx = self.ctx();
        handler.call(req.args, &ctx).await
    }

    async fn list(&self) -> anyhow::Result<Vec<ToolDescriptor>> {
        Ok(self
            .handlers
            .values()
            .map(|h| h.descriptor())
            .collect())
    }

    async fn healthcheck(&self, tool: &str) -> anyhow::Result<bool> {
        Ok(self.handlers.contains_key(tool))
    }
}

// Helper: resolve a path argument against the repo root if relative.
fn resolve_path(p: &str, repo_root: &Option<PathBuf>) -> PathBuf {
    let path = PathBuf::from(p);
    if path.is_absolute() {
        path
    } else if let Some(root) = repo_root {
        root.join(&path)
    } else {
        path
    }
}

// Helper: extract a string field from JSON args.
fn get_str<'a>(args: &'a serde_json::Value, field: &str) -> anyhow::Result<&'a str> {
    args.get(field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing or invalid '{field}' argument"))
}

#[allow(dead_code)]
fn get_str_or<'a>(args: &'a serde_json::Value, field: &str, default: &'a str) -> &'a str {
    args.get(field).and_then(|v| v.as_str()).unwrap_or(default)
}

// --- read_file ---
struct ReadFileTool;
#[async_trait::async_trait]
impl ToolHandler for ReadFileTool {
    async fn call(&self, args: serde_json::Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let path_str = get_str(&args, "path")?;
        let path = resolve_path(path_str, &ctx.repo_root);
        let bytes = std::fs::read(&path)?;
        let len = bytes.len();
        let mut content = String::from_utf8_lossy(&bytes).to_string();
        let truncated = if len > 100_000 {
            content.truncate(100_000);
            content.push_str("\n[truncated: file is {len} bytes]");
            true
        } else {
            false
        };
        Ok(ToolResult {
            ok: true,
            output: serde_json::json!({
                "path": path_str,
                "content": content,
                "bytes": len,
                "truncated": truncated,
            }),
            evidence: vec![],
        })
    }
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "read_file".to_string(),
            description: "Read a file as UTF-8 text.".to_string(),
            read_only: true,
            category: ToolCategory::LocalExecution,
        }
    }
}

// --- write_file ---
struct WriteFileTool;
#[async_trait::async_trait]
impl ToolHandler for WriteFileTool {
    async fn call(&self, args: serde_json::Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let path_str = get_str(&args, "path")?;
        let content = get_str(&args, "content")?;
        let path = resolve_path(path_str, &ctx.repo_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        Ok(ToolResult {
            ok: true,
            output: serde_json::json!({
                "path": path_str,
                "bytes": content.len(),
            }),
            evidence: vec![],
        })
    }
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "write_file".to_string(),
            description: "Write content to a file, creating parent dirs.".to_string(),
            read_only: false,
            category: ToolCategory::LocalExecution,
        }
    }
}

// --- edit_file ---
struct EditFileTool;
#[async_trait::async_trait]
impl ToolHandler for EditFileTool {
    async fn call(&self, args: serde_json::Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let path_str = get_str(&args, "path")?;
        let old_text = get_str(&args, "old_text")?;
        let new_text = get_str(&args, "new_text")?;
        let path = resolve_path(path_str, &ctx.repo_root);
        let content = std::fs::read_to_string(&path)?;
        let count = content.matches(old_text).count();
        if count == 0 {
            return Ok(ToolResult {
                ok: false,
                output: serde_json::json!({
                    "path": path_str,
                    "error": format!("old_text not found in {path_str}"),
                }),
                evidence: vec![],
            });
        }
        if count > 1 {
            return Ok(ToolResult {
                ok: false,
                output: serde_json::json!({
                    "path": path_str,
                    "error": format!("old_text matches {count} times in {path_str}; must be unique"),
                }),
                evidence: vec![],
            });
        }
        let new_content = content.replacen(old_text, new_text, 1);
        std::fs::write(&path, &new_content)?;
        Ok(ToolResult {
            ok: true,
            output: serde_json::json!({
                "path": path_str,
                "replaced": 1,
            }),
            evidence: vec![],
        })
    }
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "edit_file".to_string(),
            description: "Replace the first unique occurrence of old_text with new_text.".to_string(),
            read_only: false,
            category: ToolCategory::LocalExecution,
        }
    }
}

// --- bash ---
struct BashTool;
#[async_trait::async_trait]
impl ToolHandler for BashTool {
    async fn call(&self, args: serde_json::Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let command = get_str(&args, "command")?;
        let cwd = args
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .or_else(|| ctx.repo_root.clone())
            .unwrap_or_else(|| PathBuf::from("."));
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(120);

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(command).current_dir(&cwd);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd.stdin(std::process::Stdio::null());

        let start = Instant::now();
        let child = cmd.spawn()?;
        let timeout = Duration::from_secs(timeout_secs);
        let result = tokio::time::timeout(timeout, child.wait_with_output()).await;
        let duration_ms = start.elapsed().as_millis() as u64;

        let (exit_code, mut stdout, mut stderr) = match result {
            Ok(Ok(o)) => (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).to_string(), String::from_utf8_lossy(&o.stderr).to_string()),
            Ok(Err(e)) => {
                return Ok(ToolResult {
                    ok: false,
                    output: serde_json::json!({"exit_code": -1, "stderr": e.to_string()}),
                    evidence: vec![],
                });
            }
            Err(_) => {
                return Ok(ToolResult {
                    ok: false,
                    output: serde_json::json!({"exit_code": 124, "stderr": format!("timed out after {timeout_secs}s")}),
                    evidence: vec![],
                });
            }
        };

        truncate_to(&mut stdout, 50_000);
        truncate_to(&mut stderr, 50_000);

        Ok(ToolResult {
            ok: exit_code == 0,
            output: serde_json::json!({
                "exit_code": exit_code,
                "stdout": stdout,
                "stderr": stderr,
                "duration_ms": duration_ms,
            }),
            evidence: vec![],
        })
    }
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "bash".to_string(),
            description: "Run a shell command and capture output.".to_string(),
            read_only: false,
            category: ToolCategory::LocalExecution,
        }
    }
}

fn truncate_to(s: &mut String, max: usize) {
    if s.len() > max {
        s.truncate(max);
        s.push_str(&format!("\n[truncated: {} bytes]", s.len()));
    }
}

// --- glob ---
struct GlobTool;
#[async_trait::async_trait]
impl ToolHandler for GlobTool {
    async fn call(&self, args: serde_json::Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let pattern = get_str(&args, "pattern")?;
        let base = args
            .get("path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .or_else(|| ctx.repo_root.clone())
            .unwrap_or_else(|| PathBuf::from("."));

        let glob = globset::Glob::new(pattern)
            .map_err(|e| anyhow::anyhow!("invalid glob pattern: {e}"))?
            .compile_matcher();

        let mut matches: Vec<String> = Vec::new();
        let walker = ignore::WalkBuilder::new(&base)
            .hidden(false)
            .git_ignore(true)
            .build();
        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(&base)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .to_string();
            if glob.is_match(&rel) || glob.is_match(entry.path()) {
                matches.push(rel);
                if matches.len() >= 200 {
                    break;
                }
            }
        }
        Ok(ToolResult {
            ok: true,
            output: serde_json::json!({"matches": matches}),
            evidence: vec![],
        })
    }
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "glob".to_string(),
            description: "Find files matching a glob pattern (respects .gitignore).".to_string(),
            read_only: true,
            category: ToolCategory::LocalExecution,
        }
    }
}

// --- grep ---
struct GrepTool;
#[async_trait::async_trait]
impl ToolHandler for GrepTool {
    async fn call(&self, args: serde_json::Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let pattern = get_str(&args, "pattern")?;
        let base = args
            .get("path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .or_else(|| ctx.repo_root.clone())
            .unwrap_or_else(|| PathBuf::from("."));
        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(100) as usize;

        let re = regex::Regex::new(pattern)
            .map_err(|e| anyhow::anyhow!("invalid regex: {e}"))?;

        let mut results: Vec<serde_json::Value> = Vec::new();
        let walker = ignore::WalkBuilder::new(&base)
            .hidden(false)
            .git_ignore(true)
            .build();
        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let path = entry.path();
            let Ok(content) = std::fs::read_to_string(path) else {
                continue;
            };
            let rel = path
                .strip_prefix(&base)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();
            for (lineno, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    results.push(serde_json::json!({
                        "path": rel,
                        "line": lineno + 1,
                        "content": line,
                    }));
                    if results.len() >= max_results {
                        break;
                    }
                }
            }
            if results.len() >= max_results {
                break;
            }
        }
        Ok(ToolResult {
            ok: true,
            output: serde_json::json!({"matches": results, "count": results.len()}),
            evidence: vec![],
        })
    }
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "grep".to_string(),
            description: "Search file contents with a regex (respects .gitignore).".to_string(),
            read_only: true,
            category: ToolCategory::LocalExecution,
        }
    }
}

// --- git_status ---
struct GitStatusTool;
#[async_trait::async_trait]
impl ToolHandler for GitStatusTool {
    async fn call(&self, args: serde_json::Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let cwd = args
            .get("path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .or_else(|| ctx.repo_root.clone())
            .unwrap_or_else(|| PathBuf::from("."));

        let output = tokio::process::Command::new("git")
            .arg("status")
            .arg("--porcelain=v1")
            .current_dir(&cwd)
            .output()
            .await?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let files: Vec<serde_json::Value> = stdout
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| {
                let status = l.get(..2).unwrap_or("").trim().to_string();
                let path = l.get(3..).unwrap_or("").to_string();
                serde_json::json!({"path": path, "status": status})
            })
            .collect();

        Ok(ToolResult {
            ok: output.status.success(),
            output: serde_json::json!({"files": files}),
            evidence: vec![],
        })
    }
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "git_status".to_string(),
            description: "Run `git status --porcelain=v1` in the repo.".to_string(),
            read_only: true,
            category: ToolCategory::VersionControl,
        }
    }
}

#[cfg(test)]
mod default_router_tests {
    use super::*;
    use std::path::Path;

    fn unique_dir() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("chimera-tools-test-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn router_for(dir: &Path) -> DefaultToolRouter {
        DefaultToolRouter::new(Some(dir.to_path_buf()))
    }

    fn call(router: &DefaultToolRouter, tool: &str, args: serde_json::Value) -> ToolResult {
        let req = ToolCall {
            tool: tool.to_string(),
            args,
            world: None,
            requested_by: Uuid::new_v4(),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(router.call(req)).unwrap()
    }

    #[test]
    fn write_then_read_roundtrip() {
        let dir = unique_dir();
        let router = router_for(&dir);
        let w = call(
            &router,
            "write_file",
            serde_json::json!({"path": "a.txt", "content": "hello world"}),
        );
        assert!(w.ok);
        let r = call(&router, "read_file", serde_json::json!({"path": "a.txt"}));
        assert!(r.ok);
        assert_eq!(
            r.output.get("content").and_then(|v| v.as_str()),
            Some("hello world")
        );
    }

    #[test]
    fn edit_file_replaces_unique() {
        let dir = unique_dir();
        let router = router_for(&dir);
        call(
            &router,
            "write_file",
            serde_json::json!({"path": "b.txt", "content": "foo bar foo"}),
        );
        let e = call(
            &router,
            "edit_file",
            serde_json::json!({"path": "b.txt", "old_text": "bar", "new_text": "baz"}),
        );
        assert!(e.ok);
        let r = call(&router, "read_file", serde_json::json!({"path": "b.txt"}));
        assert_eq!(
            r.output.get("content").and_then(|v| v.as_str()),
            Some("foo baz foo")
        );
    }

    #[test]
    fn edit_file_errors_on_non_unique() {
        let dir = unique_dir();
        let router = router_for(&dir);
        call(
            &router,
            "write_file",
            serde_json::json!({"path": "c.txt", "content": "x x"}),
        );
        let e = call(
            &router,
            "edit_file",
            serde_json::json!({"path": "c.txt", "old_text": "x", "new_text": "y"}),
        );
        assert!(!e.ok);
    }

    #[test]
    fn bash_runs_command() {
        let dir = unique_dir();
        let router = router_for(&dir);
        let r = call(
            &router,
            "bash",
            serde_json::json!({"command": "echo bash-test-output"}),
        );
        assert!(r.ok);
        assert!(r.output
            .get("stdout")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("bash-test-output"));
    }

    #[test]
    fn glob_finds_files() {
        let dir = unique_dir();
        std::fs::write(dir.join("alpha.rs"), "").unwrap();
        std::fs::write(dir.join("beta.txt"), "").unwrap();
        let router = router_for(&dir);
        let r = call(&router, "glob", serde_json::json!({"pattern": "*.rs"}));
        assert!(r.ok);
        let matches = r.output.get("matches").unwrap().as_array().unwrap();
        assert!(matches.iter().any(|m| {
            m.as_str().unwrap_or("").ends_with("alpha.rs")
        }));
    }

    #[test]
    fn grep_matches_content() {
        let dir = unique_dir();
        std::fs::write(dir.join("search.txt"), "findme\nother").unwrap();
        let router = router_for(&dir);
        let r = call(&router, "grep", serde_json::json!({"pattern": "findme"}));
        assert!(r.ok);
        let matches = r.output.get("matches").unwrap().as_array().unwrap();
        assert_eq!(matches.len(), 1);
    }

    #[tokio::test]
    async fn list_returns_all_tools() {
        let dir = unique_dir();
        let router = router_for(&dir);
        let descs = router.list().await.unwrap();
        let names: Vec<&str> = descs.iter().map(|d| d.name.as_str()).collect();
        for expected in [
            "read_file",
            "write_file",
            "edit_file",
            "bash",
            "glob",
            "grep",
            "git_status",
        ] {
            assert!(names.contains(&expected), "missing tool: {expected}");
        }
    }

    #[tokio::test]
    async fn healthcheck_recognizes_registered() {
        let dir = unique_dir();
        let router = router_for(&dir);
        assert!(router.healthcheck("bash").await.unwrap());
        assert!(!router.healthcheck("nope").await.unwrap());
    }
}