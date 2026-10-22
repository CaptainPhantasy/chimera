use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// ID types (mirrored from core for crate independence)
// ---------------------------------------------------------------------------

pub type SessionId = Uuid;
pub type CheckpointId = Uuid;

// ---------------------------------------------------------------------------
// Session metadata
// ---------------------------------------------------------------------------

/// Metadata for creating a new session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    /// Human-readable label for the session.
    pub label: String,
    /// The objective that started this session.
    pub objective: String,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Session events
// ---------------------------------------------------------------------------

/// An event in the session event log.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    /// Session was created.
    Created { meta: SessionMeta },
    /// A task was added to the session.
    TaskAdded { description: String },
    /// A creature was spawned.
    CreatureSpawned {
        creature_name: String,
        instance_id: String,
    },
    /// A tool was called.
    ToolCalled {
        tool: String,
        args: serde_json::Value,
    },
    /// An approval was requested.
    ApprovalRequested { action: String, class: String },
    /// An approval decision was made.
    ApprovalDecided { action: String, approved: bool },
    /// A checkpoint was created.
    Checkpointed { checkpoint_id: CheckpointId },
    /// The session was forked.
    Forked {
        parent_id: SessionId,
        child_id: SessionId,
    },
    /// A custom event for extensibility.
    Custom {
        key: String,
        payload: serde_json::Value,
    },
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// Reconstructed state of a session from the event log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// Session identifier.
    pub id: SessionId,
    /// Session metadata.
    pub meta: SessionMeta,
    /// All events in order.
    pub events: Vec<SessionEvent>,
    /// Checkpoint IDs created so far.
    pub checkpoints: Vec<CheckpointId>,
    /// Whether the session is still active.
    pub active: bool,
}

// ---------------------------------------------------------------------------
// SessionStore trait
// ---------------------------------------------------------------------------

/// Durable store for session lifecycle: create, append, snapshot, fork, load.
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    /// Create a new session and return its ID.
    async fn create(&self, meta: SessionMeta) -> anyhow::Result<SessionId>;

    /// Append an event to the session's event log.
    async fn append_event(&self, id: SessionId, event: SessionEvent) -> anyhow::Result<()>;

    /// Create a checkpoint snapshot and return the checkpoint ID.
    async fn snapshot(&self, id: SessionId) -> anyhow::Result<CheckpointId>;

    /// Fork a session, creating a new lineage with inherited state.
    async fn fork(&self, id: SessionId) -> anyhow::Result<SessionId>;

    /// Load the full reconstructed state of a session.
    async fn load(&self, id: SessionId) -> anyhow::Result<SessionState>;

    /// List all known session IDs.
    async fn list(&self) -> anyhow::Result<Vec<SessionId>>;
}

// ---------------------------------------------------------------------------
// File-backed SessionStore
// ---------------------------------------------------------------------------

use anyhow::{bail, Context};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

const FORMAT_VERSION: u32 = 1;
const MAX_RECORD_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SNAPSHOT_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    format_version: u32,
    data: T,
}

/// An explicitly selected relative file and its captured contents. None means absent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileSnapshot {
    pub path: PathBuf,
    pub bytes: Option<Vec<u8>>,
    pub unix_mode: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: CheckpointId,
    pub session_id: SessionId,
    pub created_at: DateTime<Utc>,
    pub state: SessionState,
    pub files: Vec<FileSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestorePreview {
    pub checkpoint_id: CheckpointId,
    /// Pass these exact values to restore_checkpoint after approving this preview.
    pub current_files: Vec<FileSnapshot>,
    pub restored_files: Vec<FileSnapshot>,
}

/// A checkpoint listed for rewind: identity, time and captured paths only.
/// File contents stay in storage until a restore preview asks for them.
#[derive(Debug, Clone)]
pub struct CheckpointSummary {
    pub id: CheckpointId,
    pub created_at: DateTime<Utc>,
    pub files: Vec<PathBuf>,
}

impl SessionState {
    /// Effective conversation/task events, following any state restore. The full
    /// events field remains an append-only audit history of abandoned work too.
    pub fn effective_events(&self) -> anyhow::Result<Vec<SessionEvent>> {
        let mut result = Vec::new();
        for event in &self.events {
            if let SessionEvent::Custom { key, payload } = event {
                if key == "checkpoint_restored" {
                    result = serde_json::from_value(payload["restored_events"].clone())
                        .context("invalid restored session events")?;
                    continue;
                }
            }
            result.push(event.clone());
        }
        Ok(result)
    }
}

/// Versioned, private, recoverable session records. Each writer takes an OS file
/// lock; the lock is automatically released after a crash. Old flat JSON loads.
/// Exclusive ownership of one live runner; released on drop or process exit.
#[derive(Debug)]
pub struct RunLease {
    _file: File,
}

pub struct FileSessionStore {
    root: PathBuf,
}

