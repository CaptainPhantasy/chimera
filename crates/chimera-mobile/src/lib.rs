use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// ID types
// ---------------------------------------------------------------------------

pub type DeviceId = Uuid;

// ---------------------------------------------------------------------------
// Device specification
// ---------------------------------------------------------------------------

/// Specification for booting a mobile emulator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceSpec {
    /// Device profile name (e.g. "pixel8", "iphone15").
    pub profile: String,
    /// Android API level or iOS version.
    pub os_version: Option<String>,
    /// Whether to enable network access.
    pub network: bool,
    /// Optional label.
    pub label: Option<String>,
}

// ---------------------------------------------------------------------------
// Device actions
// ---------------------------------------------------------------------------

/// An action to perform on a mobile device.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum DeviceAction {
    /// Install an APK or app bundle.
    InstallApp { path: String },
    /// Launch an app by package/bundle ID.
    LaunchApp { package: String },
    /// Tap at screen coordinates.
    Tap { x: i32, y: i32 },
    /// Swipe from one point to another.
    Swipe { x1: i32, y1: i32, x2: i32, y2: i32 },
    /// Type text into the focused field.
    TypeText { text: String },
    /// Press a hardware button (home, back, etc.).
    PressButton { button: String },
    /// Take a screenshot.
    Screenshot,
    /// Record video for a duration.
    RecordVideo { duration_secs: u64 },
    /// Execute an ADB/simctl shell command.
    ShellCommand { command: String },
}

// ---------------------------------------------------------------------------
// Device artifacts
// ---------------------------------------------------------------------------

/// An artifact produced by a device action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DeviceArtifact {
    /// A screenshot.
    Screenshot { uri: String, captured_at: DateTime<Utc> },
    /// A video recording.
    Video { uri: String, duration_secs: u64 },
    /// Shell command output.
    ShellOutput { stdout: String, stderr: String, exit_code: i32 },
    /// Action completed with no specific artifact.
    Ack,
}

// ---------------------------------------------------------------------------
// Device state
// ---------------------------------------------------------------------------

/// Runtime state of a mobile device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceState {
    pub id: DeviceId,
    pub spec: DeviceSpec,
    pub status: DeviceStatus,
    pub booted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    Booting,
    Running,
    Paused,
    ShuttingDown,
    Off,
    Failed,
}

// ---------------------------------------------------------------------------
// MobileController trait
// ---------------------------------------------------------------------------

/// Controls mobile emulator lifecycle and device actions.
#[async_trait::async_trait]
pub trait MobileController: Send + Sync {
    /// Boot a mobile emulator with the given spec.
    async fn boot(&self, spec: DeviceSpec) -> anyhow::Result<DeviceId>;

    /// Perform an action on a booted device.
    async fn perform(&self, id: DeviceId, action: DeviceAction) -> anyhow::Result<DeviceArtifact>;

    /// Get the current state of a device.
    async fn state(&self, id: DeviceId) -> anyhow::Result<DeviceState>;

    /// Shut down a device.
    async fn shutdown(&self, id: DeviceId) -> anyhow::Result<()>;

    /// List all active device IDs.
    async fn list_devices(&self) -> anyhow::Result<Vec<DeviceId>>;
}
