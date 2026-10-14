use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// ID types
// ---------------------------------------------------------------------------

pub type SessionId = Uuid;
pub type CreatureId = Uuid;

// ---------------------------------------------------------------------------
// Trace event
// ---------------------------------------------------------------------------

/// The severity / category of a trace event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// A structured trace event emitted by the harness or a creature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEvent {
    /// When the event occurred.
    pub timestamp: DateTime<Utc>,
    /// Severity level.
    pub level: TraceLevel,
    /// Which session this event belongs to.
    pub session_id: SessionId,
    /// Which creature emitted the event (if any).
    pub creature_id: Option<CreatureId>,
    /// The span or operation name.
    pub span: String,
    /// Human-readable message.
    pub message: String,
    /// Structured metadata.
    pub fields: serde_json::Value,
}

// ---------------------------------------------------------------------------
// TraceSink trait
// ---------------------------------------------------------------------------

/// Sink for structured trace events. Supports emit and query.
#[async_trait::async_trait]
pub trait TraceSink: Send + Sync {
    /// Emit a trace event to the sink.
    async fn emit(&self, event: TraceEvent) -> anyhow::Result<()>;

    /// Query all trace events for a session.
    async fn query(&self, session_id: SessionId) -> anyhow::Result<Vec<TraceEvent>>;

    /// Query trace events for a specific creature within a session.
    async fn query_creature(
        &self,
        session_id: SessionId,
        creature_id: CreatureId,
    ) -> anyhow::Result<Vec<TraceEvent>>;
}

// ---------------------------------------------------------------------------
// File-backed TraceSink
// ---------------------------------------------------------------------------

use anyhow::{bail, Context};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const FORMAT_VERSION: u32 = 1;
const MAX_TRACE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct StoredEvent {
    format_version: u32,
    data: TraceEvent,
}

/// Durable versioned JSONL records. No memory cache can hide historical events
/// or claim an event survived a failed write. Per-directory OS locking also
/// protects independent processes. A previous committed file is retained.
pub struct FileTraceSink {
    root: PathBuf,
}

impl FileTraceSink {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn directory(&self) -> anyhow::Result<PathBuf> {
        if !self.root.exists() {
            std::fs::create_dir_all(&self.root)?;
        }
        let root = self.root.canonicalize()?;
        let state = private_dir(&root.join(".chimera"))?;
        private_dir(&state.join("traces"))
    }

    fn trace_path(&self, id: SessionId) -> anyhow::Result<PathBuf> {
        Ok(self.directory()?.join(format!("{id}.jsonl")))
    }

    fn read_events(&self, id: SessionId) -> anyhow::Result<Vec<TraceEvent>> {
        let path = self.trace_path(id)?;
        reject_link(&path)?;
        if !path.exists() {
            return Ok(vec![]);
        }
        let bytes = bounded_read(&path)?;
        let raw = std::str::from_utf8(&bytes)
            .with_context(|| format!("invalid UTF-8 trace at {}", path.display()))?;
        let mut events = Vec::new();
        for (index, line) in raw.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let value: serde_json::Value = serde_json::from_str(line).with_context(|| {
                format!(
                    "corrupt trace {} line {}; previous version is {}.previous",
                    path.display(),
                    index + 1,
                    path.display()
                )
            })?;
            let event: TraceEvent = if let Some(version) = value.get("format_version") {
                if version.as_u64() != Some(FORMAT_VERSION as u64) {
                    bail!("unsupported trace version {version}");
                }
                serde_json::from_value(
                    value
                        .get("data")
                        .cloned()
                        .context("trace envelope lacks data")?,
                )?
            } else {
                serde_json::from_value(value)?
            };
            if event.session_id != id {
                bail!(
                    "trace session identity mismatch in {} line {}",
                    path.display(),
                    index + 1
                );
            }
            events.push(event);
        }
        Ok(events)
    }

    /// Callback-safe durable emission. The write either replaces the committed
    /// file in full or reports an error. Interrupted temporary files are retained.
    pub fn emit_sync(&self, event: TraceEvent) -> anyhow::Result<()> {
        let dir = self.directory()?;
        let _lock = lock_directory(&dir)?;
        let path = self.trace_path(event.session_id)?;
        // Validate the old stream before extending it; never silently skip damage.
        self.read_events(event.session_id)?;
        let mut bytes = if path.exists() {
            bounded_read(&path)?
        } else {
            Vec::new()
        };
        if !bytes.is_empty() && bytes.last() != Some(&b'\n') {
            bytes.push(b'\n');
        }
        bytes.extend(serde_json::to_vec(&StoredEvent {
            format_version: FORMAT_VERSION,
            data: event,
        })?);
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_TRACE_BYTES {
            bail!(
                "trace exceeds 32 MiB storage limit; export/archive this session before continuing"
            );
        }
        if path.exists() {
            atomic_bytes(
                &PathBuf::from(format!("{}.previous", path.display())),
                &bounded_read(&path)?,
            )?;
        }
        atomic_bytes(&path, &bytes)
    }

    /// Export useful structural evidence without arbitrary messages, spans,
    /// paths, or metadata that might contain credentials or private source.
    pub async fn export_redacted(&self, id: SessionId) -> anyhow::Result<serde_json::Value> {
        let events = self.query(id).await?;
        Ok(
            serde_json::json!({"format_version":FORMAT_VERSION,"session_id":id,"events":events.iter().map(|e|serde_json::json!({"timestamp":e.timestamp,"level":e.level,"session_id":e.session_id,"creature_id":e.creature_id,"message":"[redacted]","fields":"[redacted]"})).collect::<Vec<_>>()}),
        )
    }
}

