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

// ---------------------------------------------------------------------------
// Process-backed ShellManager
// ---------------------------------------------------------------------------

use std::path::Path;
use std::process::Stdio;
use std::time::Instant;
use tokio::process::Command;

/// A `ShellManager` that spawns real subprocesses via `sh -c` and tracks
/// worktrees with `git worktree`.
pub struct ProcessShellManager {
    shells: tokio::sync::RwLock<HashMap<ShellId, (ShellSpec, Vec<ShellEvent>)>>,
}

impl Default for ProcessShellManager {
    fn default() -> Self {
        Self {
            shells: tokio::sync::RwLock::new(HashMap::new()),
        }
    }
}

impl ProcessShellManager {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl ShellManager for ProcessShellManager {
    async fn spawn_shell(&self, spec: ShellSpec) -> anyhow::Result<ShellId> {
        let id = Uuid::new_v4();
        let event = ShellEvent::Spawned {
            shell_id: id,
            spec: spec.clone(),
            at: Utc::now(),
        };
        let mut guard = self.shells.write().await;
        guard.insert(id, (spec, vec![event]));
        Ok(id)
    }

    async fn exec(&self, shell: ShellId, cmd: ShellCommand) -> anyhow::Result<ShellOutput> {
        let (cwd, env, read_only) = {
            let guard = self.shells.read().await;
            let (spec, _) = guard
                .get(&shell)
                .ok_or_else(|| anyhow::anyhow!("unknown shell {shell}"))?;
            (spec.cwd.clone(), spec.env.clone(), spec.read_only)
        };

        if read_only {
            return Err(anyhow::anyhow!(
                "shell {shell} is read-only; command rejected"
            ));
        }

        let mut command = Command::new("sh");
        command.arg("-c").arg(&cmd.command);
        command.current_dir(&cwd);
        for (k, v) in env {
            command.env(k, v);
        }
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        command.stdin(Stdio::null());

        let start = Instant::now();
        let timeout = std::time::Duration::from_secs(cmd.timeout_secs.unwrap_or(120));

        let child = command.spawn()?;
        let output = tokio::time::timeout(timeout, child.wait_with_output()).await;
        let duration_ms = start.elapsed().as_millis() as u64;

        let output = match output {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                return Err(anyhow::anyhow!("command failed: {e}"));
            }
            Err(_) => {
                return Ok(ShellOutput {
                    exit_code: 124,
                    stdout: String::new(),
                    stderr: format!("command timed out after {timeout:?}"),
                    duration_ms,
                });
            }
        };

        let result = ShellOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            duration_ms,
        };

        // Record in transcript.
        let mut guard = self.shells.write().await;
        if let Some((_, events)) = guard.get_mut(&shell) {
            events.push(ShellEvent::CommandExecuted {
                command: cmd,
                output: result.clone(),
                at: Utc::now(),
            });
        }

        Ok(result)
    }

    async fn transcript(&self, shell: ShellId) -> anyhow::Result<Vec<ShellEvent>> {
        let guard = self.shells.read().await;
        let (_, events) = guard
            .get(&shell)
            .ok_or_else(|| anyhow::anyhow!("unknown shell {shell}"))?;
        Ok(events.clone())
    }

    async fn close(&self, shell: ShellId) -> anyhow::Result<()> {
        let mut guard = self.shells.write().await;
        if let Some((_, events)) = guard.get_mut(&shell) {
            events.push(ShellEvent::Closed { at: Utc::now() });
        }
        Ok(())
    }

    async fn create_worktree(&self, spec: WorktreeSpec) -> anyhow::Result<WorktreeHandle> {
        let branch = spec.branch.as_deref().unwrap_or("HEAD");
        let path = spec.repo_root.join(format!(
            ".chimera/worktrees/{}",
            Uuid::new_v4().as_simple()
        ));
        std::fs::create_dir_all(path.parent().unwrap())?;

        let output = tokio::process::Command::new("git")
            .arg("worktree")
            .arg("add")
            .arg("-b")
            .arg(format!("chimera-{}", Uuid::new_v4().as_simple()))
            .arg(&path)
            .arg(branch)
            .current_dir(&spec.repo_root)
            .output()
            .await?;

        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "git worktree add failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // Spawn a shell bound to the worktree path.
        let shell_spec = ShellSpec {
            cwd: path.clone(),
            env: HashMap::new(),
            read_only: spec.read_only,
            label: spec.label.clone(),
        };
        let shell_id = self.spawn_shell(shell_spec).await?;

        Ok(WorktreeHandle {
            path,
            shell_id,
            read_only: spec.read_only,
        })
    }

    async fn destroy_worktree(&self, path: PathBuf) -> anyhow::Result<()> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let output = tokio::process::Command::new("git")
            .arg("worktree")
            .arg("remove")
            .arg(&path)
            .current_dir(parent)
            .output()
            .await?;
        if !output.status.success() {
            // Non-fatal: the worktree may already be gone.
            tracing::warn!(
                "git worktree remove failed (non-fatal): {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    async fn list_shells(&self) -> anyhow::Result<Vec<ShellId>> {
        let guard = self.shells.read().await;
        Ok(guard.keys().copied().collect())
    }
}

#[cfg(test)]
mod process_tests {
    use super::*;

    #[tokio::test]
    async fn spawn_exec_and_list() {
        let mgr = ProcessShellManager::new();
        let spec = ShellSpec {
            cwd: std::env::current_dir().unwrap(),
            env: HashMap::new(),
            read_only: false,
            label: None,
        };
        let id = mgr.spawn_shell(spec).await.unwrap();
        let out = mgr
            .exec(
                id,
                ShellCommand {
                    command: "echo hello-chimera".to_string(),
                    timeout_secs: Some(10),
                },
            )
            .await
            .unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.contains("hello-chimera"));
        let ids = mgr.list_shells().await.unwrap();
        assert!(ids.contains(&id));
    }

    #[tokio::test]
    async fn read_only_shell_rejects_exec() {
        let mgr = ProcessShellManager::new();
        let spec = ShellSpec {
            cwd: std::env::current_dir().unwrap(),
            env: HashMap::new(),
            read_only: true,
            label: None,
        };
        let id = mgr.spawn_shell(spec).await.unwrap();
        let res = mgr
            .exec(
                id,
                ShellCommand {
                    command: "echo x".to_string(),
                    timeout_secs: Some(5),
                },
            )
            .await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn transcript_records_events() {
        let mgr = ProcessShellManager::new();
        let spec = ShellSpec {
            cwd: std::env::current_dir().unwrap(),
            env: HashMap::new(),
            read_only: false,
            label: Some("t".to_string()),
        };
        let id = mgr.spawn_shell(spec).await.unwrap();
        mgr.exec(
            id,
            ShellCommand {
                command: "true".to_string(),
                timeout_secs: Some(5),
            },
        )
        .await
        .unwrap();
        let events = mgr.transcript(id).await.unwrap();
        // Spawned + CommandExecuted
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], ShellEvent::Spawned { .. }));
        assert!(matches!(events[1], ShellEvent::CommandExecuted { .. }));
    }
}
