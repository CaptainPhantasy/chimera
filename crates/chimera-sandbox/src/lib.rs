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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            cpu_cores: None,
            memory_mb: None,
            disk_mb: None,
            timeout_secs: None,
        }
    }
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