impl FileSessionStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn storage_dir(&self) -> anyhow::Result<PathBuf> {
        let root = self
            .root
            .canonicalize()
            .context("workspace root is unavailable")?;
        let state = private_dir(&root.join(".chimera"))?;
        private_dir(&state.join("sessions"))
    }

    fn lock(&self) -> anyhow::Result<File> {
        lock_directory(&self.storage_dir()?)
    }

    fn session_path(&self, id: SessionId) -> anyhow::Result<PathBuf> {
        Ok(self.storage_dir()?.join(format!("{id}.json")))
    }

    fn write_state(&self, state: &SessionState) -> anyhow::Result<()> {
        write_record(&self.session_path(state.id)?, state)
    }

    fn read_state(&self, id: SessionId) -> anyhow::Result<SessionState> {
        let path = self.session_path(id)?;
        let state: SessionState = read_record(&path)?;
        if state.id != id {
            bail!("session identity mismatch in {}", path.display());
        }
        state.effective_events()?;
        Ok(state)
    }

    /// Claim a session before reading conversation context and hold this lease
    /// until that run finishes. Separate from event locks, so appends still work.
    pub fn claim_run(&self, id: SessionId) -> anyhow::Result<RunLease> {
        let _state_lock = self.lock()?;
        self.read_state(id)?;
        self.lock_run(id)
    }

    /// Caller holds the state lock. Recovery may need this lock when the current
    /// record cannot be read, so validation belongs to the calling operation.
    fn lock_run(&self, id: SessionId) -> anyhow::Result<RunLease> {
        let path = self.storage_dir()?.join(format!(".run-{id}.lock"));
        reject_link(&path)?;
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts.open(path)?;
        file.try_lock().map_err(|e| {
            anyhow::anyhow!("session {id} already has an active runner or cannot be locked: {e}")
        })?;
        Ok(RunLease { _file: file })
    }

    /// Synchronous callback-safe write; no await occurs while the OS lock is held.
    pub fn append_event_sync(&self, id: SessionId, event: SessionEvent) -> anyhow::Result<()> {
        let _lock = self.lock()?;
        let mut state = self.read_state(id)?;
        state.events.push(event);
        self.write_state(&state)
    }

    /// End a run with an explicit status and retain the complete event history.
    pub async fn close(&self, id: SessionId, outcome: &str) -> anyhow::Result<()> {
        if ![
            "completed",
            "failed",
            "cancelled",
            "interrupted",
            "unfinished",
        ]
        .contains(&outcome)
        {
            bail!("invalid run outcome: {outcome}");
        }
        let _lock = self.lock()?;
        let mut state = self.read_state(id)?;
        state.active = false;
        state.events.push(SessionEvent::Custom {
            key: "run_outcome".into(),
            payload: serde_json::json!({"outcome":outcome,"at":Utc::now()}),
        });
        self.write_state(&state)
    }

    pub async fn reopen(&self, id: SessionId) -> anyhow::Result<()> {
        let _lock = self.lock()?;
        let mut state = self.read_state(id)?;
        state.active = true;
        state.events.push(SessionEvent::Custom {
            key: "run_resumed".into(),
            payload: serde_json::json!({"at":Utc::now()}),
        });
        self.write_state(&state)
    }

    /// Permanently remove a saved conversation and its file record.
    pub async fn delete(&self, id: SessionId) -> anyhow::Result<()> {
        let _lock = self.lock()?;
        let path = self.session_path(id)?;
        if !path.exists() {
            bail!("no saved conversation exists for that id");
        }
        std::fs::remove_file(path)?;
        sync_dir(&self.storage_dir()?)?;
        Ok(())
    }

    fn checkpoint_path(&self, id: CheckpointId) -> anyhow::Result<PathBuf> {
        Ok(private_dir(&self.storage_dir()?.join("checkpoints"))?.join(format!("{id}.json")))
    }

    fn read_checkpoint(&self, session: SessionId, id: CheckpointId) -> anyhow::Result<Checkpoint> {
        let cp: Checkpoint = read_record(&self.checkpoint_path(id)?)?;
        if cp.id != id || cp.session_id != session || cp.state.id != session {
            bail!("checkpoint belongs to a different session");
        }
        Ok(cp)
    }

    /// Capture state and only the caller's explicitly approved relative files.
    pub async fn checkpoint_with_files(
        &self,
        id: SessionId,
        paths: &[PathBuf],
    ) -> anyhow::Result<CheckpointId> {
        let _lock = self.lock()?;
        let mut state = self.read_state(id)?;
        let mut seen = std::collections::HashSet::new();
        let mut files = Vec::new();
        let mut total = 0usize;
        for path in paths {
            if !seen.insert(path.clone()) {
                bail!("duplicate snapshot path: {}", path.display());
            }
            let file = self.capture_file(path)?;
            total += file.bytes.as_ref().map_or(0, Vec::len);
            if total > MAX_SNAPSHOT_BYTES as usize {
                bail!("checkpoint file contents exceed 8 MiB");
            }
            files.push(file);
        }
        let cp = Checkpoint {
            id: Uuid::new_v4(),
            session_id: id,
            created_at: Utc::now(),
            state: state.clone(),
            files,
        };
        write_record(&self.checkpoint_path(cp.id)?, &cp)?;
        state.checkpoints.push(cp.id);
        state.events.push(SessionEvent::Checkpointed {
            checkpoint_id: cp.id,
        });
        self.write_state(&state)?;
        Ok(cp.id)
    }

    /// Every checkpoint of one session, newest first, without pulling file
    /// contents into the listing.
    pub async fn list_checkpoints(
        &self,
        session: SessionId,
    ) -> anyhow::Result<Vec<CheckpointSummary>> {
        let _lock = self.lock()?;
        let state = self.read_state(session)?;
        let mut summaries = Vec::new();
        for id in state.checkpoints {
            let checkpoint = self.read_checkpoint(session, id)?;
            summaries.push(CheckpointSummary {
                id: checkpoint.id,
                created_at: checkpoint.created_at,
                files: checkpoint
                    .files
                    .iter()
                    .map(|file| file.path.clone())
                    .collect(),
            });
        }
        summaries.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
        Ok(summaries)
    }

    pub async fn preview_restore(
        &self,
        session: SessionId,
        checkpoint: CheckpointId,
    ) -> anyhow::Result<RestorePreview> {
        let _lock = self.lock()?;
        let cp = self.read_checkpoint(session, checkpoint)?;
        let current_files = cp
            .files
            .iter()
            .map(|f| self.capture_file(&f.path))
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(RestorePreview {
            checkpoint_id: checkpoint,
            current_files,
            restored_files: cp.files,
        })
    }

    /// Restore an approved preview. Existing files are moved into a private
    /// archive before replacement; writes use create-only links, never overwrite
    /// an intervening user edit. A durable journal allows recovery after failure.
    /// This is a recoverable multi-file operation, not an all-files transaction.
    pub async fn restore_checkpoint(
        &self,
        session: SessionId,
        checkpoint: CheckpointId,
        expected_current: &[FileSnapshot],
    ) -> anyhow::Result<SessionState> {
        let _lock = self.lock()?;
        let _run_lease = self.lock_run(session)?;
        let cp = self.read_checkpoint(session, checkpoint)?;
        let mut current_state = self.read_state(session)?;
        if cp.files.len() != expected_current.len() {
            bail!("restore preview does not cover every file");
        }
        for (desired, expected) in cp.files.iter().zip(expected_current) {
            if desired.path != expected.path || self.capture_file(&desired.path)? != *expected {
                bail!(
                    "file changed since restore preview: {}",
                    desired.path.display()
                );
            }
        }
        let restores = private_dir(&self.storage_dir()?.join("restores"))?;
        let archive = private_dir(&restores.join(Uuid::new_v4().to_string()))?;
        write_record(
            &archive.join("journal.json"),
            &serde_json::json!({"checkpoint":checkpoint,"session":session,"before":current_state,"originals":expected_current,"desired":cp.files,"status":"prepared"}),
        )?;
        for (index, (desired, expected)) in cp.files.iter().zip(expected_current).enumerate() {
            let target = self.scoped_path(&desired.path)?;
            if self.capture_file(&desired.path)? != *expected {
                bail!(
                    "file changed during restore: {}; recovery: {}",
                    desired.path.display(),
                    archive.display()
                );
            }
            if expected.bytes.is_some() {
                let original = archive.join(format!("{index}.original"));
                std::fs::rename(&target, &original).with_context(|| {
                    format!(
                        "archive original {}; recovery: {}",
                        target.display(),
                        archive.display()
                    )
                })?;
                // Recheck bytes after the atomic move, catching a concurrent edit
                // between preview validation and preserving the original.
                let bytes = bounded_read(&original, MAX_SNAPSHOT_BYTES)?;
                if Some(bytes) != expected.bytes {
                    let _ = std::fs::hard_link(&original, &target);
                    bail!(
                        "file changed while archiving {}; original preserved at {}",
                        target.display(),
                        original.display()
                    );
                }
                File::open(&original)?.sync_all()?;
                sync_dir(target.parent().context("file has no parent")?)?;
                sync_dir(&archive)?;
            }
            if let Some(bytes) = &desired.bytes {
                let staged = archive.join(format!("{index}.restored"));
                let mut file = private_new_file(&staged)?;
                file.write_all(bytes)?;
                #[cfg(unix)]
                if let Some(mode) = desired.unix_mode {
                    use std::os::unix::fs::PermissionsExt;
                    file.set_permissions(std::fs::Permissions::from_mode(mode & 0o777))?;
                }
                file.sync_all()?;
                // hard_link refuses an existing destination, preserving any new
                // file created by the user since the original was archived.
                std::fs::hard_link(&staged, &target).with_context(|| {
                    format!(
                        "restore conflict at {}; recovery: {}",
                        target.display(),
                        archive.display()
                    )
                })?;
                sync_dir(target.parent().context("file has no parent")?)?;
            }
        }
        current_state.active = cp.state.active;
        current_state.events.push(SessionEvent::Custom { key:"checkpoint_restored".into(), payload:serde_json::json!({"checkpoint_id":checkpoint,"restored_events":cp.state.effective_events()?,"recovery_directory":archive,"at":Utc::now()}) });
        self.write_state(&current_state)?;
        write_record(
            &archive.join("completed.json"),
            &serde_json::json!({"session":session,"checkpoint":checkpoint,"at":Utc::now()}),
        )?;
        Ok(current_state)
    }

    fn scoped_path(&self, relative: &Path) -> anyhow::Result<PathBuf> {
        if relative.as_os_str().is_empty() {
            bail!("empty checkpoint path");
        }
        let root = self.root.canonicalize()?;
        let mut target = root;
        for component in relative.components() {
            let Component::Normal(name) = component else {
                bail!("checkpoint paths must be relative and contain no traversal");
            };
            let name_text = name.to_string_lossy();
            if [
                ".git",
                ".chimera",
                ".floyd",
                ".supercache",
                ".agents",
                ".codex",
                ".aws",
            ]
            .contains(&name_text.as_ref())
                || name_text == ".env"
                || name_text.starts_with(".env.")
            {
                bail!("protected checkpoint path: {}", relative.display());
            }
            target.push(name);
            match std::fs::symlink_metadata(&target) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    bail!("symlink checkpoint path: {}", target.display())
                }
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        if !target.parent().is_some_and(Path::is_dir) {
            bail!("checkpoint file parent must already exist");
        }
        Ok(target)
    }

    fn capture_file(&self, path: &Path) -> anyhow::Result<FileSnapshot> {
        let target = self.scoped_path(path)?;
        let meta = match std::fs::symlink_metadata(&target) {
            Ok(meta) => Some(meta),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        if meta.as_ref().is_some_and(|m| !m.is_file()) {
            bail!(
                "checkpoint target is not a regular file: {}",
                path.display()
            );
        }
        #[cfg(unix)]
        let unix_mode = {
            use std::os::unix::fs::PermissionsExt;
            meta.as_ref().map(|m| m.permissions().mode() & 0o777)
        };
        #[cfg(not(unix))]
        let unix_mode = None;
        let bytes = if meta.is_some() {
            Some(bounded_read(&target, MAX_SNAPSHOT_BYTES)?)
        } else {
            None
        };
        Ok(FileSnapshot {
            path: path.into(),
            bytes,
            unix_mode,
        })
    }

    /// Explicitly recover the last committed record. The current record is moved
    /// aside intact, including corrupt bytes; recovery is never silently applied.
    pub async fn recover_previous(&self, id: SessionId) -> anyhow::Result<SessionState> {
        let _lock = self.lock()?;
        let _run_lease = self.lock_run(id)?;
        let path = self.session_path(id)?;
        let previous = PathBuf::from(format!("{}.previous", path.display()));
        let mut state: SessionState = read_record(&previous)?;
        if state.id != id {
            bail!("previous session identity mismatch");
        }
        state.effective_events()?;
        reject_link(&path)?;
        let recovery = self
            .storage_dir()?
            .join(format!("{}.recovered-{}", id, Uuid::new_v4()));
        if path.exists() {
            std::fs::rename(&path, &recovery)?;
            sync_dir(&self.storage_dir()?)?;
        }
        state.events.push(SessionEvent::Custom {
            key: "record_recovered".into(),
            payload: serde_json::json!({"preserved_record": recovery, "at": Utc::now()}),
        });
        self.write_state(&state)?;
        Ok(state)
    }

    /// Privacy-first diagnostics: no conversation text, tool arguments, paths,
    /// labels, or arbitrary custom payloads are exported.
    pub async fn export_redacted(&self, id: SessionId) -> anyhow::Result<serde_json::Value> {
        let state = self.load(id).await?;
        let events: Vec<_> = state
            .events
            .iter()
            .map(|e| {
                let kind = match e {
                    SessionEvent::Created { .. } => "created",
                    SessionEvent::TaskAdded { .. } => "task_added",
                    SessionEvent::CreatureSpawned { .. } => "creature_spawned",
                    SessionEvent::ToolCalled { .. } => "tool_called",
                    SessionEvent::ApprovalRequested { .. } => "approval_requested",
                    SessionEvent::ApprovalDecided { .. } => "approval_decided",
                    SessionEvent::Checkpointed { .. } => "checkpointed",
                    SessionEvent::Forked { .. } => "forked",
                    SessionEvent::Custom { .. } => "custom",
                };
                serde_json::json!({"type":kind,"details":"[redacted]"})
            })
            .collect();
        Ok(
            serde_json::json!({"format_version":FORMAT_VERSION,"id":state.id,"created_at":state.meta.created_at,"active":state.active,"checkpoints":state.checkpoints,"events":events}),
        )
    }
}

