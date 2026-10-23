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
    /// Attach an execution world before policy checks and operator approval.
    /// Routers with a selected world must reject a different requested world.
    async fn bind_call(&self, req: ToolCall) -> anyhow::Result<ToolCall> {
        Ok(req)
    }

    /// Invoke a tool and return the result.
    async fn call(&self, req: ToolCall) -> anyhow::Result<ToolResult>;

    /// Whether this exact call needs a fresh operator decision.
    async fn approval_required(&self, req: &ToolCall) -> anyhow::Result<bool> {
        let descriptor = self
            .list()
            .await?
            .into_iter()
            .find(|tool| tool.name == req.tool)
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {}", req.tool))?;
        Ok(!descriptor.read_only)
    }
    /// Grant one short-lived approval bound to the complete call and caller.
    async fn approve_once(&self, _req: &ToolCall) -> anyhow::Result<()> {
        anyhow::bail!("this router does not support approvals")
    }
    /// Revoke any unused approval for this exact call.
    async fn deny(&self, _req: &ToolCall) -> anyhow::Result<()> {
        Ok(())
    }

    /// List all available tools.
    async fn list(&self) -> anyhow::Result<Vec<ToolDescriptor>>;

    /// Provider-facing schema for dynamic tools. Built-ins use the agent's
    /// known schemas when this returns None.
    async fn input_schema(&self, _tool: &str) -> anyhow::Result<Option<serde_json::Value>> {
        Ok(None)
    }

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
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FILE_LIMIT: usize = 100_000;

/// The effective tool restrictions, enforced again at dispatch.
#[derive(Debug, Clone)]
pub struct ToolPolicy {
    pub read_only: bool,
    pub approval_ttl: Duration,
}
impl ToolPolicy {
    pub fn read_only() -> Self {
        Self {
            read_only: true,
            approval_ttl: Duration::from_secs(120),
        }
    }
    pub fn supervised() -> Self {
        Self {
            read_only: false,
            approval_ttl: Duration::from_secs(120),
        }
    }
}
impl Default for ToolPolicy {
    fn default() -> Self {
        Self::supervised()
    }
}

#[derive(Default)]
struct FileState {
    observed: Mutex<HashMap<(CreatureId, PathBuf), Vec<u8>>>,
    writes: Mutex<()>,
}

/// A concrete `ToolRouter` wired with the builtin file, shell, git, glob, and
/// grep tools.
pub struct DefaultToolRouter {
    handlers: HashMap<String, Box<dyn ToolHandler>>,
    repo_root: Option<PathBuf>,
    policy: ToolPolicy,
    approvals: Mutex<HashMap<String, Instant>>,
    files: Arc<FileState>,
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
    actor: CreatureId,
    files: Arc<FileState>,
}

impl DefaultToolRouter {
    /// Build a router with all builtin tools registered.
    pub fn new(repo_root: Option<PathBuf>) -> Self {
        Self::with_policy(repo_root, ToolPolicy::supervised())
    }

    pub fn with_policy(repo_root: Option<PathBuf>, policy: ToolPolicy) -> Self {
        let mut handlers: HashMap<String, Box<dyn ToolHandler>> = HashMap::new();
        let register = |name: &str,
                        handler: Box<dyn ToolHandler>,
                        map: &mut HashMap<String, Box<dyn ToolHandler>>| {
            map.insert(name.to_string(), handler);
        };
        register("read_file", Box::new(ReadFileTool), &mut handlers);
        register("write_file", Box::new(WriteFileTool), &mut handlers);
        register("edit_file", Box::new(EditFileTool), &mut handlers);
        register("bash", Box::new(BashTool), &mut handlers);
        register("glob", Box::new(GlobTool), &mut handlers);
        register("grep", Box::new(GrepTool), &mut handlers);
        register("git_status", Box::new(GitStatusTool), &mut handlers);

        Self {
            handlers,
            repo_root: repo_root.or_else(|| std::env::current_dir().ok()),
            policy,
            approvals: Mutex::new(HashMap::new()),
            files: Arc::new(FileState::default()),
        }
    }

