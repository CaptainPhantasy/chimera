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
    CreatureSpawned { creature_name: String, instance_id: String },
    /// A tool was called.
    ToolCalled { tool: String, args: serde_json::Value },
    /// An approval was requested.
    ApprovalRequested { action: String, class: String },
    /// An approval decision was made.
    ApprovalDecided { action: String, approved: bool },
    /// A checkpoint was created.
    Checkpointed { checkpoint_id: CheckpointId },
    /// The session was forked.
    Forked { parent_id: SessionId, child_id: SessionId },
    /// A custom event for extensibility.
    Custom { key: String, payload: serde_json::Value },
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

use std::path::PathBuf;
use tracing::warn;

/// A `SessionStore` that persists each session as a JSON file under
/// `<root>/.chimera/sessions/<id>.json`.
pub struct FileSessionStore {
    root: PathBuf,
}

impl FileSessionStore {
    /// Create a store rooted at the given workspace directory. Session files
    /// live under `<root>/.chimera/sessions/`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn sessions_dir(&self) -> PathBuf {
        self.root.join(".chimera").join("sessions")
    }

    fn session_path(&self, id: SessionId) -> PathBuf {
        self.sessions_dir().join(format!("{id}.json"))
    }

    fn write_state(&self, state: &SessionState) -> anyhow::Result<()> {
        let dir = self.sessions_dir();
        std::fs::create_dir_all(&dir)?;
        let path = self.session_path(state.id);
        let json = serde_json::to_string_pretty(state)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    fn read_state(&self, id: SessionId) -> anyhow::Result<SessionState> {
        let path = self.session_path(id);
        let raw = std::fs::read_to_string(&path)?;
        let state: SessionState = serde_json::from_str(&raw)?;
        Ok(state)
    }
    // The events vector of a fresh session is seeded in `create` directly.
}

#[async_trait::async_trait]
impl SessionStore for FileSessionStore {
    async fn create(&self, meta: SessionMeta) -> anyhow::Result<SessionId> {
        let id = Uuid::new_v4();
        let state = SessionState {
            id,
            meta,
            events: vec![],
            checkpoints: vec![],
            active: true,
        };
        // Seed with a Created event.
        let mut state = state;
        let created = SessionEvent::Created {
            meta: state.meta.clone(),
        };
        state.events.push(created);
        self.write_state(&state)?;
        Ok(id)
    }

    async fn append_event(&self, id: SessionId, event: SessionEvent) -> anyhow::Result<()> {
        let mut state = self.read_state(id)?;
        state.events.push(event);
        self.write_state(&state)
    }

    async fn snapshot(&self, id: SessionId) -> anyhow::Result<CheckpointId> {
        let mut state = self.read_state(id)?;
        let cp = Uuid::new_v4();
        state.checkpoints.push(cp);
        state.events.push(SessionEvent::Checkpointed { checkpoint_id: cp });
        self.write_state(&state)?;
        Ok(cp)
    }

    async fn fork(&self, id: SessionId) -> anyhow::Result<SessionId> {
        let parent = self.read_state(id)?;
        let child_id = Uuid::new_v4();
        let mut child = SessionState {
            id: child_id,
            meta: parent.meta.clone(),
            events: parent.events.clone(),
            checkpoints: parent.checkpoints.clone(),
            active: true,
        };
        child.events.push(SessionEvent::Forked {
            parent_id: id,
            child_id,
        });
        self.write_state(&child)?;
        Ok(child_id)
    }

    async fn load(&self, id: SessionId) -> anyhow::Result<SessionState> {
        self.read_state(id)
    }

    async fn list(&self) -> anyhow::Result<Vec<SessionId>> {
        let dir = self.sessions_dir();
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warn!(error = %e, "skipping unreadable dir entry");
                    continue;
                }
            };
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s,
                None => continue,
            };
            match Uuid::parse_str(stem) {
                Ok(id) => ids.push(id),
                Err(_) => continue,
            }
        }
        Ok(ids)
    }
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
}