#[async_trait::async_trait]
impl SessionStore for FileSessionStore {
    async fn create(&self, meta: SessionMeta) -> anyhow::Result<SessionId> {
        // Support a fresh test/workspace directory without following state links.
        if !self.root.exists() {
            std::fs::create_dir_all(&self.root)?;
        }
        let _lock = self.lock()?;
        let id = Uuid::new_v4();
        let state = SessionState {
            id,
            meta: meta.clone(),
            events: vec![SessionEvent::Created { meta }],
            checkpoints: vec![],
            active: true,
        };
        self.write_state(&state)?;
        Ok(id)
    }
    async fn append_event(&self, id: SessionId, event: SessionEvent) -> anyhow::Result<()> {
        self.append_event_sync(id, event)
    }
    async fn snapshot(&self, id: SessionId) -> anyhow::Result<CheckpointId> {
        self.checkpoint_with_files(id, &[]).await
    }
    async fn fork(&self, id: SessionId) -> anyhow::Result<SessionId> {
        let _lock = self.lock()?;
        let mut child = self.read_state(id)?;
        child.id = Uuid::new_v4();
        child.active = true;
        // Parent checkpoints are immutable and bound to their parent identity.
        child.checkpoints.clear();
        child.events.push(SessionEvent::Forked {
            parent_id: id,
            child_id: child.id,
        });
        self.write_state(&child)?;
        Ok(child.id)
    }
    async fn load(&self, id: SessionId) -> anyhow::Result<SessionState> {
        let _lock = self.lock()?;
        self.read_state(id)
    }
    async fn list(&self) -> anyhow::Result<Vec<SessionId>> {
        if !self.root.join(".chimera").join("sessions").exists() {
            return Ok(vec![]);
        }
        let _lock = self.lock()?;
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(self.storage_dir()?)? {
            let path = entry?.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Some(id) = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| Uuid::parse_str(s).ok())
                {
                    ids.push(id);
                }
            }
        }
        ids.sort();
        Ok(ids)
    }
}

