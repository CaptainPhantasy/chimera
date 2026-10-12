use anyhow::{Context, Result};
use chimera_browser::{BrowserAction, BrowserConfig, BrowserController, BrowserTarget, PuppeteerBrowser};
use chimera_mobile::{AndroidConfig, AndroidController, DeviceAction, DeviceSpec, MobileController};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use crate::{OutputFormat, output, policy, workspace_root};

fn actions(path: Option<&Path>) -> Result<Vec<BrowserAction>> {
    let Some(path) = path else { return Ok(Vec::new()); };
    let metadata = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= 100_000,
        "browser actions must be a regular JSON file no larger than 100000 bytes");
    let actions: Vec<BrowserAction> = serde_json::from_slice(&std::fs::read(path)?)
        .context("browser actions must be a JSON array of typed browser actions")?;
    anyhow::ensure!(actions.len() <= 100, "at most 100 browser actions can run at once");
    Ok(actions)
}

async fn approve(tool: &str, args: Value, format: OutputFormat) -> Result<()> {
    crate::commands::approve(chimera_tools::ToolCall {
        tool: tool.into(), args, world: None, requested_by: uuid::Uuid::new_v4(),
    }, format).await
}

fn save_report(directory: &Path, value: &Value) -> Result<PathBuf> {
    use std::io::Write;
    let path = directory.join("result.json");
    let mut options = std::fs::OpenOptions::new(); options.write(true).create_new(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    let mut file = options.open(&path)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    Ok(path)
}

pub(crate) async fn browser(
    target: String,
    actions_path: Option<PathBuf>,
    allowed_origins: Vec<String>,
    output_dir: PathBuf,
    format: OutputFormat,
    profile: Option<&str>,
) -> Result<()> {
    anyhow::ensure!(!policy(profile, false)?.read_only, "read-only policy does not launch a browser");
    let actions = actions(actions_path.as_deref())?;
    let output_dir = output_dir.join(format!("browser-{}", uuid::Uuid::new_v4()));
    let mut config = BrowserConfig::discover(output_dir.clone())?;
    config.allowed_origins = allowed_origins;
    approve("browser_sequence", json!({
        "target": target, "actions": actions, "allowed_origins": config.allowed_origins,
        "output_directory": output_dir, "capture_final_screenshot": true,
        "browser": config.executable_path, "automation_module": config.automation_module,
        "network_policy": "Chrome's network access is allowed; page requests use these origins and empty origins enable page offline mode. This is not operating-system network isolation.",
    }), format).await?;
    let controller = PuppeteerBrowser::new(config)?;
    let target = if target == "about:blank" { BrowserTarget::Blank } else { BrowserTarget::Url(target) };
    let id = tokio::select! {
        result = controller.open(target) => result?,
        signal = tokio::signal::ctrl_c() => { signal?; anyhow::bail!("browser startup cancelled; owned processes stopped"); }
    };
    let mut artifacts = Vec::new();
    let operation: Result<Value> = tokio::select! {
        result = async {
            for action in actions { artifacts.push(controller.act(id, action).await?); }
            let evidence = controller.screenshot(id).await?;
            let state = controller.state(id).await?;
            Ok(json!({"state": state, "screenshot": evidence}))
        } => result,
        signal = tokio::signal::ctrl_c() => { signal?; Err(anyhow::anyhow!("browser actions cancelled")) }
    };
    let cleanup = controller.close(id).await;
    let report = json!({
        "kind": "browser", "session": id, "artifacts": artifacts,
        "result": operation.as_ref().ok(), "error": operation.as_ref().err().map(ToString::to_string),
        "closed": cleanup.is_ok(), "cleanup_error": cleanup.as_ref().err().map(ToString::to_string),
    });
    let report_path = save_report(&output_dir, &report)?;
    output(format, json!({"report": report_path, "details": report}), format!("Browser report: {}", report_path.display()))?;
    cleanup.context("browser cleanup failed")?;
    operation?;
    Ok(())
}

pub(crate) async fn phone(
    device: String,
    app: PathBuf,
    package: String,
    network: bool,
    output_dir: PathBuf,
    format: OutputFormat,
    profile: Option<&str>,
) -> Result<()> {
    anyhow::ensure!(!policy(profile, false)?.read_only, "read-only policy does not launch an emulator");
    anyhow::ensure!(network, "network-disabled Android emulation is unsupported; CHIMERA will not silently enable network access");
    let root = workspace_root().canonicalize()?;
    let app = if app.is_absolute() {app} else {root.join(app)};
    let relative = app.strip_prefix(&root).context("APK must be inside the current project")?;
    let mut cursor = root.clone();
    for component in relative.components() {
        anyhow::ensure!(!matches!(component, std::path::Component::ParentDir), "APK path cannot contain parent traversal");
        cursor.push(component);
        anyhow::ensure!(!std::fs::symlink_metadata(&cursor)?.file_type().is_symlink(), "APK path cannot contain symbolic links");
    }
    let metadata = std::fs::symlink_metadata(&app)?;
    anyhow::ensure!(metadata.is_file() && !metadata.file_type().is_symlink(), "app must be a regular APK file");
    let app = app.canonicalize()?;
    anyhow::ensure!(app.starts_with(&root) && app.extension().is_some_and(|extension| extension == "apk"), "APK must be inside the current project");
    let output_dir = output_dir.join(format!("android-{}", uuid::Uuid::new_v4()));
    let config = AndroidConfig::discover(root, output_dir.clone())?;
    approve("android_app_check", json!({
        "device_profile": device, "apk": app, "package": package, "network": network,
        "output_directory": output_dir, "actions": ["boot owned emulator", "install APK", "launch package", "screenshot", "shutdown owned emulator"],
        "adb": config.adb, "emulator": config.emulator,
    }), format).await?;
    let controller = AndroidController::new(config)?;
    let spec = DeviceSpec {profile: device, os_version: None, network, label: Some("CHIMERA app check".into())};
    let id = tokio::select! {
        result = controller.boot(spec) => result?,
        signal = tokio::signal::ctrl_c() => { signal?; anyhow::bail!("Android startup cancelled; owned processes stopped"); }
    };
    let operation: Result<Value> = tokio::select! {
        result = async {
            controller.perform(id, DeviceAction::InstallApp {path: app.to_string_lossy().into_owned()}).await?;
            controller.perform(id, DeviceAction::LaunchApp {package}).await?;
            let screenshot = controller.perform(id, DeviceAction::Screenshot).await?;
            let state = controller.state(id).await?;
            Ok(json!({"state": state, "screenshot": screenshot}))
        } => result,
        signal = tokio::signal::ctrl_c() => { signal?; Err(anyhow::anyhow!("Android app check cancelled")) }
    };
    let cleanup = controller.shutdown(id).await;
    let report = json!({
        "kind": "android", "device": id,
        "result": operation.as_ref().ok(), "error": operation.as_ref().err().map(ToString::to_string),
        "closed": cleanup.is_ok(), "cleanup_error": cleanup.as_ref().err().map(ToString::to_string),
    });
    let report_path = save_report(&output_dir, &report)?;
    output(format, json!({"report": report_path, "details": report}), format!("Android report: {}", report_path.display()))?;
    cleanup.context("Android cleanup failed")?;
    operation?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_actions_are_bounded_and_reject_unknown_operations() {
        let file = std::env::temp_dir().join(format!("chimera-actions-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&file, r#"[{"action":"read_text","selector":"h1"}]"#).unwrap();
        assert_eq!(actions(Some(&file)).unwrap().len(), 1);
        std::fs::write(&file, r#"[{"action":"invented"}]"#).unwrap();
        assert!(actions(Some(&file)).is_err());
    }
    #[tokio::test]
    async fn read_only_rejects_before_creating_output_or_starting_programs() {
        let path = std::env::temp_dir().join(format!("chimera-device-denied-{}", uuid::Uuid::new_v4()));
        let error = browser("about:blank".into(), None, vec![], path.clone(), OutputFormat::Json, Some("read-only")).await.unwrap_err();
        assert!(error.to_string().contains("read-only")); assert!(!path.exists());
        let error = phone("pixel8".into(), "demo.apk".into(), "org.demo".into(), true, path.clone(), OutputFormat::Json, Some("read-only")).await.unwrap_err();
        assert!(error.to_string().contains("read-only")); assert!(!path.exists());
    }
}