    fn ctx(&self, actor: CreatureId) -> ToolContext {
        ToolContext {
            repo_root: self.repo_root.clone(),
            actor,
            files: self.files.clone(),
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
        let writes = !handler.descriptor().read_only;
        anyhow::ensure!(
            !writes || !self.policy.read_only,
            "plan mode denies mutating tool: {}",
            req.tool
        );
        if writes {
            let key = serde_json::to_string(&req)?;
            let expiry = self
                .approvals
                .lock()
                .map_err(|_| anyhow::anyhow!("approval state unavailable"))?
                .remove(&key);
            anyhow::ensure!(
                expiry.is_some_and(|expiry| expiry > Instant::now()),
                "approval required for this exact action: {}",
                req.tool
            );
        }
        anyhow::ensure!(
            req.world.is_none(),
            "requested execution world is unavailable; refusing host fallback"
        );
        let ctx = self.ctx(req.requested_by);
        handler.call(req.args, &ctx).await
    }

    async fn approval_required(&self, req: &ToolCall) -> anyhow::Result<bool> {
        let handler = self
            .handlers
            .get(&req.tool)
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {}", req.tool))?;
        let writes = !handler.descriptor().read_only;
        anyhow::ensure!(
            !writes || !self.policy.read_only,
            "plan mode denies mutating tool: {}",
            req.tool
        );
        Ok(writes)
    }

    async fn approve_once(&self, req: &ToolCall) -> anyhow::Result<()> {
        anyhow::ensure!(!self.policy.read_only, "plan mode cannot approve mutation");
        anyhow::ensure!(
            self.handlers.contains_key(&req.tool),
            "unknown tool: {}",
            req.tool
        );
        let expiry = Instant::now()
            .checked_add(self.policy.approval_ttl)
            .ok_or_else(|| anyhow::anyhow!("invalid approval lifetime"))?;
        let mut approvals = self
            .approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("approval state unavailable"))?;
        approvals.retain(|_, until| *until > Instant::now());
        anyhow::ensure!(approvals.len() < 128, "too many pending approvals");
        approvals.insert(serde_json::to_string(req)?, expiry);
        Ok(())
    }

    async fn deny(&self, req: &ToolCall) -> anyhow::Result<()> {
        self.approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("approval state unavailable"))?
            .remove(&serde_json::to_string(req)?);
        Ok(())
    }

    async fn list(&self) -> anyhow::Result<Vec<ToolDescriptor>> {
        Ok(self
            .handlers
            .values()
            .filter(|h| !self.policy.read_only || h.descriptor().read_only)
            .map(|h| h.descriptor())
            .collect())
    }

    async fn healthcheck(&self, tool: &str) -> anyhow::Result<bool> {
        Ok(self.handlers.contains_key(tool))
    }
}

// Every existing component must be a real directory/file inside the selected root.
fn resolve_path(p: &str, repo_root: &Option<PathBuf>) -> anyhow::Result<PathBuf> {
    use std::path::Component;
    let root = repo_root
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no project root configured"))?
        .canonicalize()?;
    let path = PathBuf::from(p);
    anyhow::ensure!(
        !path.components().any(|c| matches!(c, Component::ParentDir)),
        "parent path traversal is not allowed"
    );
    let candidate = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    let relative = candidate
        .strip_prefix(&root)
        .map_err(|_| anyhow::anyhow!("path is outside the allowed project"))?;
    let mut checked = root;
    for part in relative.components() {
        let text = part.as_os_str().to_string_lossy();
        anyhow::ensure!(
            !is_private(&text),
            "protected path is not available to tools: {text}"
        );
        checked.push(part);
        match std::fs::symlink_metadata(&checked) {
            Ok(meta) => anyhow::ensure!(
                !meta.file_type().is_symlink(),
                "symbolic links are not allowed: {}",
                checked.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(checked)
}

fn is_private(name: &str) -> bool {
    matches!(
        name,
        ".git" | ".chimera" | ".ssh" | ".aws" | ".codex" | ".supercache" | ".agents"
    ) || name == ".env"
        || (name.starts_with(".env.") && name != ".env.example")
}

fn read_bounded(path: &std::path::Path) -> anyhow::Result<(Vec<u8>, bool)> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(target_os = "macos")]
        options.custom_flags(0x100); // O_NOFOLLOW
        #[cfg(not(target_os = "macos"))]
        options.custom_flags(0x20000);
    }
    let file = options.open(path)?;
    anyhow::ensure!(file.metadata()?.is_file(), "path is not a regular file");
    let mut bytes = Vec::new();
    file.take((FILE_LIMIT + 1) as u64).read_to_end(&mut bytes)?;
    let truncated = bytes.len() > FILE_LIMIT;
    bytes.truncate(FILE_LIMIT);
    Ok((bytes, truncated))
}