fn private_dir(path: &Path) -> anyhow::Result<PathBuf> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => (),
        Ok(_) => bail!(
            "refusing non-directory or symlink state path: {}",
            path.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(path) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e.into()),
            }
            let meta = std::fs::symlink_metadata(path)?;
            if !meta.is_dir() || meta.file_type().is_symlink() {
                bail!("unsafe state directory: {}", path.display());
            }
        }
        Err(e) => return Err(e.into()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(path.into())
}

fn reject_link(path: &Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            bail!("refusing non-regular state file: {}", path.display())
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn private_new_file(path: &Path) -> anyhow::Result<File> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    Ok(opts.open(path)?)
}

fn lock_directory(dir: &Path) -> anyhow::Result<File> {
    let path = dir.join(".lock");
    reject_link(&path)?;
    let mut opts = OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts.open(&path)?;
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock)
                if started.elapsed() < Duration::from_secs(5) =>
            {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(e) => bail!("session lock unavailable at {}: {e}", path.display()),
        }
    }
}

fn bounded_read(path: &Path, limit: u64) -> anyhow::Result<Vec<u8>> {
    reject_link(path)?;
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        bail!("file exceeds storage size limit: {}", path.display());
    }
    Ok(bytes)
}

fn read_record<T: serde::de::DeserializeOwned>(path: &Path) -> anyhow::Result<T> {
    let bytes = bounded_read(path, MAX_RECORD_BYTES)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "corrupt record {}; previous version is {}.previous",
            path.display(),
            path.display()
        )
    })?;
    if let Some(version) = value.get("format_version") {
        if version.as_u64() != Some(FORMAT_VERSION as u64) {
            bail!("unsupported record format {version} at {}", path.display());
        }
        serde_json::from_value(
            value
                .get("data")
                .cloned()
                .context("versioned record has no data")?,
        )
        .with_context(|| format!("invalid record data at {}", path.display()))
    } else {
        serde_json::from_value(value)
            .with_context(|| format!("invalid legacy record at {}", path.display()))
    }
}

