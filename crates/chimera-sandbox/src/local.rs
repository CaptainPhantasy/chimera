//! Persisted local execution worlds. Unsupported isolation is rejected, never simulated.
use super::*;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

pub struct LocalWorldManager {
    project: PathBuf,
    storage: PathBuf,
}

struct FileLock(std::fs::File);
/// Holds the selected world open until a bounded operation finishes or is dropped.
/// Pause and close wait for this ownership guard before moving its files.
pub struct WorldOperation {
    pub state: WorldState,
    _active: FileLock,
}
impl FileLock {
    fn try_take(path: &Path) -> anyhow::Result<Option<Self>> {
        let mut options = std::fs::OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
            #[cfg(target_os = "macos")] options.custom_flags(0x100);
            #[cfg(not(target_os = "macos"))] options.custom_flags(0x20000);
        }
        let file = options.open(path)?;
        #[cfg(unix)] {
            use std::os::fd::AsRawFd;
            unsafe extern "C" { fn flock(fd: i32, operation: i32) -> i32; }
            if unsafe { flock(file.as_raw_fd(), 2 | 4) } != 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::WouldBlock { return Ok(None); }
                return Err(error.into());
            }
            Ok(Some(Self(file)))
        }
        #[cfg(not(unix))]
        anyhow::bail!("world locking is unavailable on this platform")
    }
}
impl Drop for FileLock {
    fn drop(&mut self) {
        #[cfg(unix)] {
            use std::os::fd::AsRawFd;
            unsafe extern "C" { fn flock(fd: i32, operation: i32) -> i32; }
            unsafe { flock(self.0.as_raw_fd(), 8); }
        }
    }
}

