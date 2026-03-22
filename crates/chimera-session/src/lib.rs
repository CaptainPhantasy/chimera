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
