use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// ID types
// ---------------------------------------------------------------------------

pub type WorkflowId = Uuid;

// ---------------------------------------------------------------------------
// Schedule frequency
// ---------------------------------------------------------------------------

/// How often a scheduled workflow runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduleFrequency {
    /// Run once at a specific time.
    Once { at: DateTime<Utc> },
    /// Run on an interval in seconds.
    Interval { every_secs: u64 },
    /// Cron expression.
    Cron { expression: String },
    /// Watch mode — triggered by events, not time.
    Watch { pattern: String },
}

// ---------------------------------------------------------------------------
// Schedule spec
// ---------------------------------------------------------------------------

/// Specification for a scheduled or watched workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleSpec {
    /// Human-readable label.
    pub label: String,
    /// The objective to execute on each trigger.
    pub objective: String,
    /// How often to run.
    pub frequency: ScheduleFrequency,
    /// Optional pack name to use.
    pub pack: Option<String>,
    /// Whether the workflow is currently enabled.
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// Workflow handle
// ---------------------------------------------------------------------------

/// Runtime state of a scheduled workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowHandle {
    /// Unique workflow ID.
    pub id: WorkflowId,
    /// The spec that created this workflow.
    pub spec: ScheduleSpec,
    /// Current status.
    pub status: WorkflowStatus,
    /// When the workflow was created.
    pub created_at: DateTime<Utc>,
    /// When the workflow last ran.
    pub last_run: Option<DateTime<Utc>>,
    /// How many times it has run.
    pub run_count: u64,
}

/// Status of a scheduled workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    Active,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

// ---------------------------------------------------------------------------
// WorkflowScheduler trait
// ---------------------------------------------------------------------------

/// Manages scheduled and watched workflows.
#[async_trait::async_trait]
pub trait WorkflowScheduler: Send + Sync {
    /// Create a new scheduled workflow.
    async fn create(&self, spec: ScheduleSpec) -> anyhow::Result<WorkflowId>;

    /// Get the handle for a workflow.
    async fn get(&self, id: WorkflowId) -> anyhow::Result<WorkflowHandle>;

    /// List all workflow IDs.
    async fn list(&self) -> anyhow::Result<Vec<WorkflowId>>;

    /// Pause a workflow.
    async fn pause(&self, id: WorkflowId) -> anyhow::Result<()>;

    /// Resume a paused workflow.
    async fn resume(&self, id: WorkflowId) -> anyhow::Result<()>;

    /// Cancel a workflow.
    async fn cancel(&self, id: WorkflowId) -> anyhow::Result<()>;
}

// ---------------------------------------------------------------------------
// In-memory mock
// ---------------------------------------------------------------------------

/// In-memory WorkflowScheduler for testing.
pub struct MockScheduler {
    workflows: tokio::sync::RwLock<std::collections::HashMap<WorkflowId, WorkflowHandle>>,
}

impl MockScheduler {
    pub fn new() -> Self {
        Self {
            workflows: tokio::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }
}

impl Default for MockScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl WorkflowScheduler for MockScheduler {
    async fn create(&self, spec: ScheduleSpec) -> anyhow::Result<WorkflowId> {
        let id = Uuid::new_v4();
        let handle = WorkflowHandle {
            id,
            spec,
            status: WorkflowStatus::Active,
            created_at: Utc::now(),
            last_run: None,
            run_count: 0,
        };
        self.workflows.write().await.insert(id, handle);
        Ok(id)
    }

    async fn get(&self, id: WorkflowId) -> anyhow::Result<WorkflowHandle> {
        self.workflows
            .read()
            .await
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("workflow not found: {}", id))
    }

    async fn list(&self) -> anyhow::Result<Vec<WorkflowId>> {
        Ok(self.workflows.read().await.keys().copied().collect())
    }

    async fn pause(&self, id: WorkflowId) -> anyhow::Result<()> {
        let mut workflows = self.workflows.write().await;
        let handle = workflows
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("workflow not found: {}", id))?;
        handle.status = WorkflowStatus::Paused;
        Ok(())
    }

    async fn resume(&self, id: WorkflowId) -> anyhow::Result<()> {
        let mut workflows = self.workflows.write().await;
        let handle = workflows
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("workflow not found: {}", id))?;
        handle.status = WorkflowStatus::Active;
        Ok(())
    }

    async fn cancel(&self, id: WorkflowId) -> anyhow::Result<()> {
        let mut workflows = self.workflows.write().await;
        let handle = workflows
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("workflow not found: {}", id))?;
        handle.status = WorkflowStatus::Cancelled;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn daily_spec(label: &str) -> ScheduleSpec {
        ScheduleSpec {
            label: label.into(),
            objective: format!("{} objective", label),
            frequency: ScheduleFrequency::Interval { every_secs: 86400 },
            pack: None,
            enabled: true,
        }
    }

    #[tokio::test]
    async fn create_and_get() {
        let sched = MockScheduler::new();
        let id = sched.create(daily_spec("dep-check")).await.unwrap();

        let handle = sched.get(id).await.unwrap();
        assert_eq!(handle.spec.label, "dep-check");
        assert_eq!(handle.status, WorkflowStatus::Active);
        assert_eq!(handle.run_count, 0);
        assert!(handle.last_run.is_none());
    }

    #[tokio::test]
    async fn list_workflows() {
        let sched = MockScheduler::new();
        let id1 = sched.create(daily_spec("a")).await.unwrap();
        let id2 = sched.create(daily_spec("b")).await.unwrap();

        let ids = sched.list().await.unwrap();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&id1));
        assert!(ids.contains(&id2));
    }

    #[tokio::test]
    async fn pause_and_resume() {
        let sched = MockScheduler::new();
        let id = sched.create(daily_spec("test")).await.unwrap();

        sched.pause(id).await.unwrap();
        assert_eq!(sched.get(id).await.unwrap().status, WorkflowStatus::Paused);

        sched.resume(id).await.unwrap();
        assert_eq!(sched.get(id).await.unwrap().status, WorkflowStatus::Active);
    }

    #[tokio::test]
    async fn cancel_workflow() {
        let sched = MockScheduler::new();
        let id = sched.create(daily_spec("to-cancel")).await.unwrap();

        sched.cancel(id).await.unwrap();
        assert_eq!(sched.get(id).await.unwrap().status, WorkflowStatus::Cancelled);
    }

    #[tokio::test]
    async fn get_unknown_fails() {
        let sched = MockScheduler::new();
        assert!(sched.get(Uuid::new_v4()).await.is_err());
    }

    #[tokio::test]
    async fn watch_frequency_serializes() {
        let spec = ScheduleSpec {
            label: "log-watcher".into(),
            objective: "monitor prod logs".into(),
            frequency: ScheduleFrequency::Watch { pattern: "*.log".into() },
            pack: Some("incident-response".into()),
            enabled: true,
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("watch"));
        assert!(json.contains("*.log"));
    }
}