fn sync_dir(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    Ok(())
}

fn atomic_bytes(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    reject_link(path)?;
    let parent = path.parent().context("record has no parent")?;
    let temporary = parent.join(format!(".pending-{}", Uuid::new_v4()));
    let mut file = private_new_file(&temporary)?;
    #[cfg(test)]
    if std::env::var_os("CHIMERA_INTERRUPT_ATOMIC_TARGET").as_deref() == Some(path.as_os_str()) {
        // The child test is killed at a real partial-write boundary, before installation.
        file.write_all(&bytes[..bytes.len() / 2])?;
        file.sync_all()?;
        std::fs::write(
            std::env::var_os("CHIMERA_INTERRUPT_READY").expect("test signal path"),
            b"partial temporary saved",
        )?;
        loop {
            std::thread::park();
        }
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)?;
    sync_dir(parent)
}

fn write_record<T: Serialize>(path: &Path, data: &T) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(&Envelope {
        format_version: FORMAT_VERSION,
        data,
    })?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        bail!("record exceeds 32 MiB limit");
    }
    reject_link(path)?;
    if path.exists() {
        let old = bounded_read(path, MAX_RECORD_BYTES)?;
        let previous = PathBuf::from(format!("{}.previous", path.display()));
        atomic_bytes(&previous, &old)?;
    }
    atomic_bytes(path, &bytes)
}

#[cfg(test)]
mod file_tests {
    use super::*;

