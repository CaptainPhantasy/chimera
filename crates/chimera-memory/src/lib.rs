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

// ---------------------------------------------------------------------------
// In-memory mock implementation
// ---------------------------------------------------------------------------

/// In-memory implementation of MemoryStore for testing.
pub struct InMemoryStore {
    items: tokio::sync::RwLock<Vec<MemoryItem>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            items: tokio::sync::RwLock::new(Vec::new()),
        }
    }
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl MemoryStore for InMemoryStore {
    async fn remember(&self, item: MemoryItem) -> anyhow::Result<MemoryId> {
        let id = item.id;
        self.items.write().await.push(item);
        Ok(id)
    }

    async fn retrieve(&self, query: MemoryQuery) -> anyhow::Result<Vec<MemoryItem>> {
        let items = self.items.read().await;
        let mut results: Vec<MemoryItem> = items
            .iter()
            .filter(|item| {
                if let Some(layer) = query.layer {
                    if item.layer != layer {
                        return false;
                    }
                }
                if let Some(ref sid) = query.session_id {
                    if item.session_id.as_ref() != Some(sid) {
                        return false;
                    }
                }
                if let Some(ref cid) = query.creature_id {
                    if item.creature_id.as_ref() != Some(cid) {
                        return false;
                    }
                }
                if let Some(min_rel) = query.min_relevance {
                    if item.relevance < min_rel {
                        return false;
                    }
                }
                if let Some(ref q) = query.query {
                    if !item.content.contains(q.as_str()) && !item.key.contains(q.as_str()) {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();
        results.truncate(query.limit);
        Ok(results)
    }

    async fn forget(&self, id: MemoryId) -> anyhow::Result<()> {
        self.items.write().await.retain(|item| item.id != id);
        Ok(())
    }

    async fn compact(&self, layer: MemoryLayer) -> anyhow::Result<u64> {
        let mut items = self.items.write().await;
        let before = items.len();
        items.retain(|item| item.layer != layer);
        Ok((before - items.len()) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_item(key: &str, layer: MemoryLayer, relevance: f64) -> MemoryItem {
        let now = Utc::now();
        MemoryItem {
            id: Uuid::new_v4(),
            layer,
            key: key.into(),
            content: format!("content for {}", key),
            metadata: serde_json::json!({}),
            created_at: now,
            last_accessed: now,
            session_id: None,
            creature_id: None,
            relevance,
        }
    }

    #[tokio::test]
    async fn remember_and_retrieve() {
        let store = InMemoryStore::new();
        let item = make_item("auth-pattern", MemoryLayer::Episodic, 0.9);
        let id = store.remember(item).await.unwrap();

        let results = store
            .retrieve(MemoryQuery {
                layer: Some(MemoryLayer::Episodic),
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, id);
        assert_eq!(results[0].key, "auth-pattern");
    }

    #[tokio::test]
    async fn retrieve_filters_by_layer() {
        let store = InMemoryStore::new();
        store.remember(make_item("a", MemoryLayer::Scratch, 0.5)).await.unwrap();
        store.remember(make_item("b", MemoryLayer::Episodic, 0.8)).await.unwrap();
        store.remember(make_item("c", MemoryLayer::Scratch, 0.6)).await.unwrap();

        let results = store
            .retrieve(MemoryQuery {
                layer: Some(MemoryLayer::Scratch),
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn retrieve_filters_by_relevance() {
        let store = InMemoryStore::new();
        store.remember(make_item("low", MemoryLayer::Episodic, 0.3)).await.unwrap();
        store.remember(make_item("high", MemoryLayer::Episodic, 0.9)).await.unwrap();

        let results = store
            .retrieve(MemoryQuery {
                min_relevance: Some(0.5),
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "high");
    }

    #[tokio::test]
    async fn retrieve_text_search() {
        let store = InMemoryStore::new();
        store.remember(make_item("auth", MemoryLayer::Episodic, 0.8)).await.unwrap();
        store.remember(make_item("db-migration", MemoryLayer::Episodic, 0.7)).await.unwrap();

        let results = store
            .retrieve(MemoryQuery {
                query: Some("auth".into()),
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "auth");
    }

    #[tokio::test]
    async fn forget_removes_item() {
        let store = InMemoryStore::new();
        let id = store.remember(make_item("temp", MemoryLayer::Scratch, 0.5)).await.unwrap();

        store.forget(id).await.unwrap();

        let results = store.retrieve(MemoryQuery::default()).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn compact_removes_layer() {
        let store = InMemoryStore::new();
        store.remember(make_item("a", MemoryLayer::Scratch, 0.5)).await.unwrap();
        store.remember(make_item("b", MemoryLayer::Scratch, 0.6)).await.unwrap();
        store.remember(make_item("c", MemoryLayer::Episodic, 0.9)).await.unwrap();

        let removed = store.compact(MemoryLayer::Scratch).await.unwrap();
        assert_eq!(removed, 2);

        let results = store.retrieve(MemoryQuery::default()).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "c");
    }
}
