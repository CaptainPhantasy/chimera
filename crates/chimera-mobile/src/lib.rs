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

// ---------------------------------------------------------------------------
// Mock mobile controller
// ---------------------------------------------------------------------------

/// A mock MobileController that tracks device lifecycle in memory.
pub struct MockMobileController {
    devices: tokio::sync::RwLock<std::collections::HashMap<DeviceId, DeviceState>>,
}

impl MockMobileController {
    pub fn new() -> Self {
        Self {
            devices: tokio::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }
}

impl Default for MockMobileController {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl MobileController for MockMobileController {
    async fn boot(&self, spec: DeviceSpec) -> anyhow::Result<DeviceId> {
        let id = Uuid::new_v4();
        let state = DeviceState {
            id,
            spec,
            status: DeviceStatus::Running,
            booted_at: Some(Utc::now()),
        };
        self.devices.write().await.insert(id, state);
        Ok(id)
    }

    async fn perform(&self, id: DeviceId, action: DeviceAction) -> anyhow::Result<DeviceArtifact> {
        let devices = self.devices.read().await;
        if !devices.contains_key(&id) {
            anyhow::bail!("device not found: {}", id);
        }
        match action {
            DeviceAction::Screenshot => Ok(DeviceArtifact::Screenshot {
                uri: format!("mock://screenshot-{}.png", id),
                captured_at: Utc::now(),
            }),
            DeviceAction::ShellCommand { command } => Ok(DeviceArtifact::ShellOutput {
                stdout: format!("mock output of: {}", command),
                stderr: String::new(),
                exit_code: 0,
            }),
            _ => Ok(DeviceArtifact::Ack),
        }
    }

    async fn state(&self, id: DeviceId) -> anyhow::Result<DeviceState> {
        self.devices
            .read()
            .await
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("device not found: {}", id))
    }

    async fn shutdown(&self, id: DeviceId) -> anyhow::Result<()> {
        let mut devices = self.devices.write().await;
        match devices.get_mut(&id) {
            Some(state) => {
                state.status = DeviceStatus::Off;
                Ok(())
            }
            None => anyhow::bail!("device not found: {}", id),
        }
    }

    async fn list_devices(&self) -> anyhow::Result<Vec<DeviceId>> {
        Ok(self.devices.read().await.keys().copied().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel_spec() -> DeviceSpec {
        DeviceSpec {
            profile: "pixel8".into(),
            os_version: Some("34".into()),
            network: true,
            label: Some("test-device".into()),
        }
    }

    #[tokio::test]
    async fn boot_creates_running_device() {
        let ctrl = MockMobileController::new();
        let id = ctrl.boot(pixel_spec()).await.unwrap();

        let state = ctrl.state(id).await.unwrap();
        assert_eq!(state.status, DeviceStatus::Running);
        assert_eq!(state.spec.profile, "pixel8");
        assert!(state.booted_at.is_some());
    }

    #[tokio::test]
    async fn perform_screenshot() {
        let ctrl = MockMobileController::new();
        let id = ctrl.boot(pixel_spec()).await.unwrap();

        let artifact = ctrl.perform(id, DeviceAction::Screenshot).await.unwrap();
        match artifact {
            DeviceArtifact::Screenshot { uri, .. } => assert!(uri.contains("screenshot")),
            _ => panic!("expected screenshot artifact"),
        }
    }

    #[tokio::test]
    async fn perform_shell_command() {
        let ctrl = MockMobileController::new();
        let id = ctrl.boot(pixel_spec()).await.unwrap();

        let artifact = ctrl
            .perform(id, DeviceAction::ShellCommand { command: "ls /".into() })
            .await
            .unwrap();
        match artifact {
            DeviceArtifact::ShellOutput { stdout, exit_code, .. } => {
                assert!(stdout.contains("ls /"));
                assert_eq!(exit_code, 0);
            }
            _ => panic!("expected shell output"),
        }
    }

    #[tokio::test]
    async fn perform_on_unknown_device_fails() {
        let ctrl = MockMobileController::new();
        let result = ctrl.perform(Uuid::new_v4(), DeviceAction::Screenshot).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn shutdown_sets_off() {
        let ctrl = MockMobileController::new();
        let id = ctrl.boot(pixel_spec()).await.unwrap();

        ctrl.shutdown(id).await.unwrap();
        let state = ctrl.state(id).await.unwrap();
        assert_eq!(state.status, DeviceStatus::Off);
    }

    #[tokio::test]
    async fn list_devices_tracks_booted() {
        let ctrl = MockMobileController::new();
        assert!(ctrl.list_devices().await.unwrap().is_empty());

        let id1 = ctrl.boot(pixel_spec()).await.unwrap();
        let id2 = ctrl.boot(pixel_spec()).await.unwrap();

        let devices = ctrl.list_devices().await.unwrap();
        assert_eq!(devices.len(), 2);
        assert!(devices.contains(&id1));
        assert!(devices.contains(&id2));
    }

    #[test]
    fn mock_mobile_controller_default() {
        let _ = MockMobileController::default();
    }

    #[tokio::test]
    async fn shutdown_unknown_device_fails() {
        let ctrl = MockMobileController::new();
        assert!(ctrl.shutdown(Uuid::new_v4()).await.is_err());
    }
}
