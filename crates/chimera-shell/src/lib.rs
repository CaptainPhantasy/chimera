use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// ID types
// ---------------------------------------------------------------------------

/// Unique shell session identifier.
pub type ShellId = Uuid;

// ---------------------------------------------------------------------------
// Shell specification
// ---------------------------------------------------------------------------

/// Specification for spawning a new shell session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellSpec {
    /// Working directory for the shell.
    pub cwd: PathBuf,
    /// Environment variable overrides.
    pub env: HashMap<String, String>,
    /// Whether the shell is read-only (commands that write are blocked by policy).
    pub read_only: bool,
    /// Optional label for display (e.g. "wt-1:RAVEN").
    pub label: Option<String>,
}

// ---------------------------------------------------------------------------
// Shell command / output
// ---------------------------------------------------------------------------

/// A command to execute in a shell session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellCommand {
    /// The command line to execute.
    pub command: String,
    /// Optional timeout in seconds.
    pub timeout_secs: Option<u64>,
}

/// The result of executing a shell command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellOutput {
    /// Exit code (0 = success).
    pub exit_code: i32,
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
}

// ---------------------------------------------------------------------------
// Shell events (transcript)
// ---------------------------------------------------------------------------

/// A recorded event in a shell session's transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShellEvent {
    /// Shell was spawned.
    Spawned {
        shell_id: ShellId,
        spec: ShellSpec,
        at: DateTime<Utc>,
    },
    /// A command was executed.
    CommandExecuted {
        command: ShellCommand,
        output: ShellOutput,
        at: DateTime<Utc>,
    },
    /// Shell was closed.
    Closed {
        at: DateTime<Utc>,
    },
}

// ---------------------------------------------------------------------------
// Worktree specification
// ---------------------------------------------------------------------------

/// Specification for creating an isolated git worktree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeSpec {
    /// Source repo root.
    pub repo_root: PathBuf,
    /// Branch or ref to check out.
    pub branch: Option<String>,
    /// Whether the worktree should be read-only.
    pub read_only: bool,
    /// Optional label.
    pub label: Option<String>,
}

/// A created worktree handle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeHandle {
    /// Unique worktree path.
    pub path: PathBuf,
    /// The shell bound to this worktree.
    pub shell_id: ShellId,
    /// Whether it is read-only.
    pub read_only: bool,
}

// ---------------------------------------------------------------------------
// ShellManager trait
// ---------------------------------------------------------------------------

/// Manages shell sessions and worktrees.
#[async_trait::async_trait]
pub trait ShellManager: Send + Sync {
    /// Spawn a new shell session with the given spec.
    async fn spawn_shell(&self, spec: ShellSpec) -> anyhow::Result<ShellId>;

    /// Execute a command in an existing shell session.
    async fn exec(&self, shell: ShellId, cmd: ShellCommand) -> anyhow::Result<ShellOutput>;

    /// Get the full transcript of a shell session.
    async fn transcript(&self, shell: ShellId) -> anyhow::Result<Vec<ShellEvent>>;

    /// Close a shell session.
    async fn close(&self, shell: ShellId) -> anyhow::Result<()>;

    /// Create an isolated git worktree and return a bound shell.
    async fn create_worktree(&self, spec: WorktreeSpec) -> anyhow::Result<WorktreeHandle>;

    /// Destroy a worktree and its associated shell.
    async fn destroy_worktree(&self, path: PathBuf) -> anyhow::Result<()>;

    /// List all active shell IDs.
    async fn list_shells(&self) -> anyhow::Result<Vec<ShellId>>;
}