#[async_trait::async_trait]
impl TraceSink for FileTraceSink {
    async fn emit(&self, event: TraceEvent) -> anyhow::Result<()> {
        self.emit_sync(event)
    }
    async fn query(&self, session_id: SessionId) -> anyhow::Result<Vec<TraceEvent>> {
        let _lock = lock_directory(&self.directory()?)?;
        self.read_events(session_id)
    }
    async fn query_creature(
        &self,
        session_id: SessionId,
        creature_id: CreatureId,
    ) -> anyhow::Result<Vec<TraceEvent>> {
        Ok(self
            .query(session_id)
            .await?
            .into_iter()
            .filter(|event| event.creature_id == Some(creature_id))
            .collect())
    }
}

fn private_dir(path: &Path) -> anyhow::Result<PathBuf> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => (),
        Ok(_) => bail!("refusing unsafe trace directory: {}", path.display()),
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
                bail!("unsafe trace directory: {}", path.display());
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
            bail!("refusing non-regular trace file: {}", path.display())
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
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
    let file = opts.open(path)?;
    let start = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if start.elapsed() < Duration::from_secs(5) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(e) => bail!("trace lock unavailable: {e}"),
        }
    }
}

fn bounded_read(path: &Path) -> anyhow::Result<Vec<u8>> {
    reject_link(path)?;
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_TRACE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_TRACE_BYTES {
        bail!("trace exceeds 32 MiB storage limit");
    }
    Ok(bytes)
}