fn private_directory(path: &Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => anyhow::ensure!(metadata.is_dir() && !metadata.file_type().is_symlink(), "unsafe world storage directory: {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(path)?,
        Err(error) => return Err(error.into()),
    }
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

impl LocalWorldManager {
    pub fn new(project_root: PathBuf) -> anyhow::Result<Self> {
        let project = project_root.canonicalize()?;
        anyhow::ensure!(project.is_dir(), "project root is not a directory");
        private_directory(&project.join(".chimera"))?;
        let storage = project.join(".chimera/worlds");
        private_directory(&storage)?;
        for name in ["workspaces", "quarantine", "history"] { private_directory(&storage.join(name))?; }
        Ok(Self { project, storage })
    }

    fn state_path(&self, id: WorldId) -> PathBuf { self.storage.join(format!("{id}.json")) }
    fn active_path(&self, id: WorldId) -> PathBuf { self.storage.join(format!("{id}.active")) }

    fn state_lock(&self, id: WorldId) -> anyhow::Result<FileLock> {
        FileLock::try_take(&self.storage.join(format!("{id}.lock")))?
            .ok_or_else(|| anyhow::anyhow!("world {id} is being updated; retry after the current operation"))
    }

    fn load(&self, id: WorldId) -> anyhow::Result<WorldState> {
        let path = self.state_path(id);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| anyhow::anyhow!("cannot load world {id}: {error}"))?;
        anyhow::ensure!(metadata.is_file() && !metadata.file_type().is_symlink(), "unsafe world state file");
        anyhow::ensure!(metadata.len() <= 1_000_000, "world state exceeds size limit");
        let mut bytes = Vec::new();
        std::fs::File::open(&path)?.take(1_000_001).read_to_end(&mut bytes)?;
        anyhow::ensure!(bytes.len() <= 1_000_000, "world state exceeds size limit");
        let state: WorldState = serde_json::from_slice(&bytes)
            .map_err(|error| anyhow::anyhow!("world {id} has damaged state: {error}"))?;
        anyhow::ensure!(state.id == id, "world identifier does not match its saved state");
        let root = state.root_path.as_ref().ok_or_else(|| anyhow::anyhow!("world has no execution path"))?;
        match state.spec.kind {
            WorldKind::LocalShell => anyhow::ensure!(root == &self.project, "local world root does not match its project"),
            WorldKind::WorktreeReadOnly | WorldKind::WorktreeReadWrite => {
                let expected = self.storage.join("workspaces").join(id.to_string());
                let preserved = self.storage.join("quarantine").join(id.to_string());
                anyhow::ensure!(root == &expected || (state.status == WorldStatus::Destroyed && root == &preserved), "world path does not match an owned workspace");
            },
            _ => anyhow::bail!("saved world uses an unsupported execution backend"),
        }
        Ok(state)
    }

    fn save(&self, state: &WorldState) -> anyhow::Result<()> {
        let path = self.state_path(state.id);
        if path.exists() {
            let metadata = std::fs::symlink_metadata(&path)?;
            anyhow::ensure!(metadata.is_file() && !metadata.file_type().is_symlink(), "unsafe world state file");
            std::fs::hard_link(&path, self.storage.join("history").join(format!("{}-{}.json", state.id, Uuid::new_v4())))?;
        }
        let pending = self.storage.join("history").join(format!("{}-pending.json", Uuid::new_v4()));
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        let mut file = options.open(&pending)?;
        file.write_all(&serde_json::to_vec_pretty(state)?)?;
        file.sync_all()?;
        std::fs::rename(&pending, path)?;
        std::fs::File::open(&self.storage)?.sync_all()?;
        Ok(())
    }

    fn validate_spec(&self, spec: &WorldSpec) -> anyhow::Result<()> {
        anyhow::ensure!(matches!(spec.kind, WorldKind::LocalShell | WorldKind::WorktreeReadOnly | WorldKind::WorktreeReadWrite), "execution backend {:?} is unavailable; no world was created", spec.kind);
        anyhow::ensure!(cfg!(target_os = "macos"), "native isolated worlds are supported only on macOS");
        anyhow::ensure!(!spec.network, "network access is not supported by the local isolation policy");
        anyhow::ensure!(spec.image.is_none(), "images require an unavailable container or virtual-machine backend");
        anyhow::ensure!(spec.limits.cpu_cores.is_none() && spec.limits.memory_mb.is_none() && spec.limits.disk_mb.is_none(), "CPU, memory and disk limits require an unavailable resource-isolation backend");
        anyhow::ensure!(spec.limits.timeout_secs.is_none_or(|value| (1..=600).contains(&value)), "world timeout must be between 1 and 600 seconds");
        anyhow::ensure!(spec.kind != WorldKind::WorktreeReadOnly || !spec.writable, "read-only worktree cannot be writable");
        if let Some(root) = &spec.root { anyhow::ensure!(root.canonicalize()? == self.project, "world source root must match the selected project"); }
        for key in spec.env.keys() {
            anyhow::ensure!(matches!(key.as_str(), "LANG" | "LC_ALL" | "TERM" | "NO_COLOR"), "environment override is not allowed: {key}");
        }
        Ok(())
    }

    /// Claim one operation after checking the saved world and its lifetime.
    pub fn claim_operation(&self, id: WorldId) -> anyhow::Result<WorldOperation> {
        let active = FileLock::try_take(&self.active_path(id))?
            .ok_or_else(|| anyhow::anyhow!("world already has a running operation"))?;
        let state = self.load(id)?;
        anyhow::ensure!(state.status == WorldStatus::Running, "world is not running: {:?}", state.status);
        self.validate_spec(&state.spec)?;
        let root = state.root_path.as_ref().unwrap();
        anyhow::ensure!(!std::fs::symlink_metadata(root)?.file_type().is_symlink(), "world root was replaced by a symbolic link");
        let lifetime_remaining = state.spec.limits.timeout_secs.map(|limit| {
            let elapsed = (Utc::now() - state.created_at).num_seconds().max(0) as u64;
            limit.saturating_sub(elapsed)
        });
        anyhow::ensure!(lifetime_remaining != Some(0), "world lifetime has expired");
        Ok(WorldOperation { state, _active: active })
    }

    /// Run one command in an allocated world; saved pause/close decisions also
    /// stop work submitted by a separate LocalWorldManager process.
    pub async fn execute(&self, id: WorldId, command: &str, timeout_secs: u64) -> anyhow::Result<chimera_shell::ShellOutput> {
        self.execute_in(id, Path::new("."), command, timeout_secs).await
    }

    /// The command directory is resolved below the world's actual root; it can
    /// never select the source checkout or an unrelated host directory.
    pub async fn execute_in(&self, id: WorldId, cwd: &Path, command: &str, timeout_secs: u64) -> anyhow::Result<chimera_shell::ShellOutput> {
        anyhow::ensure!((1..=600).contains(&timeout_secs), "command timeout must be between 1 and 600 seconds");
        let active = self.claim_operation(id)?;
        let state = &active.state;
        let root = state.root_path.as_ref().unwrap();
        let cwd = if cwd.is_absolute() { cwd.to_owned() } else { root.join(cwd) }.canonicalize()?;
        anyhow::ensure!(cwd.is_dir() && cwd.starts_with(root), "command directory is outside the selected world");
        let lifetime_remaining = state.spec.limits.timeout_secs.map(|limit| {
            let elapsed = (Utc::now() - state.created_at).num_seconds().max(0) as u64;
            limit.saturating_sub(elapsed)
        });
        let timeout = timeout_secs.min(lifetime_remaining.unwrap_or(600));
        let watcher = async {
            loop {
                tokio::time::sleep(Duration::from_millis(30)).await;
                if self.load(id)?.status != WorldStatus::Running { return Ok::<(), anyhow::Error>(()); }
            }
        };
        let result = tokio::select! {
            result = chimera_shell::process::run_scoped_command_with_access(root, &cwd, command, &state.spec.env, timeout, state.spec.writable) => result,
            result = watcher => {
                result?;
                Ok(chimera_shell::ShellOutput { exit_code: 130, stdout: String::new(), stderr: "world paused or closed; owned processes stopped".into(), duration_ms: 0 })
            }
        };
        drop(active);
        result
    }

    async fn wait_idle(&self, id: WorldId) -> anyhow::Result<FileLock> {
        for _ in 0..100 {
            if let Some(lock) = FileLock::try_take(&self.active_path(id))? { return Ok(lock); }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        anyhow::bail!("world {id} did not acknowledge stop; files were preserved")
    }
}

#[async_trait::async_trait]
impl WorldManager for LocalWorldManager {
    async fn allocate(&self, mut spec: WorldSpec) -> anyhow::Result<WorldId> {
        self.validate_spec(&spec)?;
        spec.root = Some(self.project.clone());
        let id = Uuid::new_v4();
        let _lock = self.state_lock(id)?;
        let root = if spec.kind == WorldKind::LocalShell { self.project.clone() }
            else { self.storage.join("workspaces").join(id.to_string()) };
        let mut state = WorldState { id, spec, status: WorldStatus::Provisioning, created_at: Utc::now(), root_path: Some(root.clone()) };
        self.save(&state)?;
        if state.spec.kind != WorldKind::LocalShell {
            let output = tokio::process::Command::new("git").args(["worktree", "add", "--detach"])
                .arg(&root).arg("HEAD").current_dir(&self.project).output().await?;
            if !output.status.success() {
                state.status = WorldStatus::Failed;
                self.save(&state)?;
                anyhow::bail!("worktree allocation failed: {}", String::from_utf8_lossy(&output.stderr));
            }
        }
        state.status = WorldStatus::Running;
        self.save(&state)?;
        Ok(id)
    }

    async fn inspect(&self, id: WorldId) -> anyhow::Result<WorldState> { self.load(id) }

    async fn pause(&self, id: WorldId) -> anyhow::Result<()> {
        {
            let _lock = self.state_lock(id)?;
            let mut state = self.load(id)?;
            anyhow::ensure!(state.status == WorldStatus::Running, "only a running world can be paused");
            state.status = WorldStatus::Paused;
            self.save(&state)?;
        }
        let _idle = self.wait_idle(id).await?;
        Ok(())
    }

    async fn resume(&self, id: WorldId) -> anyhow::Result<()> {
        let _lock = self.state_lock(id)?;
        let mut state = self.load(id)?;
        anyhow::ensure!(state.status == WorldStatus::Paused, "only a paused world can be resumed");
        self.validate_spec(&state.spec)?;
        anyhow::ensure!(state.root_path.as_ref().unwrap().is_dir(), "world execution directory is missing");
        state.status = WorldStatus::Running;
        self.save(&state)
    }

    async fn destroy(&self, id: WorldId) -> anyhow::Result<()> {
        {
            let _lock = self.state_lock(id)?;
            let mut state = self.load(id)?;
            anyhow::ensure!(state.status != WorldStatus::Destroyed, "world is already closed");
            state.status = WorldStatus::Destroying;
            self.save(&state)?;
        }
        let _idle = self.wait_idle(id).await?;
        let _lock = self.state_lock(id)?;
        let mut state = self.load(id)?;
        if state.spec.kind != WorldKind::LocalShell {
            let root = state.root_path.as_ref().unwrap();
            let quarantine = self.storage.join("quarantine").join(id.to_string());
            if quarantine.exists() {
                state.status = WorldStatus::Failed;
                self.save(&state)?;
                anyhow::bail!("worktree cleanup failed; quarantine destination already exists and files were preserved");
            }
            let output = tokio::process::Command::new("git").args(["worktree", "move"])
                .arg(root).arg(&quarantine).current_dir(&self.project).output().await?;
            if !output.status.success() {
                state.status = WorldStatus::Failed;
                self.save(&state)?;
                anyhow::bail!("worktree cleanup failed; files were preserved: {}", String::from_utf8_lossy(&output.stderr));
            }
            state.root_path = Some(quarantine);
        }
        state.status = WorldStatus::Destroyed;
        self.save(&state)
    }

    async fn list(&self) -> anyhow::Result<Vec<WorldId>> {
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(&self.storage)? {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                let id = path.file_stem().and_then(|s| s.to_str()).and_then(|s| Uuid::parse_str(s).ok())
                    .ok_or_else(|| anyhow::anyhow!("invalid world record name: {}", path.display()))?;
                if self.load(id)?.status != WorldStatus::Destroyed { ids.push(id); }
            }
        }
        ids.sort();
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> PathBuf {
        let path = std::env::temp_dir().join(format!("chimera-world-{}", Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        path.canonicalize().unwrap()
    }
    fn spec(kind: WorldKind, writable: bool) -> WorldSpec {
        WorldSpec { kind, writable, image: None, root: None, env: HashMap::new(), network: false, label: Some("test".into()), limits: ResourceLimits::default() }
    }
    async fn git(repo: &Path, args: &[&str]) {
        let out = tokio::process::Command::new("git").args(args).current_dir(repo).output().await.unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    async fn repository() -> PathBuf {
        let repo = fixture();
        git(&repo, &["init", "-q"]).await;
        std::fs::write(repo.join("source.txt"), "original").unwrap();
        git(&repo, &["add", "source.txt"]).await;
        git(&repo, &["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "fixture"]).await;
        repo
    }

    #[tokio::test]
    async fn unsupported_backends_and_unenforced_limits_never_allocate() {
        let manager = LocalWorldManager::new(fixture()).unwrap();
        for kind in [WorldKind::Container, WorldKind::MicroVm, WorldKind::BrowserSandbox, WorldKind::MobileEmulator, WorldKind::RemoteRunner] {
            assert!(manager.allocate(spec(kind, false)).await.is_err());
        }
        let mut network = spec(WorldKind::LocalShell, false);
        network.network = true;
        assert!(manager.allocate(network).await.is_err());
        let mut resources = spec(WorldKind::LocalShell, false);
        resources.limits.memory_mb = Some(100);
        assert!(manager.allocate(resources).await.is_err());
        assert!(manager.list().await.unwrap().is_empty());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn worktree_is_real_persists_and_preserves_dirty_files_on_close() {
        let repo = repository().await;
        let manager = LocalWorldManager::new(repo.clone()).unwrap();
        let id = manager.allocate(spec(WorldKind::WorktreeReadWrite, true)).await.unwrap();
        let world = manager.inspect(id).await.unwrap();
        let path = world.root_path.unwrap();
        assert_ne!(path, repo);
        assert_eq!(std::fs::read_to_string(path.join("source.txt")).unwrap(), "original");
        let out = manager.execute(id, "printf edited > source.txt; printf new > untracked.txt", 5).await.unwrap();
        assert_eq!(out.exit_code, 0, "{}", out.stderr);
        assert_eq!(std::fs::read_to_string(repo.join("source.txt")).unwrap(), "original");
        drop(manager);
        let reopened = LocalWorldManager::new(repo).unwrap();
        assert!(reopened.list().await.unwrap().contains(&id));
        reopened.destroy(id).await.unwrap();
        assert!(!path.exists());
        let closed = reopened.inspect(id).await.unwrap();
        assert_eq!(closed.status, WorldStatus::Destroyed);
        let preserved = closed.root_path.unwrap();
        assert_eq!(std::fs::read_to_string(preserved.join("source.txt")).unwrap(), "edited");
        assert_eq!(std::fs::read_to_string(preserved.join("untracked.txt")).unwrap(), "new");
        assert!(reopened.execute(id, "true", 1).await.is_err());
        assert!(!reopened.list().await.unwrap().contains(&id));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn readonly_world_reads_but_cannot_write() {
        let repo = repository().await;
        let manager = LocalWorldManager::new(repo).unwrap();
        let id = manager.allocate(spec(WorldKind::WorktreeReadOnly, false)).await.unwrap();
        let out = manager.execute(id, "cat source.txt", 5).await.unwrap();
        assert_eq!(out.exit_code, 0, "{}", out.stderr);
        assert_eq!(out.stdout, "original");
        let out = manager.execute(id, "printf bad > source.txt", 5).await.unwrap();
        assert_ne!(out.exit_code, 0);
        assert_eq!(std::fs::read_to_string(manager.inspect(id).await.unwrap().root_path.unwrap().join("source.txt")).unwrap(), "original");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn separate_manager_pause_stops_running_command_and_resume_works() {
        let repo = fixture();
        let manager = LocalWorldManager::new(repo.clone()).unwrap();
        let id = manager.allocate(spec(WorldKind::LocalShell, true)).await.unwrap();
        let task = tokio::spawn(async move { manager.execute(id, "printf ready > ready; (sleep 2; printf bad > late) & wait", 10).await });
        for _ in 0..30 {
            if repo.join("ready").exists() { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(repo.join("ready").exists());
        let other = LocalWorldManager::new(repo.clone()).unwrap();
        other.pause(id).await.unwrap();
        assert_eq!(task.await.unwrap().unwrap().exit_code, 130);
        tokio::time::sleep(Duration::from_millis(2100)).await;
        assert!(!repo.join("late").exists());
        assert!(other.execute(id, "true", 1).await.is_err());
        other.resume(id).await.unwrap();
        assert_eq!(other.execute(id, "printf resumed", 2).await.unwrap().stdout, "resumed");
        other.destroy(id).await.unwrap();
        assert!(repo.join("ready").exists());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn failed_cleanup_is_an_error_and_keeps_dirty_files() {
        let repo = repository().await;
        let manager = LocalWorldManager::new(repo.clone()).unwrap();
        let id = manager.allocate(spec(WorldKind::WorktreeReadWrite, true)).await.unwrap();
        let path = manager.inspect(id).await.unwrap().root_path.unwrap();
        std::fs::write(path.join("keep"), "dirty").unwrap();
        // Never merge into or replace an existing quarantine destination.
        std::fs::create_dir(manager.storage.join("quarantine").join(id.to_string())).unwrap();
        assert!(manager.destroy(id).await.is_err());
        assert_eq!(manager.inspect(id).await.unwrap().status, WorldStatus::Failed);
        assert_eq!(std::fs::read_to_string(path.join("keep")).unwrap(), "dirty");
    }
}
