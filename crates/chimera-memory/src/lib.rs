use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// ID types
// ---------------------------------------------------------------------------

pub type MemoryId = Uuid;

// ---------------------------------------------------------------------------
// Memory layers
// ---------------------------------------------------------------------------

/// Which memory layer an item belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryLayer {
    /// Per-creature ephemeral working memory.
    Scratch,
    /// Current run summaries and constraints.
    Session,
    /// Past problems, actions, outcomes.
    Episodic,
    /// Concept graph of repo, systems, services, owners, risks.
    Semantic,
    /// Reusable skills and successful procedures.
    Pattern,
}

// ---------------------------------------------------------------------------
// Memory item
// ---------------------------------------------------------------------------

/// A stored memory item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryItem {
    /// Unique memory ID.
    pub id: MemoryId,
    /// Which layer this memory belongs to.
    pub layer: MemoryLayer,
    /// Human-readable label or key.
    pub key: String,
    /// The memory content.
    pub content: String,
    /// Structured metadata.
    pub metadata: serde_json::Value,
    /// When this memory was created.
    pub created_at: DateTime<Utc>,
    /// When this memory was last accessed.
    pub last_accessed: DateTime<Utc>,
    /// Optional session ID this memory is scoped to.
    pub session_id: Option<Uuid>,
    /// Optional creature ID this memory is scoped to.
    pub creature_id: Option<Uuid>,
    /// Relevance score (0.0 to 1.0, higher = more relevant).
    pub relevance: f64,
}

// ---------------------------------------------------------------------------
// Memory query
// ---------------------------------------------------------------------------

/// A query against the memory store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryQuery {
    /// Filter by layer.
    pub layer: Option<MemoryLayer>,
    /// Free-text search query.
    pub query: Option<String>,
    /// Filter by session ID.
    pub session_id: Option<Uuid>,
    /// Filter by creature ID.
    pub creature_id: Option<Uuid>,
    /// Maximum number of results.
    pub limit: usize,
    /// Minimum relevance threshold.
    pub min_relevance: Option<f64>,
}

impl Default for MemoryQuery {
    fn default() -> Self {
        Self {
            layer: None,
            query: None,
            session_id: None,
            creature_id: None,
            limit: 20,
            min_relevance: None,
        }
    }
}

// ---------------------------------------------------------------------------
// MemoryStore trait
// ---------------------------------------------------------------------------

/// Layered memory store: scratch, session, episodic, semantic, pattern.
#[async_trait::async_trait]
pub trait MemoryStore: Send + Sync {
    /// Store a memory item and return its ID.
    async fn remember(&self, item: MemoryItem) -> anyhow::Result<MemoryId>;

    /// Retrieve memory items matching a query.
    async fn retrieve(&self, query: MemoryQuery) -> anyhow::Result<Vec<MemoryItem>>;

    /// Delete a memory item by ID.
    async fn forget(&self, id: MemoryId) -> anyhow::Result<()>;

    /// Compact a memory layer (merge, summarize, prune old entries).
    async fn compact(&self, layer: MemoryLayer) -> anyhow::Result<u64>;
}