fn atomic_bytes(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    reject_link(path)?;
    let parent = path.parent().context("trace has no parent")?;
    let temp = parent.join(format!(".pending-{}", Uuid::new_v4()));
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(temp, path)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod file_tests {
    use super::*;

    fn unique_root() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("chimera-trace-test-{nanos}"))
    }

    fn sample_event(session: SessionId, creature: Option<CreatureId>) -> TraceEvent {
        TraceEvent {
            timestamp: Utc::now(),
            level: TraceLevel::Info,
            session_id: session,
            creature_id: creature,
            span: "test".to_string(),
            message: "hello".to_string(),
            fields: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn emit_and_query_roundtrip() {
        let root = unique_root();
        let sink = FileTraceSink::new(&root);
        let sid = Uuid::new_v4();
        sink.emit(sample_event(sid, None)).await.unwrap();
        sink.emit(sample_event(sid, None)).await.unwrap();
        let events = sink.query(sid).await.unwrap();
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn query_creature_filters() {
        let root = unique_root();
        let sink = FileTraceSink::new(&root);
        let sid = Uuid::new_v4();
        let c1 = Uuid::new_v4();
        let c2 = Uuid::new_v4();
        sink.emit(sample_event(sid, Some(c1))).await.unwrap();
        sink.emit(sample_event(sid, Some(c2))).await.unwrap();
        sink.emit(sample_event(sid, Some(c1))).await.unwrap();
        let c1_events = sink.query_creature(sid, c1).await.unwrap();
        assert_eq!(c1_events.len(), 2);
        let c2_events = sink.query_creature(sid, c2).await.unwrap();
        assert_eq!(c2_events.len(), 1);
    }

    #[tokio::test]
    async fn persists_across_instances() {
        let root = unique_root();
        let sid = Uuid::new_v4();
        {
            let sink = FileTraceSink::new(&root);
            sink.emit(sample_event(sid, None)).await.unwrap();
        }
        // New instance reading the same root should load from disk.
        let sink2 = FileTraceSink::new(&root);
        let events = sink2.query(sid).await.unwrap();
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn query_empty_session_returns_empty() {
        let root = unique_root();
        let sink = FileTraceSink::new(&root);
        let events = sink.query(Uuid::new_v4()).await.unwrap();
        assert!(events.is_empty());
    }
    #[tokio::test]
    async fn reopen_then_append_includes_prior_history() {
        let root = unique_root();
        let sid = Uuid::new_v4();
        FileTraceSink::new(&root)
            .emit(sample_event(sid, None))
            .await
            .unwrap();
        let reopened = FileTraceSink::new(&root);
        reopened.emit(sample_event(sid, None)).await.unwrap();
        assert_eq!(reopened.query(sid).await.unwrap().len(), 2);
        assert_eq!(FileTraceSink::new(&root).query(sid).await.unwrap().len(), 2);
    }
    #[tokio::test]
    async fn failed_write_is_not_reported_as_saved() {
        let root = unique_root();
        let sid = Uuid::new_v4();
        let sink = FileTraceSink::new(&root);
        sink.emit(sample_event(sid, None)).await.unwrap();
        let path = sink.trace_path(sid).unwrap();
        std::fs::create_dir(PathBuf::from(format!("{}.previous", path.display()))).unwrap();
        assert!(sink.emit(sample_event(sid, None)).await.is_err());
        assert_eq!(sink.query(sid).await.unwrap().len(), 1);
    }
    #[tokio::test]
    async fn legacy_corruption_and_version_are_explicit() {
        let root = unique_root();
        let sid = Uuid::new_v4();
        let sink = FileTraceSink::new(&root);
        let path = sink.trace_path(sid).unwrap();
        std::fs::write(&path, serde_json::to_vec(&sample_event(sid, None)).unwrap()).unwrap();
        sink.emit(sample_event(sid, None)).await.unwrap();
        assert_eq!(sink.query(sid).await.unwrap().len(), 2);
        std::fs::write(&path, b"broken\n").unwrap();
        assert!(sink
            .query(sid)
            .await
            .unwrap_err()
            .to_string()
            .contains("corrupt trace"));
        assert!(sink.emit(sample_event(sid, None)).await.is_err());
        std::fs::write(&path, b"{\"format_version\":99,\"data\":{}}\n").unwrap();
        assert!(sink
            .query(sid)
            .await
            .unwrap_err()
            .to_string()
            .contains("unsupported trace version"));
    }
    #[tokio::test]
    async fn export_redacts_free_text() {
        let root = unique_root();
        let sid = Uuid::new_v4();
        let sink = FileTraceSink::new(&root);
        let mut event = sample_event(sid, None);
        event.span = "SECRET_SPAN".into();
        event.message = "SECRET_MESSAGE".into();
        event.fields = serde_json::json!({"secret":"SECRET_VALUE"});
        sink.emit(event).await.unwrap();
        let text = sink.export_redacted(sid).await.unwrap().to_string();
        assert!(!text.contains("SECRET"));
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn records_are_private_and_symlinks_rejected() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = unique_root();
        let sid = Uuid::new_v4();
        let sink = FileTraceSink::new(&root);
        sink.emit(sample_event(sid, None)).await.unwrap();
        assert_eq!(
            std::fs::metadata(sink.trace_path(sid).unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let other = Uuid::new_v4();
        symlink(
            sink.trace_path(sid).unwrap(),
            sink.trace_path(other).unwrap(),
        )
        .unwrap();
        assert!(sink.emit(sample_event(other, None)).await.is_err());
        assert!(sink.query(other).await.is_err());
    }
    #[cfg(unix)]
    #[test]
    fn partial_write_worker() {
        let Ok(root) = std::env::var("CHIMERA_TRACE_IO_ROOT") else {
            return;
        };
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
        let id = Uuid::parse_str(&std::env::var("CHIMERA_TRACE_IO_ID").unwrap()).unwrap();
        let mut event = sample_event(id, None);
        event.message = "large output".repeat(10_000);
        assert!(FileTraceSink::new(root).emit_sync(event).is_err());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn real_partial_trace_write_preserves_saved_history_and_has_no_cache_success() {
        let root = unique_root();
        let sink = FileTraceSink::new(&root);
        let id = Uuid::new_v4();
        sink.emit(sample_event(id, None)).await.unwrap();
        let path = sink.trace_path(id).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(before.len() < 4096);
        assert!(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "file_tests::partial_write_worker"])
            .env("CHIMERA_TRACE_IO_ROOT", &root)
            .env("CHIMERA_TRACE_IO_ID", id.to_string())
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap()
            .success());
        assert_eq!(std::fs::read(path).unwrap(), before);
        assert_eq!(sink.query(id).await.unwrap().len(), 1);
        assert_eq!(FileTraceSink::new(&root).query(id).await.unwrap().len(), 1);
        sink.emit(sample_event(id, None)).await.unwrap();
        assert_eq!(sink.query(id).await.unwrap().len(), 2);
    }
}
