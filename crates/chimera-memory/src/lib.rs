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
        let mut kept: Vec<MemoryItem> = Vec::new();
        for item in items.drain(..) {
            if item.layer != layer || !kept.iter().any(|old| same_memory(old, &item)) {
                kept.push(item);
            }
        }
        *items = kept;
        Ok((before - items.len()) as u64)
    }
}

/// Private durable record storage shared by project-scoped memory and workflows.
/// Records carry their canonical project identity, preventing accidental reads
/// when a state folder is copied into an unrelated project.
pub mod storage {
    use anyhow::{bail, Context};
    use serde::{de::DeserializeOwned, Serialize};
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};
    use uuid::Uuid;
    const LIMIT: u64 = 32 * 1024 * 1024;

    pub struct ProjectRecords {
        root: PathBuf,
        namespace: String,
    }
    impl ProjectRecords {
        pub fn new(root: impl Into<PathBuf>, namespace: &str) -> Self {
            Self {
                root: root.into(),
                namespace: namespace.into(),
            }
        }
        pub fn root(&self) -> &Path {
            &self.root
        }
        fn directory(&self) -> anyhow::Result<PathBuf> {
            valid_name(&self.namespace)?;
            let root = self
                .root
                .canonicalize()
                .context("project folder is unavailable")?;
            let state = private_dir(&root.join(".chimera"))?;
            private_dir(&state.join(&self.namespace))
        }
        pub fn path(&self, name: &str) -> anyhow::Result<PathBuf> {
            valid_name(name)?;
            Ok(self.directory()?.join(format!("{name}.json")))
        }
        fn lock(&self) -> anyhow::Result<File> {
            let path = self.directory()?.join(".lock");
            regular(&path)?;
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
                    Err(std::fs::TryLockError::WouldBlock)
                        if start.elapsed() < Duration::from_secs(5) =>
                    {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(e) => bail!("project record lock unavailable: {e}"),
                }
            }
        }
        fn load<T: Default + DeserializeOwned>(&self, name: &str) -> anyhow::Result<T> {
            let path = self.path(name)?;
            regular(&path)?;
            if !path.exists() {
                return Ok(T::default());
            }
            let value: serde_json::Value = serde_json::from_slice(&read_bytes(&path)?)
                .with_context(|| {
                    format!(
                        "corrupt project record at {}; previous version is {}.previous",
                        path.display(),
                        path.display()
                    )
                })?;
            if value["format_version"].as_u64() != Some(1) {
                bail!("unsupported project record version");
            }
            if value["project_root"].as_str() != self.root.canonicalize()?.to_str() {
                bail!("project identity mismatch in {}", path.display());
            }
            serde_json::from_value(value["data"].clone()).context("invalid project record contents")
        }
        pub fn read<T: Default + DeserializeOwned>(&self, name: &str) -> anyhow::Result<T> {
            let _lock = self.lock()?;
            self.load(name)
        }
        pub fn update<T: Default + DeserializeOwned + Serialize, R>(
            &self,
            name: &str,
            update: impl FnOnce(&mut T) -> anyhow::Result<R>,
        ) -> anyhow::Result<R> {
            let _lock = self.lock()?;
            let mut data = self.load(name)?;
            let result = update(&mut data)?;
            let bytes = serde_json::to_vec(
                &serde_json::json!({"format_version":1,"project_root":self.root.canonicalize()?,"data":data}),
            )?;
            if bytes.len() as u64 > LIMIT {
                bail!("project record exceeds 32 MiB limit");
            }
            let path = self.path(name)?;
            if path.exists() {
                atomic_write(
                    &PathBuf::from(format!("{}.previous", path.display())),
                    &read_bytes(&path)?,
                )?;
            }
            atomic_write(&path, &bytes)?;
            Ok(result)
        }
    }
    fn valid_name(name: &str) -> anyhow::Result<()> {
        if name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            bail!("invalid record name");
        }
        Ok(())
    }
    fn private_dir(path: &Path) -> anyhow::Result<PathBuf> {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => (),
            Ok(_) => bail!("unsafe project state directory: {}", path.display()),
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
                    bail!("unsafe project state directory");
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
    fn regular(path: &Path) -> anyhow::Result<()> {
        match std::fs::symlink_metadata(path) {
            Ok(m) if !m.is_file() || m.file_type().is_symlink() => {
                bail!("unsafe project state file: {}", path.display())
            }
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
    fn read_bytes(path: &Path) -> anyhow::Result<Vec<u8>> {
        regular(path)?;
        let mut bytes = Vec::new();
        File::open(path)?.take(LIMIT + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > LIMIT {
            bail!("project record exceeds 32 MiB limit");
        }
        Ok(bytes)
    }
    fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
        regular(path)?;
        let parent = path.parent().context("record parent missing")?;
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
}

#[derive(Default, Serialize, Deserialize)]
struct MemoryRecords {
    items: Vec<MemoryItem>,
    archived: Vec<ArchivedMemory>,
}
#[derive(Serialize, Deserialize)]
struct ArchivedMemory {
    item: MemoryItem,
    reason: String,
    at: DateTime<Utc>,
}

/// Durable memory confined to a canonical project. A session view can see its
/// own records plus shared project records; it cannot see other sessions.
pub struct FileMemoryStore {
    records: storage::ProjectRecords,
    session: Option<Uuid>,
}
impl FileMemoryStore {
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            records: storage::ProjectRecords::new(root, "memory"),
            session: None,
        }
    }
    pub fn for_session(root: impl Into<std::path::PathBuf>, session: Uuid) -> Self {
        Self {
            records: storage::ProjectRecords::new(root, "memory"),
            session: Some(session),
        }
    }
    pub async fn show(&self, id: MemoryId) -> anyhow::Result<MemoryItem> {
        let records: MemoryRecords = self.records.read("items")?;
        records
            .items
            .into_iter()
            .find(|item| {
                item.id == id && (item.session_id.is_none() || item.session_id == self.session)
            })
            .ok_or_else(|| anyhow::anyhow!("memory not found in current scope: {id}"))
    }
}
fn validate_item(item: &MemoryItem) -> anyhow::Result<()> {
    if item.key.trim().is_empty() || item.key.len() > 256 || item.content.len() > 1024 * 1024 {
        anyhow::bail!("memory key must be nonempty and <=256 bytes; content must be <=1 MiB");
    }
    if !item.relevance.is_finite() || !(0.0..=1.0).contains(&item.relevance) {
        anyhow::bail!("memory relevance must be between 0 and 1");
    }
    Ok(())
}
fn same_memory(a: &MemoryItem, b: &MemoryItem) -> bool {
    a.layer == b.layer
        && a.key == b.key
        && a.content == b.content
        && a.metadata == b.metadata
        && a.session_id == b.session_id
        && a.creature_id == b.creature_id
}

#[async_trait::async_trait]
impl MemoryStore for FileMemoryStore {
    async fn remember(&self, mut item: MemoryItem) -> anyhow::Result<MemoryId> {
        validate_item(&item)?;
        if let Some(session) = self.session {
            if item.session_id.is_some_and(|id| id != session) {
                anyhow::bail!("memory belongs to another session");
            }
            item.session_id = Some(session);
        }
        self.records.update("items", |records: &mut MemoryRecords| {
            if let Some(index) = records.items.iter().position(|old| old.id == item.id) {
                if records.items[index].session_id != item.session_id {
                    anyhow::bail!("cannot move a memory id between session scopes");
                }
                let previous = records.items.remove(index);
                records.archived.push(ArchivedMemory {
                    item: previous,
                    reason: "updated".into(),
                    at: Utc::now(),
                });
            }
            let id = item.id;
            records.items.push(item);
            Ok(id)
        })
    }
    async fn retrieve(&self, query: MemoryQuery) -> anyhow::Result<Vec<MemoryItem>> {
        if query.limit > 1000
            || query
                .min_relevance
                .is_some_and(|r| !r.is_finite() || !(0.0..=1.0).contains(&r))
        {
            anyhow::bail!("invalid memory query limits");
        }
        if self.session.is_some() && query.session_id.is_some() && query.session_id != self.session
        {
            anyhow::bail!("cannot query another session");
        }
        let scope = query.session_id.or(self.session);
        let records: MemoryRecords = self.records.read("items")?;
        let mut result: Vec<_> = records
            .items
            .into_iter()
            .filter(|item| {
                (item.session_id.is_none() || item.session_id == scope)
                    && query.layer.is_none_or(|layer| item.layer == layer)
                    && query
                        .creature_id
                        .is_none_or(|id| item.creature_id == Some(id))
                    && query.min_relevance.is_none_or(|min| item.relevance >= min)
                    && query
                        .query
                        .as_ref()
                        .is_none_or(|q| item.content.contains(q) || item.key.contains(q))
            })
            .collect();
        result.sort_by(|a, b| {
            b.relevance
                .total_cmp(&a.relevance)
                .then_with(|| b.last_accessed.cmp(&a.last_accessed))
                .then_with(|| a.id.cmp(&b.id))
        });
        result.truncate(query.limit);
        Ok(result)
    }
    async fn forget(&self, id: MemoryId) -> anyhow::Result<()> {
        self.records.update("items", |records: &mut MemoryRecords| {
            let index = records
                .items
                .iter()
                .position(|i| i.id == id && i.session_id == self.session)
                .ok_or_else(|| anyhow::anyhow!("memory not found in current scope"))?;
            let item = records.items.remove(index);
            records.archived.push(ArchivedMemory {
                item,
                reason: "forgotten by operator".into(),
                at: Utc::now(),
            });
            Ok(())
        })
    }
    async fn compact(&self, layer: MemoryLayer) -> anyhow::Result<u64> {
        self.records.update("items", |records: &mut MemoryRecords| {
            let mut kept: Vec<MemoryItem> = Vec::new();
            let mut count = 0;
            for item in records.items.drain(..) {
                let candidate = if item.layer == layer && item.session_id == self.session {
                    kept.iter_mut().find(|old| same_memory(old, &item))
                } else {
                    None
                };
                if let Some(old) = candidate {
                    old.created_at = old.created_at.min(item.created_at);
                    old.last_accessed = old.last_accessed.max(item.last_accessed);
                    old.relevance = old.relevance.max(item.relevance);
                    records.archived.push(ArchivedMemory {
                        item,
                        reason: "exact duplicate compacted".into(),
                        at: Utc::now(),
                    });
                    count += 1;
                } else {
                    kept.push(item);
                }
            }
            records.items = kept;
            Ok(count)
        })
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
        store
            .remember(make_item("a", MemoryLayer::Scratch, 0.5))
            .await
            .unwrap();
        store
            .remember(make_item("b", MemoryLayer::Episodic, 0.8))
            .await
            .unwrap();
        store
            .remember(make_item("c", MemoryLayer::Scratch, 0.6))
            .await
            .unwrap();

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
        store
            .remember(make_item("low", MemoryLayer::Episodic, 0.3))
            .await
            .unwrap();
        store
            .remember(make_item("high", MemoryLayer::Episodic, 0.9))
            .await
            .unwrap();

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
        store
            .remember(make_item("auth", MemoryLayer::Episodic, 0.8))
            .await
            .unwrap();
        store
            .remember(make_item("db-migration", MemoryLayer::Episodic, 0.7))
            .await
            .unwrap();

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
        let id = store
            .remember(make_item("temp", MemoryLayer::Scratch, 0.5))
            .await
            .unwrap();

        store.forget(id).await.unwrap();

        let results = store.retrieve(MemoryQuery::default()).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn compact_preserves_distinct_memories() {
        let store = InMemoryStore::new();
        store
            .remember(make_item("a", MemoryLayer::Scratch, 0.5))
            .await
            .unwrap();
        store
            .remember(make_item("b", MemoryLayer::Scratch, 0.6))
            .await
            .unwrap();
        store
            .remember(make_item("c", MemoryLayer::Episodic, 0.9))
            .await
            .unwrap();

        let removed = store.compact(MemoryLayer::Scratch).await.unwrap();
        assert_eq!(removed, 0);

        let results = store.retrieve(MemoryQuery::default()).await.unwrap();
        assert_eq!(results.len(), 3);
        assert!(results.iter().any(|item| item.key == "c"));
    }

    #[tokio::test]
    async fn retrieve_filters_by_session_id() {
        let store = InMemoryStore::new();
        let sid = Uuid::new_v4();
        let mut item_a = make_item("a", MemoryLayer::Session, 0.8);
        item_a.session_id = Some(sid);
        store.remember(item_a).await.unwrap();
        store
            .remember(make_item("b", MemoryLayer::Session, 0.8))
            .await
            .unwrap();

        let results = store
            .retrieve(MemoryQuery {
                session_id: Some(sid),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "a");
    }

    #[tokio::test]
    async fn retrieve_filters_by_creature_id() {
        let store = InMemoryStore::new();
        let cid = Uuid::new_v4();
        let mut item_a = make_item("a", MemoryLayer::Scratch, 0.8);
        item_a.creature_id = Some(cid);
        store.remember(item_a).await.unwrap();
        store
            .remember(make_item("b", MemoryLayer::Scratch, 0.8))
            .await
            .unwrap();

        let results = store
            .retrieve(MemoryQuery {
                creature_id: Some(cid),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "a");
    }

    #[test]
    fn in_memory_store_default() {
        let store = InMemoryStore::default();
        // Just verify it constructs without panic
        let _ = store;
    }
    fn root() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("chimera-memory-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }
    #[tokio::test]
    async fn durable_restart_compaction_and_archival_preserve_content() {
        let root = root();
        let store = FileMemoryStore::new(&root);
        let a = make_item("one", MemoryLayer::Pattern, 0.3);
        let a_id = a.id;
        store.remember(a.clone()).await.unwrap();
        let mut duplicate = a.clone();
        duplicate.id = Uuid::new_v4();
        duplicate.relevance = 0.9;
        store.remember(duplicate).await.unwrap();
        store
            .remember(make_item("different", MemoryLayer::Pattern, 0.8))
            .await
            .unwrap();
        assert_eq!(store.compact(MemoryLayer::Pattern).await.unwrap(), 1);
        let reopened = FileMemoryStore::new(&root);
        let items = reopened.retrieve(MemoryQuery::default()).await.unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].relevance, 0.9);
        assert_eq!(items[0].id, a_id);
        reopened.forget(a_id).await.unwrap();
        assert!(reopened.show(a_id).await.is_err());
        let stored: MemoryRecords = reopened.records.read("items").unwrap();
        assert_eq!(stored.archived.len(), 2);
        assert!(stored
            .archived
            .iter()
            .any(|entry| entry.item.content == a.content));
    }
    #[tokio::test]
    async fn durable_session_and_project_isolation() {
        let first = root();
        let other = root();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let sa = FileMemoryStore::for_session(&first, a);
        let sb = FileMemoryStore::for_session(&first, b);
        let id = sa
            .remember(make_item("private", MemoryLayer::Session, 0.9))
            .await
            .unwrap();
        assert_eq!(sa.retrieve(MemoryQuery::default()).await.unwrap().len(), 1);
        assert!(sb
            .retrieve(MemoryQuery::default())
            .await
            .unwrap()
            .is_empty());
        assert!(sb.show(id).await.is_err());
        assert!(sb
            .retrieve(MemoryQuery {
                session_id: Some(a),
                ..Default::default()
            })
            .await
            .is_err());
        assert!(FileMemoryStore::new(&other)
            .retrieve(MemoryQuery::default())
            .await
            .unwrap()
            .is_empty());
        let target = FileMemoryStore::new(&other);
        let destination = target.records.path("items").unwrap();
        std::fs::copy(sa.records.path("items").unwrap(), destination).unwrap();
        assert!(target
            .retrieve(MemoryQuery::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("project identity mismatch"));
    }
    #[tokio::test]
    async fn durable_invalid_values_and_corruption_do_not_succeed() {
        let root = root();
        let store = FileMemoryStore::new(&root);
        let mut bad = make_item("bad", MemoryLayer::Scratch, 2.0);
        assert!(store.remember(bad.clone()).await.is_err());
        bad.relevance = 0.5;
        store.remember(bad).await.unwrap();
        std::fs::write(store.records.path("items").unwrap(), b"corrupt").unwrap();
        assert!(store.retrieve(MemoryQuery::default()).await.is_err());
        assert!(store.compact(MemoryLayer::Scratch).await.is_err());
    }
    #[tokio::test]
    async fn independent_stores_preserve_overlapping_writes() {
        let root = root();
        let mut workers = Vec::new();
        for worker in 0..4 {
            let dir = root.clone();
            workers.push(std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(async move {
                    let store = FileMemoryStore::new(dir);
                    for i in 0..8 {
                        store
                            .remember(make_item(
                                &format!("{worker}-{i}"),
                                MemoryLayer::Pattern,
                                0.8,
                            ))
                            .await
                            .unwrap();
                    }
                });
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(
            FileMemoryStore::new(root)
                .retrieve(MemoryQuery {
                    limit: 100,
                    ..Default::default()
                })
                .await
                .unwrap()
                .len(),
            32
        );
    }
    #[tokio::test]
    async fn session_view_can_read_shared_memory_but_cannot_change_its_scope() {
        let root = root();
        let global = FileMemoryStore::new(&root);
        let id = global
            .remember(make_item("shared", MemoryLayer::Pattern, 0.7))
            .await
            .unwrap();
        let scoped = FileMemoryStore::for_session(&root, Uuid::new_v4());
        assert!(scoped.show(id).await.is_ok());
        assert!(scoped.forget(id).await.is_err());
        let mut replacement = make_item("changed", MemoryLayer::Pattern, 0.7);
        replacement.id = id;
        assert!(scoped.remember(replacement).await.is_err());
        assert_eq!(global.show(id).await.unwrap().key, "shared");
    }
    #[cfg(unix)]
    #[test]
    fn record_write_limit_worker() {
        let Ok(root) = std::env::var("CHIMERA_RECORD_IO_ROOT") else {
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
        let records = storage::ProjectRecords::new(root, "io-test");
        let result = records.update("record", |rows: &mut Vec<String>| {
            rows.push("new value".repeat(10_000));
            Ok(())
        });
        assert!(
            result.is_err(),
            "partial file write must not report a saved update"
        );
    }
    #[cfg(unix)]
    #[test]
    fn real_partial_write_error_preserves_project_records_and_previous_version() {
        let root = root();
        let records = storage::ProjectRecords::new(&root, "io-test");
        records
            .update("record", |rows: &mut Vec<String>| {
                rows.push("committed value".into());
                Ok(())
            })
            .unwrap();
        let path = records.path("record").unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(before.len() < 4096);
        assert!(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::record_write_limit_worker"])
            .env("CHIMERA_RECORD_IO_ROOT", &root)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap()
            .success());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            std::fs::read(std::path::PathBuf::from(format!(
                "{}.previous",
                path.display()
            )))
            .unwrap(),
            before
        );
        assert_eq!(
            records.read::<Vec<String>>("record").unwrap(),
            vec!["committed value"]
        );
        assert!(std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with(".pending-")
                && e.metadata().unwrap().len() == 4096));
        records
            .update("record", |rows: &mut Vec<String>| {
                rows.push("after failed write".into());
                Ok(())
            })
            .unwrap();
        assert_eq!(
            records.read::<Vec<String>>("record").unwrap(),
            vec!["committed value", "after failed write"]
        );
    }
}
