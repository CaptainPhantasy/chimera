pub mod process;
pub use process::run_scoped_command;

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


/// A `ShellManager` that spawns real subprocesses via `sh -c` and tracks
/// worktrees with `git worktree`.
pub struct ProcessShellManager {
    shells: tokio::sync::RwLock<HashMap<ShellId, (ShellSpec, Vec<ShellEvent>)>>,
    cancellation: tokio::sync::RwLock<HashMap<ShellId, tokio::sync::watch::Sender<bool>>>,
    worktrees: tokio::sync::RwLock<HashMap<PathBuf, ShellId>>,
}

impl Default for ProcessShellManager {
    fn default() -> Self {
        Self {
            shells: tokio::sync::RwLock::new(HashMap::new()),
            cancellation: tokio::sync::RwLock::new(HashMap::new()),
            worktrees: tokio::sync::RwLock::new(HashMap::new()),
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
        let (tx, _) = tokio::sync::watch::channel(false);
        self.cancellation.write().await.insert(id, tx);
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

        let mut cancelled = self.cancellation.read().await.get(&shell)
            .ok_or_else(|| anyhow::anyhow!("unknown shell {shell}"))?.subscribe();
        anyhow::ensure!(!*cancelled.borrow(), "shell {shell} is closed");
        let result = tokio::select! {
            biased;
            _ = cancelled.changed() => ShellOutput {
                exit_code: 130, stdout: String::new(),
                stderr: "shell closed; owned processes stopped".into(), duration_ms: 0,
            },
            result = run_scoped_command(&cwd, &cwd, &cmd.command, &env, cmd.timeout_secs.unwrap_or(120)) => result?,
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
        let signals = self.cancellation.read().await;
        let signal = signals.get(&shell).ok_or_else(|| anyhow::anyhow!("unknown shell {shell}"))?;
        if !*signal.borrow() {
            signal.send_replace(true);
            if let Some((_, events)) = self.shells.write().await.get_mut(&shell) {
                events.push(ShellEvent::Closed { at: Utc::now() });
            }
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
        self.worktrees.write().await.insert(path.clone(), shell_id);

        Ok(WorktreeHandle {
            path,
            shell_id,
            read_only: spec.read_only,
        })
    }

    async fn destroy_worktree(&self, path: PathBuf) -> anyhow::Result<()> {
        let shell = self.worktrees.read().await.get(&path).copied()
            .ok_or_else(|| anyhow::anyhow!("worktree is not owned by this manager: {}", path.display()))?;
        self.close(shell).await?;
        // Preserve the entire checkout. Cleanup never removes uncommitted work.
        let parent = path.parent().ok_or_else(|| anyhow::anyhow!("worktree has no parent"))?;
        let quarantine = parent.join(format!("quarantine-{}", Uuid::new_v4()));
        let output = tokio::process::Command::new("git")
            .arg("worktree").arg("move").arg(&path).arg(&quarantine)
            .current_dir(&path).output().await?;
        anyhow::ensure!(output.status.success(), "worktree quarantine failed: {}", String::from_utf8_lossy(&output.stderr));
        self.worktrees.write().await.remove(&path);
        Ok(())
    }

    async fn list_shells(&self) -> anyhow::Result<Vec<ShellId>> {
        let signals = self.cancellation.read().await;
        Ok(signals.iter().filter(|(_, signal)| !*signal.borrow()).map(|(id, _)| *id).collect())
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
        assert_eq!(out.exit_code, 0, "{}", out.stderr);
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
        let result = mgr.exec(
            id,
            ShellCommand {
                command: "true".to_string(),
                timeout_secs: Some(5),
            },
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 0, "{}", result.stderr);
        let events = mgr.transcript(id).await.unwrap();
        // Spawned + CommandExecuted
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], ShellEvent::Spawned { .. }));
        assert!(matches!(events[1], ShellEvent::CommandExecuted { .. }));
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    fn fixture() -> PathBuf {
        let path = std::env::temp_dir().join(format!("chimera-shell-safety-{}", Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        path.canonicalize().unwrap()
    }
    #[tokio::test]
    async fn closed_shell_rejects_work_and_is_not_active() {
        let mgr = ProcessShellManager::new();
        let id = mgr.spawn_shell(ShellSpec { cwd: fixture(), env: HashMap::new(), read_only: false, label: None }).await.unwrap();
        mgr.close(id).await.unwrap();
        assert!(mgr.exec(id, ShellCommand { command: "true".into(), timeout_secs: Some(1) }).await.is_err());
        assert!(!mgr.list_shells().await.unwrap().contains(&id));
        assert!(mgr.close(Uuid::new_v4()).await.is_err());
        assert!(mgr.destroy_worktree(fixture()).await.is_err());
    }
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn sandbox_denies_outside_files_private_files_environment_and_network() {
        let root = fixture();
        let outside = fixture().join("private");
        std::fs::write(&outside, "PRIVATE-OUTSIDE").unwrap();
        std::fs::write(root.join(".env"), "PRIVATE-ENV").unwrap();
        let allowed = run_scoped_command(&root, &root, "printf allowed", &HashMap::new(), 5).await.unwrap();
        assert_eq!(allowed.exit_code, 0, "{}", allowed.stderr);
        assert_eq!(allowed.stdout, "allowed");
        let command = format!("cat '{}'; cat .env; printf '%s' \"$CHIMERA_FAKE_SECRET\"", outside.display());
        let out = run_scoped_command(&root, &root, &command, &HashMap::new(), 5).await.unwrap();
        assert!(!out.stdout.contains("PRIVATE-"));
        assert!(out.stderr.contains("Operation not permitted"), "{}", out.stderr);
        let mut invalid_env = HashMap::new();
        invalid_env.insert("CHIMERA_FAKE_SECRET".into(), "secret".into());
        assert!(run_scoped_command(&root, &root, "true", &invalid_env, 1).await.is_err());
        let secret_key = format!("CHIMERA_TEST_SECRET_{}", Uuid::new_v4().as_simple());
        std::env::set_var(&secret_key, "FAKE-PRIVATE-MARKER");
        let out = run_scoped_command(&root, &root, &format!("printf '%s' \"${secret_key}\""), &HashMap::new(), 3).await.unwrap();
        std::env::remove_var(&secret_key);
        assert_eq!(out.exit_code, 0, "{}", out.stderr);
        assert!(out.stdout.is_empty());
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let command = format!("curl --max-time 1 http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let out = run_scoped_command(&root, &root, &command, &HashMap::new(), 3).await.unwrap();
        assert_ne!(out.exit_code, 0);
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn timeout_and_dropped_future_kill_child_groups() {
        let root = fixture();
        let out = run_scoped_command(&root, &root, "(sleep 2; printf late > late) & wait", &HashMap::new(), 1).await.unwrap();
        assert_eq!(out.exit_code, 124, "{}", out.stderr);
        tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
        assert!(!root.join("late").exists());
        let task_root = root.clone();
        let task = tokio::spawn(async move { run_scoped_command(&task_root, &task_root, "printf ready > ready; (sleep 2; printf late > cancelled) & wait", &HashMap::new(), 10).await });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(root.join("ready").exists(), "cancellation fixture never started");
        task.abort();
        let _ = task.await;
        tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
        assert!(!root.join("cancelled").exists());
    }
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn output_is_bounded_while_captured() {
        let root = fixture();
        let out = run_scoped_command(&root, &root, "head -c 150000 /dev/zero | tr '\\0' x", &HashMap::new(), 5).await.unwrap();
        assert_eq!(out.exit_code, 0, "{}", out.stderr);
        assert!(out.stdout.len() <= 50_100);
        assert!(out.stdout.contains("truncated"));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod close_and_cleanup_tests {
    use super::*;
    fn fixture() -> PathBuf {
        let path = std::env::temp_dir().join(format!("chimera-owned-{}", Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        path.canonicalize().unwrap()
    }
    #[tokio::test]
    async fn closing_an_active_shell_stops_its_children() {
        let root = fixture();
        let mgr = std::sync::Arc::new(ProcessShellManager::new());
        let id = mgr.spawn_shell(ShellSpec { cwd: root.clone(), env: HashMap::new(), read_only: false, label: None }).await.unwrap();
        let worker = mgr.clone();
        let task = tokio::spawn(async move { worker.exec(id, ShellCommand {
            command: "printf ready > ready; (sleep 2; printf late > late) & wait".into(), timeout_secs: Some(10),
        }).await });
        for _ in 0..30 {
            if root.join("ready").exists() { break; }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(root.join("ready").exists(), "child did not start");
        mgr.close(id).await.unwrap();
        assert_eq!(task.await.unwrap().unwrap().exit_code, 130);
        tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
        assert!(!root.join("late").exists());
    }
    #[tokio::test]
    async fn worktree_cleanup_preserves_dirty_files_and_reports_failure() {
        let repo = fixture();
        for args in [vec!["init", "-q"], vec!["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--allow-empty", "-qm", "fixture"]] {
            let result = tokio::process::Command::new("git").args(args).current_dir(&repo).output().await.unwrap();
            assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        }
        let mgr = ProcessShellManager::new();
        let worktree = mgr.create_worktree(WorktreeSpec { repo_root: repo.clone(), branch: None, read_only: false, label: None }).await.unwrap();
        std::fs::write(worktree.path.join("user-work"), "keep me").unwrap();
        mgr.destroy_worktree(worktree.path.clone()).await.unwrap();
        assert!(!worktree.path.exists());
        let retained = std::fs::read_dir(repo.join(".chimera/worktrees")).unwrap()
            .filter_map(Result::ok).map(|entry| entry.path()).find(|p| p.file_name().unwrap().to_string_lossy().starts_with("quarantine-")).unwrap();
        assert_eq!(std::fs::read_to_string(retained.join("user-work")).unwrap(), "keep me");
        assert!(mgr.exec(worktree.shell_id, ShellCommand { command: "true".into(), timeout_secs: Some(1) }).await.is_err());
        assert!(mgr.destroy_worktree(worktree.path).await.is_err());
    }
}