    fn unique_root() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("chimera-session-test-{nanos}"))
    }

    fn sample_meta() -> SessionMeta {
        SessionMeta {
            label: "test".to_string(),
            objective: "do the thing".to_string(),
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn create_append_load_roundtrip() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let id = store.create(sample_meta()).await.unwrap();
        store
            .append_event(
                id,
                SessionEvent::TaskAdded {
                    description: "step1".to_string(),
                },
            )
            .await
            .unwrap();
        let state = store.load(id).await.unwrap();
        assert_eq!(state.id, id);
        // Created + TaskAdded
        assert_eq!(state.events.len(), 2);
        assert!(matches!(state.events[0], SessionEvent::Created { .. }));
    }

    #[tokio::test]
    async fn snapshot_records_checkpoint() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let id = store.create(sample_meta()).await.unwrap();
        let cp = store.snapshot(id).await.unwrap();
        let state = store.load(id).await.unwrap();
        assert!(state.checkpoints.contains(&cp));
    }

    #[tokio::test]
    async fn fork_inherits_and_marks() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let parent = store.create(sample_meta()).await.unwrap();
        let child = store.fork(parent).await.unwrap();
        assert_ne!(parent, child);
        let child_state = store.load(child).await.unwrap();
        assert!(child_state
            .events
            .iter()
            .any(|e| matches!(e, SessionEvent::Forked { parent_id: p, child_id: c } if *p == parent && *c == child)));
    }

    #[tokio::test]
    async fn list_returns_created_sessions() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let a = store.create(sample_meta()).await.unwrap();
        let b = store.create(sample_meta()).await.unwrap();
        let ids = store.list().await.unwrap();
        assert!(ids.contains(&a));
        assert!(ids.contains(&b));
    }

    #[tokio::test]
    async fn list_on_empty_dir_returns_empty() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let ids = store.list().await.unwrap();
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn delete_removes_the_record_and_is_refused_for_unknown_ids() {
        let root = unique_root();
        let store = FileSessionStore::new(root.clone());
        let id = store.create(sample_meta()).await.unwrap();
        assert!(store.load(id).await.is_ok());
        store.delete(id).await.unwrap();
        assert!(store.load(id).await.is_err());
        assert!(!store.list().await.unwrap().contains(&id));
        assert!(store.delete(id).await.is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn legacy_load_versioned_save_and_explicit_corruption() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let id = store.create(sample_meta()).await.unwrap();
        let path = store.session_path(id).unwrap();
        let legacy = store.load(id).await.unwrap();
        std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(store.load(id).await.unwrap().id, id);
        store
            .append_event(
                id,
                SessionEvent::TaskAdded {
                    description: "new".into(),
                },
            )
            .await
            .unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["format_version"], 1);
        std::fs::write(&path, b"broken").unwrap();
        assert!(store
            .load(id)
            .await
            .unwrap_err()
            .to_string()
            .contains("corrupt record"));
        let previous: SessionState =
            read_record(&PathBuf::from(format!("{}.previous", path.display()))).unwrap();
        assert_eq!(previous.id, id);
        std::fs::write(&path, br#"{"format_version":999,"data":{}}"#).unwrap();
        assert!(store
            .load(id)
            .await
            .unwrap_err()
            .to_string()
            .contains("unsupported record format"));
    }

    #[tokio::test]
    async fn explicit_recovery_preserves_corrupt_original() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let id = store.create(sample_meta()).await.unwrap();
        store
            .append_event_sync(
                id,
                SessionEvent::TaskAdded {
                    description: "later".into(),
                },
            )
            .unwrap();
        let path = store.session_path(id).unwrap();
        std::fs::write(&path, b"corrupt original bytes").unwrap();
        let recovered = store.recover_previous(id).await.unwrap();
        assert_eq!(recovered.id, id);
        assert!(
            matches!(recovered.events.last(), Some(SessionEvent::Custom { key, .. }) if key == "record_recovered")
        );
        let archive = std::fs::read_dir(store.storage_dir().unwrap())
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.to_string_lossy().contains(".recovered-"))
            .unwrap();
        assert_eq!(std::fs::read(archive).unwrap(), b"corrupt original bytes");
        assert!(store.load(id).await.is_ok());
    }

    #[tokio::test]
    async fn failed_replacement_keeps_committed_session() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let id = store.create(sample_meta()).await.unwrap();
        let path = store.session_path(id).unwrap();
        let before = std::fs::read(&path).unwrap();
        std::fs::create_dir(PathBuf::from(format!("{}.previous", path.display()))).unwrap();
        assert!(store
            .append_event(
                id,
                SessionEvent::TaskAdded {
                    description: "never committed".into()
                }
            )
            .await
            .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(store.load(id).await.unwrap().events.len(), 1);
    }

    #[tokio::test]
    async fn checkpoints_restore_effective_context_and_preserve_audit() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let id = store.create(sample_meta()).await.unwrap();
        store
            .append_event(
                id,
                SessionEvent::Custom {
                    key: "messages".into(),
                    payload: serde_json::json!(["before"]),
                },
            )
            .await
            .unwrap();
        let cp = store.snapshot(id).await.unwrap();
        store
            .append_event(
                id,
                SessionEvent::Custom {
                    key: "messages".into(),
                    payload: serde_json::json!(["after"]),
                },
            )
            .await
            .unwrap();
        let state = store.restore_checkpoint(id, cp, &[]).await.unwrap();
        let effective = serde_json::to_string(&state.effective_events().unwrap()).unwrap();
        assert!(effective.contains("before"));
        assert!(!effective.contains("after"));
        assert!(serde_json::to_string(&state.events)
            .unwrap()
            .contains("after"));
        store.close(id, "cancelled").await.unwrap();
        assert!(!store.load(id).await.unwrap().active);
        store.reopen(id).await.unwrap();
        assert!(store.load(id).await.unwrap().active);
    }

    #[tokio::test]
    async fn file_restore_rejects_stale_preview_and_preserves_original() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let id = store.create(sample_meta()).await.unwrap();
        let file = root.join("test.txt");
        std::fs::write(&file, b"checkpoint bytes").unwrap();
        let cp = store
            .checkpoint_with_files(id, &[PathBuf::from("test.txt")])
            .await
            .unwrap();
        std::fs::write(&file, b"later user work").unwrap();
        let preview = store.preview_restore(id, cp).await.unwrap();
        std::fs::write(&file, b"newest user work").unwrap();
        assert!(store
            .restore_checkpoint(id, cp, &preview.current_files)
            .await
            .unwrap_err()
            .to_string()
            .contains("changed since restore preview"));
        assert_eq!(std::fs::read(&file).unwrap(), b"newest user work");
        let fresh = store.preview_restore(id, cp).await.unwrap();
        store
            .restore_checkpoint(id, cp, &fresh.current_files)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"checkpoint bytes");
        let dirs = std::fs::read_dir(store.storage_dir().unwrap().join("restores"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(
            std::fs::read(dirs[0].join("0.original")).unwrap(),
            b"newest user work"
        );
        assert!(dirs[0].join("journal.json").exists());
        assert!(dirs[0].join("completed.json").exists());
    }

    #[tokio::test]
    async fn restore_absent_file_quarantines_later_file_and_parent_cannot_use_child_checkpoint() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let id = store.create(sample_meta()).await.unwrap();
        let cp = store
            .checkpoint_with_files(id, &[PathBuf::from("later.txt")])
            .await
            .unwrap();
        std::fs::write(root.join("later.txt"), b"preserved").unwrap();
        let preview = store.preview_restore(id, cp).await.unwrap();
        store
            .restore_checkpoint(id, cp, &preview.current_files)
            .await
            .unwrap();
        assert!(!root.join("later.txt").exists());
        let child = store.fork(id).await.unwrap();
        assert!(store.load(child).await.unwrap().checkpoints.is_empty());
        assert!(store.preview_restore(child, cp).await.is_err());
    }

    #[tokio::test]
    async fn checkpoint_rejects_traversal_protected_and_oversized_files() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let id = store.create(sample_meta()).await.unwrap();
        for path in [
            "../outside",
            "/tmp/outside",
            ".env",
            ".git/config",
            ".chimera/sessions/nope",
        ] {
            assert!(
                store
                    .checkpoint_with_files(id, &[PathBuf::from(path)])
                    .await
                    .is_err(),
                "{path}"
            );
        }
        let file = File::create(root.join("large.txt")).unwrap();
        file.set_len(MAX_SNAPSHOT_BYTES + 1).unwrap();
        assert!(store
            .checkpoint_with_files(id, &[PathBuf::from("large.txt")])
            .await
            .is_err());
    }

    #[tokio::test]
    async fn list_checkpoints_is_newest_first_with_paths_not_bytes() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let id = store.create(sample_meta()).await.unwrap();
        std::fs::write(root.join("older.txt"), b"OLDER_CAPTURED_BYTES").unwrap();
        let first = store
            .checkpoint_with_files(id, &[PathBuf::from("older.txt")])
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        std::fs::write(root.join("newer.txt"), b"NEWER_CAPTURED_BYTES").unwrap();
        let second = store
            .checkpoint_with_files(id, &[PathBuf::from("newer.txt")])
            .await
            .unwrap();
        let summaries = store.list_checkpoints(id).await.unwrap();
        assert_eq!(
            summaries.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![second, first]
        );
        assert_eq!(summaries[0].files, vec![PathBuf::from("newer.txt")]);
        assert_eq!(summaries[1].files, vec![PathBuf::from("older.txt")]);
        // The listing carries paths only — CheckpointSummary has no bytes
        // field, so captured contents stay in storage until a preview asks.
        // Another session never sees this session's checkpoints.
        let stranger = store.create(sample_meta()).await.unwrap();
        assert!(store.list_checkpoints(stranger).await.unwrap().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn private_permissions_and_symlink_refusal() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let id = store.create(sample_meta()).await.unwrap();
        assert_eq!(
            std::fs::metadata(store.storage_dir().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(store.session_path(id).unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let outside = root.join("outside");
        std::fs::write(&outside, b"untouched").unwrap();
        symlink(&outside, root.join("linked")).unwrap();
        assert!(store
            .checkpoint_with_files(id, &[PathBuf::from("linked")])
            .await
            .is_err());
        let second = unique_root();
        std::fs::create_dir_all(&second).unwrap();
        symlink(&root, second.join(".chimera")).unwrap();
        assert!(FileSessionStore::new(second)
            .create(sample_meta())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn redacted_export_omits_all_free_text_and_arguments() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let id = store
            .create(SessionMeta {
                label: "SECRET_LABEL".into(),
                objective: "SECRET_OBJECTIVE".into(),
                created_at: Utc::now(),
            })
            .await
            .unwrap();
        store
            .append_event(
                id,
                SessionEvent::ToolCalled {
                    tool: "SECRET_TOOL".into(),
                    args: serde_json::json!({"value":"SECRET_PAYLOAD"}),
                },
            )
            .await
            .unwrap();
        let text = store.export_redacted(id).await.unwrap().to_string();
        assert!(!text.contains("SECRET"));
        assert!(text.contains("redacted"));
    }

    #[test]
    fn process_writer() {
        let Ok(root) = std::env::var("CHIMERA_SESSION_TEST_ROOT") else {
            return;
        };
        let store = FileSessionStore::new(root);
        if std::env::var("CHIMERA_SESSION_TEST_LOCK_EXIT").is_ok() {
            let _lock = store.lock().unwrap();
            std::process::exit(0);
        }
        let id = Uuid::parse_str(&std::env::var("CHIMERA_SESSION_TEST_ID").unwrap()).unwrap();
        for _ in 0..12 {
            store
                .append_event_sync(
                    id,
                    SessionEvent::TaskAdded {
                        description: "child".into(),
                    },
                )
                .unwrap();
        }
    }

    #[tokio::test]
    async fn independent_processes_keep_every_event_and_crashed_lock_releases() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let id = store.create(sample_meta()).await.unwrap();
        let exe = std::env::current_exe().unwrap();
        let mut children = Vec::new();
        for _ in 0..3 {
            children.push(
                std::process::Command::new(&exe)
                    .args(["--exact", "file_tests::process_writer", "--test-threads=1"])
                    .env("CHIMERA_SESSION_TEST_ROOT", &root)
                    .env("CHIMERA_SESSION_TEST_ID", id.to_string())
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .unwrap(),
            );
        }
        for mut child in children {
            assert!(child.wait().unwrap().success());
        }
        assert_eq!(store.load(id).await.unwrap().events.len(), 37);
        assert!(std::process::Command::new(&exe)
            .args(["--exact", "file_tests::process_writer"])
            .env("CHIMERA_SESSION_TEST_ROOT", &root)
            .env("CHIMERA_SESSION_TEST_LOCK_EXIT", "1")
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap()
            .success());
        store
            .append_event_sync(
                id,
                SessionEvent::TaskAdded {
                    description: "after process exit".into(),
                },
            )
            .unwrap();
        assert_eq!(store.load(id).await.unwrap().events.len(), 38);
    }
    #[tokio::test]
    async fn run_lease_prevents_competing_context_writers_without_blocking_events() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let id = store.create(sample_meta()).await.unwrap();
        let lease = store.claim_run(id).unwrap();
        assert!(FileSessionStore::new(&root).claim_run(id).is_err());
        store
            .append_event_sync(
                id,
                SessionEvent::TaskAdded {
                    description: "lease holder".into(),
                },
            )
            .unwrap();
        drop(lease);
        assert!(FileSessionStore::new(&root).claim_run(id).is_ok());
    }

    #[tokio::test]
    async fn active_run_blocks_checkpoint_restore_without_changing_files_or_history() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let id = store.create(sample_meta()).await.unwrap();
        let file = root.join("selected.txt");
        let unrelated = root.join("unrelated.txt");
        std::fs::write(&file, b"checkpoint content").unwrap();
        std::fs::write(&unrelated, b"unrelated work").unwrap();
        store
            .append_event_sync(
                id,
                SessionEvent::Custom {
                    key: "messages".into(),
                    payload: serde_json::json!(["before"]),
                },
            )
            .unwrap();
        let checkpoint = store
            .checkpoint_with_files(id, &[PathBuf::from("selected.txt")])
            .await
            .unwrap();
        store
            .append_event_sync(
                id,
                SessionEvent::Custom {
                    key: "messages".into(),
                    payload: serde_json::json!(["active turn"]),
                },
            )
            .unwrap();
        std::fs::write(&file, b"later user work").unwrap();
        let preview = store.preview_restore(id, checkpoint).await.unwrap();
        let saved_before = std::fs::read(store.session_path(id).unwrap()).unwrap();
        let lease = store.claim_run(id).unwrap();
        let other = FileSessionStore::new(&root);
        let error = other
            .restore_checkpoint(id, checkpoint, &preview.current_files)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("active runner"));
        assert_eq!(
            std::fs::read(store.session_path(id).unwrap()).unwrap(),
            saved_before
        );
        assert_eq!(std::fs::read(&file).unwrap(), b"later user work");
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"unrelated work");
        assert!(!store.storage_dir().unwrap().join("restores").exists());

        drop(lease);
        let state = other
            .restore_checkpoint(id, checkpoint, &preview.current_files)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"checkpoint content");
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"unrelated work");
        assert!(serde_json::to_string(&state.events)
            .unwrap()
            .contains("active turn"));
        assert!(!serde_json::to_string(&state.effective_events().unwrap())
            .unwrap()
            .contains("active turn"));
        assert!(store.claim_run(id).is_ok());
    }

    #[tokio::test]
    async fn active_run_blocks_corrupt_record_recovery_until_lease_releases() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let id = store.create(sample_meta()).await.unwrap();
        store
            .append_event_sync(
                id,
                SessionEvent::TaskAdded {
                    description: "saved turn".into(),
                },
            )
            .unwrap();
        let lease = store.claim_run(id).unwrap();
        let path = store.session_path(id).unwrap();
        let previous = PathBuf::from(format!("{}.previous", path.display()));
        let saved_previous = std::fs::read(&previous).unwrap();
        std::fs::write(&path, b"damaged current record").unwrap();
        let other = FileSessionStore::new(&root);
        let error = other.recover_previous(id).await.unwrap_err();
        assert!(error.to_string().contains("active runner"));
        assert_eq!(std::fs::read(&path).unwrap(), b"damaged current record");
        assert_eq!(std::fs::read(&previous).unwrap(), saved_previous);
        assert!(!std::fs::read_dir(store.storage_dir().unwrap())
            .unwrap()
            .any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".recovered-")
            }));

        drop(lease);
        let state = other.recover_previous(id).await.unwrap();
        assert_eq!(state.id, id);
        assert!(state.events.iter().any(|event| matches!(event,
            SessionEvent::Custom { key, .. } if key == "record_recovered")));
        let preserved = std::fs::read_dir(store.storage_dir().unwrap())
            .unwrap()
            .find_map(|entry| {
                let path = entry.unwrap().path();
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains(".recovered-")
                    .then_some(path)
            })
            .unwrap();
        assert_eq!(std::fs::read(preserved).unwrap(), b"damaged current record");
        assert!(store.claim_run(id).is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn io_failure_worker() {
        let Ok(root) = std::env::var("CHIMERA_SESSION_IO_ROOT") else {
            return;
        };
        let mode = std::env::var("CHIMERA_SESSION_IO_MODE").unwrap();
        if mode == "limit" {
            #[repr(C)]
            struct Limit {
                current: u64,
                maximum: u64,
            }
            unsafe extern "C" {
                fn setrlimit(resource: i32, limit: *const Limit) -> i32;
                fn signal(number: i32, handler: usize) -> usize;
            }
            unsafe {
                signal(25, 1);
                assert_eq!(
                    setrlimit(
                        1,
                        &Limit {
                            current: 4096,
                            maximum: 4096
                        }
                    ),
                    0
                );
            }
        }
        let id = Uuid::parse_str(&std::env::var("CHIMERA_SESSION_IO_ID").unwrap()).unwrap();
        let result = FileSessionStore::new(root).append_event_sync(
            id,
            SessionEvent::TaskAdded {
                description: "new content".repeat(10_000),
            },
        );
        assert!(
            result.is_err(),
            "a rejected or size-limited write must fail"
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn partial_write_error_and_killed_writer_preserve_committed_bytes() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let id = store.create(sample_meta()).await.unwrap();
        let path = store.session_path(id).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(before.len() < 4096);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "file_tests::io_failure_worker"])
            .env("CHIMERA_SESSION_IO_ROOT", &root)
            .env("CHIMERA_SESSION_IO_ID", id.to_string())
            .env("CHIMERA_SESSION_IO_MODE", "limit")
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(store.load(id).await.unwrap().events.len(), 1);
        let ready = root.join("writer-ready");
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "file_tests::io_failure_worker"])
            .env("CHIMERA_SESSION_IO_ROOT", &root)
            .env("CHIMERA_SESSION_IO_ID", id.to_string())
            .env("CHIMERA_SESSION_IO_MODE", "interrupt")
            .env("CHIMERA_INTERRUPT_ATOMIC_TARGET", &path)
            .env("CHIMERA_INTERRUPT_READY", &ready)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !ready.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        child.kill().await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            std::fs::read(PathBuf::from(format!("{}.previous", path.display()))).unwrap(),
            before
        );
        assert_eq!(store.load(id).await.unwrap().events.len(), 1);
        store
            .append_event_sync(
                id,
                SessionEvent::TaskAdded {
                    description: "after failed writers".into(),
                },
            )
            .unwrap();
        assert_eq!(store.load(id).await.unwrap().events.len(), 2);
        assert!(std::fs::read_dir(store.storage_dir().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with(".pending-")));
    }
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn operating_system_read_only_storage_does_not_report_an_event_saved() {
        let root = unique_root();
        let store = FileSessionStore::new(&root);
        let id = store.create(sample_meta()).await.unwrap();
        let path = store.session_path(id).unwrap();
        let before = std::fs::read(&path).unwrap();
        let state = serde_json::to_string(
            &root
                .canonicalize()
                .unwrap()
                .join(".chimera")
                .to_string_lossy(),
        )
        .unwrap();
        let profile = format!("(version 1)(allow default)(deny file-write* (subpath {state}))");
        let status = std::process::Command::new("/usr/bin/sandbox-exec")
            .args(["-p", &profile])
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", "file_tests::io_failure_worker"])
            .env("CHIMERA_SESSION_IO_ROOT", &root)
            .env("CHIMERA_SESSION_IO_ID", id.to_string())
            .env("CHIMERA_SESSION_IO_MODE", "deny")
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(std::fs::read(path).unwrap(), before);
        assert_eq!(store.load(id).await.unwrap().events.len(), 1);
    }
}
