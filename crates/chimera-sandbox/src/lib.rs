use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// ID types
// ---------------------------------------------------------------------------

/// Unique world instance identifier.
pub type WorldId = Uuid;

// ---------------------------------------------------------------------------
// World kind
// ---------------------------------------------------------------------------

/// The type of execution environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldKind {
    LocalShell,
    WorktreeReadOnly,
    WorktreeReadWrite,
    Container,
    MicroVm,
    BrowserSandbox,
    MobileEmulator,
    RemoteRunner,
}

// ---------------------------------------------------------------------------
// World specification
// ---------------------------------------------------------------------------

/// Specification for allocating a new world.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldSpec {
    /// What kind of world to create.
    pub kind: WorldKind,
    /// Optional base image or template name (for container/VM worlds).
    pub image: Option<String>,
    /// Working directory or mount point.
    pub root: Option<PathBuf>,
    /// Environment variable overrides.
    pub env: HashMap<String, String>,
    /// Whether network access is allowed.
    pub network: bool,
    /// Whether the filesystem is writable.
    pub writable: bool,
    /// Optional label for display.
    pub label: Option<String>,
    /// Resource limits.
    pub limits: ResourceLimits,
}

/// Resource constraints for a world.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Max CPU cores (fractional).
    pub cpu_cores: Option<f64>,
    /// Max memory in MB.
    pub memory_mb: Option<u64>,
    /// Max disk in MB.
    pub disk_mb: Option<u64>,
    /// Max wall-clock lifetime in seconds.
    pub timeout_secs: Option<u64>,
}

// ---------------------------------------------------------------------------
// World state
// ---------------------------------------------------------------------------

/// Runtime state of an allocated world.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldState {
    /// World instance ID.
    pub id: WorldId,
    /// The spec used to create this world.
    pub spec: WorldSpec,
    /// Current lifecycle status.
    pub status: WorldStatus,
    /// When the world was created.
    pub created_at: DateTime<Utc>,
    /// Filesystem root path (if applicable).
    pub root_path: Option<PathBuf>,
}

/// Lifecycle status of a world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldStatus {
    /// World is being provisioned.
    Provisioning,
    /// World is running and available.
    Running,
    /// World is paused/suspended.
    Paused,
    /// World is being torn down.
    Destroying,
    /// World has been destroyed.
    Destroyed,
    /// World encountered an error.
    Failed,
}

// ---------------------------------------------------------------------------
// WorldManager trait
// ---------------------------------------------------------------------------

/// Manages execution world lifecycle: allocate, inspect, pause, resume, destroy.
#[async_trait::async_trait]
pub trait WorldManager: Send + Sync {
    /// Allocate a new world from a spec.
    async fn allocate(&self, spec: WorldSpec) -> anyhow::Result<WorldId>;

    /// Inspect the current state of a world.
    async fn inspect(&self, id: WorldId) -> anyhow::Result<WorldState>;

    /// Pause a running world (if supported by the backend).
    async fn pause(&self, id: WorldId) -> anyhow::Result<()>;

    /// Resume a paused world.
    async fn resume(&self, id: WorldId) -> anyhow::Result<()>;

    /// Destroy a world and free its resources.
    async fn destroy(&self, id: WorldId) -> anyhow::Result<()>;

    /// List all active world IDs.
    async fn list(&self) -> anyhow::Result<Vec<WorldId>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_kind_serialization() {
        let kinds = [
            WorldKind::LocalShell, WorldKind::WorktreeReadOnly, WorldKind::WorktreeReadWrite,
            WorldKind::Container, WorldKind::MicroVm, WorldKind::BrowserSandbox,
            WorldKind::MobileEmulator, WorldKind::RemoteRunner,
        ];
        for k in &kinds {
            let json = serde_json::to_string(k).unwrap();
            let _: WorldKind = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn world_status_serialization() {
        let statuses = [
            WorldStatus::Provisioning, WorldStatus::Running, WorldStatus::Paused,
            WorldStatus::Destroying, WorldStatus::Destroyed, WorldStatus::Failed,
        ];
        for s in &statuses {
            let json = serde_json::to_string(s).unwrap();
            let _: WorldStatus = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn resource_limits_default() {
        let limits = ResourceLimits::default();
        assert!(limits.cpu_cores.is_none());
        assert!(limits.memory_mb.is_none());
        assert!(limits.disk_mb.is_none());
        assert!(limits.timeout_secs.is_none());
    }

    #[test]
    fn world_spec_serialization() {
        let spec = WorldSpec {
            kind: WorldKind::Container,
            image: Some("alpine:latest".into()),
            root: Some(std::path::PathBuf::from("/tmp")),
            env: [("FOO".to_string(), "bar".to_string())].into_iter().collect(),
            network: true,
            writable: true,
            label: Some("test-world".into()),
            limits: ResourceLimits {
                cpu_cores: Some(2.0),
                memory_mb: Some(512),
                disk_mb: Some(1024),
                timeout_secs: Some(3600),
            },
        };
        let json = serde_json::to_string(&spec).unwrap();
        let deserialized: WorldSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.kind, WorldKind::Container);
        assert_eq!(deserialized.image, Some("alpine:latest".into()));
        assert!(deserialized.network);
    }

    #[test]
    fn world_state_serialization() {
        let state = WorldState {
            id: Uuid::new_v4(),
            spec: WorldSpec {
                kind: WorldKind::LocalShell,
                image: None,
                root: None,
                env: Default::default(),
                network: false,
                writable: false,
                label: None,
                limits: Default::default(),
            },
            status: WorldStatus::Running,
            created_at: Utc::now(),
            root_path: None,
        };
        let json = serde_json::to_string(&state).unwrap();
        let _: WorldState = serde_json::from_str(&json).unwrap();
    }
}
