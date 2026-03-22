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