fn save_file(
    path: &std::path::Path,
    content: &str,
    ctx: &ToolContext,
) -> anyhow::Result<Option<PathBuf>> {
    anyhow::ensure!(
        content.len() <= FILE_LIMIT,
        "write exceeds the {FILE_LIMIT} byte limit"
    );
    let _guard = ctx
        .files
        .writes
        .lock()
        .map_err(|_| anyhow::anyhow!("file state unavailable"))?;
    let root = ctx
        .repo_root
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no project root"))?
        .canonicalize()?;
    let path = resolve_path(
        path.to_str()
            .ok_or_else(|| anyhow::anyhow!("invalid path"))?,
        &Some(root.clone()),
    )?;
    let existing = match std::fs::symlink_metadata(&path) {
        Ok(_) => {
            let (bytes, truncated) = read_bounded(&path)?;
            anyhow::ensure!(!truncated, "file exceeds the editable size limit");
            let observations = ctx
                .files
                .observed
                .lock()
                .map_err(|_| anyhow::anyhow!("file state unavailable"))?;
            let expected = observations
                .get(&(ctx.actor, path.clone()))
                .ok_or_else(|| anyhow::anyhow!("read the existing file before changing it"))?;
            anyhow::ensure!(
                &bytes == expected,
                "file changed since it was read; read it again before editing"
            );
            Some(bytes)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let recovery = root.join(".chimera").join("recovery");
    for directory in [root.join(".chimera"), recovery.clone()] {
        match std::fs::symlink_metadata(&directory) {
            Ok(meta) => anyhow::ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "recovery directory is unsafe"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&directory)?;
            }
            Err(e) => return Err(e.into()),
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    let backup = if existing.is_some() {
        let backup = recovery.join(format!("{}-original", Uuid::new_v4()));
        std::fs::hard_link(&path, &backup)?;
        Some(backup)
    } else {
        None
    };
    let temporary = recovery.join(format!("{}-new", Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    if let Some(ref original) = existing {
        anyhow::ensure!(
            read_bounded(&path)?.0 == *original,
            "file changed during write; both versions retained in {}",
            recovery.display()
        );
        std::fs::set_permissions(&temporary, std::fs::metadata(&path)?.permissions())?;
        std::fs::rename(&temporary, &path)?;
        if let Some(ref backup) = backup {
            anyhow::ensure!(
                read_bounded(backup)?.0 == *original,
                "concurrent edit detected; original and new versions retained in {}",
                recovery.display()
            );
        }
    } else {
        // hard_link fails if another writer created the destination. No replacement.
        std::fs::hard_link(&temporary, &path)?;
    }
    std::fs::File::open(path.parent().unwrap())?.sync_all()?;
    // The staging copy served its purpose once the destination is durable;
    // drop the second link so recovery storage does not grow without bound.
    if existing.is_none() {
        let _ = std::fs::remove_file(&temporary);
    }
    let mut observed = ctx
        .files
        .observed
        .lock()
        .map_err(|_| anyhow::anyhow!("file state unavailable"))?;
    if observed.len() >= 128 {
        observed.clear();
    }
    observed.insert((ctx.actor, path), content.as_bytes().to_vec());
    Ok(backup)
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
        let path = resolve_path(path_str, &ctx.repo_root)?;
        let (bytes, truncated) = read_bounded(&path)?;
        let len = bytes.len();
        if !truncated {
            let mut observed = ctx
                .files
                .observed
                .lock()
                .map_err(|_| anyhow::anyhow!("file state unavailable"))?;
            // Evicted files must be read again before a later write.
            if observed.len() >= 128 {
                observed.clear();
            }
            observed.insert((ctx.actor, path.clone()), bytes.clone());
        }
        let mut content = String::from_utf8_lossy(&bytes).to_string();
        if truncated {
            content.push_str(
                "\n[truncated: read limit reached; file is not editable until read in full]",
            );
        }
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
        let path = resolve_path(path_str, &ctx.repo_root)?;
        let backup = save_file(&path, content, ctx)?;
        Ok(ToolResult {
            ok: true,
            output: serde_json::json!({
                "path": path_str,
                "bytes": content.len(),
                "recovery_copy": backup,
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
        let path = resolve_path(path_str, &ctx.repo_root)?;
        let (bytes, truncated) = read_bounded(&path)?;
        anyhow::ensure!(!truncated, "file exceeds the editable size limit");
        let content = String::from_utf8(bytes)?;
        anyhow::ensure!(!old_text.is_empty(), "old_text cannot be empty");
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
        let backup = save_file(&path, &new_content, ctx)?;
        Ok(ToolResult {
            ok: true,
            output: serde_json::json!({
                "path": path_str,
                "replaced": 1,
                "recovery_copy": backup,
            }),
            evidence: vec![],
        })
    }
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "edit_file".to_string(),
            description: "Replace the first unique occurrence of old_text with new_text."
                .to_string(),
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
        let cwd = resolve_path(
            args.get("cwd").and_then(|v| v.as_str()).unwrap_or("."),
            &ctx.repo_root,
        )?;
        let root = ctx
            .repo_root
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no project root"))?;
        let output = chimera_shell::run_scoped_command(
            root,
            &cwd,
            command,
            &HashMap::new(),
            args.get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(120),
        )
        .await?;
        Ok(ToolResult {
            ok: output.exit_code == 0,
            output: serde_json::json!({"exit_code": output.exit_code, "stdout": output.stdout,
                "stderr": output.stderr, "duration_ms": output.duration_ms}),
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

// --- glob ---
struct GlobTool;
#[async_trait::async_trait]
impl ToolHandler for GlobTool {
    async fn call(&self, args: serde_json::Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let pattern = get_str(&args, "pattern")?;
        let base = resolve_path(
            args.get("path").and_then(|v| v.as_str()).unwrap_or("."),
            &ctx.repo_root,
        )?;

        let glob = globset::Glob::new(pattern)
            .map_err(|e| anyhow::anyhow!("invalid glob pattern: {e}"))?
            .compile_matcher();

        let mut matches: Vec<String> = Vec::new();
        let walker = ignore::WalkBuilder::new(&base)
            .hidden(false)
            .git_ignore(true)
            .follow_links(false)
            .filter_entry(|entry| !is_private(&entry.file_name().to_string_lossy()))
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
        let base = resolve_path(
            args.get("path").and_then(|v| v.as_str()).unwrap_or("."),
            &ctx.repo_root,
        )?;
        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(100)
            .clamp(1, 200) as usize;

        let re = regex::Regex::new(pattern).map_err(|e| anyhow::anyhow!("invalid regex: {e}"))?;

        let mut results: Vec<serde_json::Value> = Vec::new();
        let walker = ignore::WalkBuilder::new(&base)
            .hidden(false)
            .git_ignore(true)
            .follow_links(false)
            .filter_entry(|entry| !is_private(&entry.file_name().to_string_lossy()))
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
            let Ok((bytes, _)) = read_bounded(path) else {
                continue;
            };
            let content = String::from_utf8_lossy(&bytes);
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
        let cwd = resolve_path(
            args.get("path").and_then(|v| v.as_str()).unwrap_or("."),
            &ctx.repo_root,
        )?;
        let root = ctx
            .repo_root
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no project root"))?;
        let output = chimera_shell::run_scoped_command(
            root,
            &cwd,
            "git --no-optional-locks status --porcelain=v1",
            &HashMap::new(),
            30,
        )
        .await?;
        let stdout = &output.stdout;
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
            ok: output.exit_code == 0,
            output: serde_json::json!({"files": files, "stderr": output.stderr, "exit_code": output.exit_code}),
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
            requested_by: Uuid::nil(),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            if router.approval_required(&req).await.unwrap() {
                router.approve_once(&req).await.unwrap();
            }
            router.call(req).await.unwrap()
        })
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
        assert!(r
            .output
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
        assert!(matches
            .iter()
            .any(|m| { m.as_str().unwrap_or("").ends_with("alpha.rs") }));
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
#[cfg(test)]
mod safety_tests {
    use super::*;

    fn fixture() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("chimera-safety-{}", Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        dir.canonicalize().unwrap()
    }
    fn request(tool: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            tool: tool.into(),
            args,
            world: None,
            requested_by: Uuid::nil(),
        }
    }
    async fn approved(router: &DefaultToolRouter, req: ToolCall) -> anyhow::Result<ToolResult> {
        router.approve_once(&req).await?;
        router.call(req).await
    }

    #[tokio::test]
    async fn plan_denies_mutation_even_when_called_directly() {
        let dir = fixture();
        let router = DefaultToolRouter::with_policy(Some(dir.clone()), ToolPolicy::read_only());
        for tool in ["write_file", "edit_file", "bash"] {
            let req = request(
                tool,
                serde_json::json!({"path":"x", "content":"x", "command":"true"}),
            );
            assert!(router.approve_once(&req).await.is_err());
            assert!(router.call(req).await.is_err());
        }
        assert!(!dir.join("x").exists());
        assert!(router.list().await.unwrap().iter().all(|d| d.read_only));
    }

    #[tokio::test]
    async fn approvals_are_exact_one_use_expiring_and_revocable() {
        let dir = fixture();
        let router = DefaultToolRouter::new(Some(dir.clone()));
        let req = request(
            "write_file",
            serde_json::json!({"path":"ok", "content":"yes"}),
        );
        assert!(router.call(req.clone()).await.is_err());
        router.approve_once(&req).await.unwrap();
        let mut changed = req.clone();
        changed.args["path"] = "other".into();
        assert!(router.call(changed).await.is_err());
        let mut other_actor = req.clone();
        other_actor.requested_by = Uuid::new_v4();
        assert!(router.call(other_actor).await.is_err());
        router.call(req.clone()).await.unwrap();
        assert!(router.call(req.clone()).await.is_err());
        router.approve_once(&req).await.unwrap();
        router.deny(&req).await.unwrap();
        assert!(router.call(req.clone()).await.is_err());
        let expired = DefaultToolRouter::with_policy(
            Some(dir),
            ToolPolicy {
                read_only: false,
                approval_ttl: Duration::ZERO,
            },
        );
        expired.approve_once(&req).await.unwrap();
        assert!(expired.call(req).await.is_err());
    }

    #[tokio::test]
    async fn rejects_outside_parent_private_and_symbolic_paths() {
        let dir = fixture();
        let outside = fixture();
        std::fs::write(outside.join("private"), "SECRET-MARKER").unwrap();
        std::fs::write(dir.join(".env"), "SECRET-MARKER").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, dir.join("link")).unwrap();
        let router = DefaultToolRouter::new(Some(dir.clone()));
        for path in [
            outside.join("private").to_string_lossy().into_owned(),
            "../private".into(),
            ".env".into(),
            "link/private".into(),
        ] {
            assert!(router
                .call(request("read_file", serde_json::json!({"path":path})))
                .await
                .is_err());
            assert!(approved(
                &router,
                request(
                    "write_file",
                    serde_json::json!({"path":path,"content":"bad"})
                )
            )
            .await
            .is_err());
        }
        let matches = router
            .call(request(
                "grep",
                serde_json::json!({"pattern":"SECRET-MARKER"}),
            ))
            .await
            .unwrap();
        assert_eq!(matches.output["count"], 0);
        assert_eq!(
            std::fs::read_to_string(outside.join("private")).unwrap(),
            "SECRET-MARKER"
        );
    }

    #[tokio::test]
    async fn existing_write_requires_read_and_detects_conflict_keeps_original() {
        let dir = fixture();
        let path = dir.join("file");
        std::fs::write(&path, "first").unwrap();
        let router = DefaultToolRouter::new(Some(dir));
        let write = request(
            "write_file",
            serde_json::json!({"path":"file","content":"agent"}),
        );
        assert!(approved(&router, write.clone()).await.is_err());
        router
            .call(request("read_file", serde_json::json!({"path":"file"})))
            .await
            .unwrap();
        std::fs::write(&path, "user edit").unwrap();
        assert!(approved(&router, write.clone()).await.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "user edit");
        router
            .call(request("read_file", serde_json::json!({"path":"file"})))
            .await
            .unwrap();
        let result = approved(&router, write).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(result.output["recovery_copy"].as_str().unwrap()).unwrap(),
            "user edit"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "agent");
    }

    #[tokio::test]
    async fn bounded_unicode_reads_and_search_do_not_panic_or_expose_private_data() {
        let dir = fixture();
        let content = format!("{}é{}", "a".repeat(FILE_LIMIT - 1), "z".repeat(FILE_LIMIT));
        std::fs::write(dir.join("large"), content).unwrap();
        let router = DefaultToolRouter::new(Some(dir));
        let result = router
            .call(request("read_file", serde_json::json!({"path":"large"})))
            .await
            .unwrap();
        assert_eq!(result.output["truncated"], true);
        assert!(result.output["content"].as_str().unwrap().len() <= FILE_LIMIT + 120);
        assert!(approved(
            &router,
            request(
                "write_file",
                serde_json::json!({"path":"large","content":"bad"})
            )
        )
        .await
        .is_err());
    }
}
