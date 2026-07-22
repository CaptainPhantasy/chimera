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

use std::collections::HashMap;
use std::path::PathBuf;

/// A `TraceSink` that writes trace events as JSONL files under
/// `<root>/.chimera/traces/<session_id>.jsonl` and keeps an in-memory mirror
/// for fast queries.
pub struct FileTraceSink {
    root: PathBuf,
    buffer: tokio::sync::RwLock<HashMap<SessionId, Vec<TraceEvent>>>,
}

impl FileTraceSink {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            buffer: tokio::sync::RwLock::new(HashMap::new()),
        }
    }

    fn traces_dir(&self) -> PathBuf {
        self.root.join(".chimera").join("traces")
    }

    fn trace_path(&self, session_id: SessionId) -> PathBuf {
        self.traces_dir().join(format!("{session_id}.jsonl"))
    }

    async fn load_from_disk(&self, session_id: SessionId) -> anyhow::Result<Vec<TraceEvent>> {
        let path = self.trace_path(session_id);
        if !path.exists() {
            return Ok(vec![]);
        }
        let raw = std::fs::read_to_string(&path)?;
        let mut events = Vec::new();
        for line in raw.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<TraceEvent>(line) {
                Ok(e) => events.push(e),
                Err(e) => {
                    tracing::warn!(error = %e, "skipping malformed trace line");
                }
            }
        }
        Ok(events)
    }
}

#[async_trait::async_trait]
impl TraceSink for FileTraceSink {
    async fn emit(&self, event: TraceEvent) -> anyhow::Result<()> {
        // Append to in-memory buffer.
        {
            let mut buf = self.buffer.write().await;
            buf.entry(event.session_id)
                .or_default()
                .push(event.clone());
        }

        // Append a line to the JSONL file.
        let dir = self.traces_dir();
        std::fs::create_dir_all(&dir)?;
        let path = self.trace_path(event.session_id);
        let line = serde_json::to_string(&event)? + "\n";

        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        file.write_all(line.as_bytes())?;

        Ok(())
    }

    async fn query(&self, session_id: SessionId) -> anyhow::Result<Vec<TraceEvent>> {
        {
            let buf = self.buffer.read().await;
            if let Some(events) = buf.get(&session_id) {
                if !events.is_empty() {
                    return Ok(events.clone());
                }
            }
        }
        self.load_from_disk(session_id).await
    }

    async fn query_creature(
        &self,
        session_id: SessionId,
        creature_id: CreatureId,
    ) -> anyhow::Result<Vec<TraceEvent>> {
        let events = self.query(session_id).await?;
        Ok(events
            .into_iter()
            .filter(|e| e.creature_id == Some(creature_id))
            .collect())
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
}
