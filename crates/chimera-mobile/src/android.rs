use super::*;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;
use tokio::io::AsyncReadExt;

/// Operator-selected SDK and storage paths. No tools or images are downloaded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AndroidConfig {
    pub adb: PathBuf,
    pub emulator: PathBuf,
    pub avd_home: PathBuf,
    pub app_root: PathBuf,
    pub artifact_dir: PathBuf,
}
impl AndroidConfig {
    pub fn discover(app_root: PathBuf, artifact_dir: PathBuf) -> anyhow::Result<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| anyhow::anyhow!("HOME is not set; supply AndroidConfig explicitly"))?;
        let sdk = std::env::var_os("ANDROID_HOME").or_else(|| std::env::var_os("ANDROID_SDK_ROOT"))
            .map(PathBuf::from).unwrap_or_else(|| home.join("Library/Android/sdk"));
        let adb = std::env::var_os("CHIMERA_ADB").map(PathBuf::from).unwrap_or_else(|| sdk.join("platform-tools/adb"));
        let emulator = std::env::var_os("CHIMERA_EMULATOR").map(PathBuf::from).unwrap_or_else(|| sdk.join("emulator/emulator"));
        anyhow::ensure!(adb.is_file(), "Android Debug Bridge is unavailable at {}; install Android SDK platform-tools or set CHIMERA_ADB", adb.display());
        anyhow::ensure!(emulator.is_file(), "Android emulator is unavailable at {}; install Android SDK emulator or set CHIMERA_EMULATOR", emulator.display());
        let avd_home = std::env::var_os("ANDROID_AVD_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".android/avd"));
        Ok(Self {adb, emulator, avd_home, app_root, artifact_dir})
    }
}

struct ProcessGroup { pid: i32, active: AtomicBool }
impl ProcessGroup {
    fn stop(&self) {
        if self.active.swap(false, Ordering::SeqCst) {
            #[cfg(unix)] unsafe {
                unsafe extern "C" { fn kill(pid: i32, signal: i32) -> i32; }
                kill(-self.pid, 9);
            }
        }
    }
}
impl Drop for ProcessGroup { fn drop(&mut self) { self.stop(); } }

struct Process {
    child: tokio::process::Child,
    group: Arc<ProcessGroup>,
    log: Arc<std::sync::Mutex<Vec<u8>>>,
}
impl Process {
    fn spawn(mut command: tokio::process::Command) -> anyhow::Result<Self> {
        command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
        #[cfg(unix)] { use std::os::unix::process::CommandExt; command.as_std_mut().process_group(0); }
        let mut child = command.spawn()?;
        let group = Arc::new(ProcessGroup {pid: child.id().unwrap() as i32, active: AtomicBool::new(true)});
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        for mut stream in [Box::new(child.stdout.take().unwrap()) as Box<dyn tokio::io::AsyncRead + Unpin + Send>, Box::new(child.stderr.take().unwrap())] {
            let log = log.clone();
            tokio::spawn(async move {
                let mut bytes = [0; 4096];
                while let Ok(count) = stream.read(&mut bytes).await {
                    if count == 0 { break; }
                    if let Ok(mut log) = log.lock() { let count = count.min(16_384usize.saturating_sub(log.len())); log.extend_from_slice(&bytes[..count]); }
                }
            });
        }
        Ok(Self {child, group, log})
    }
    fn running(&mut self) -> anyhow::Result<bool> {
        let running = self.group.active.load(Ordering::SeqCst) && self.child.try_wait()?.is_none();
        if !running { self.group.stop(); }
        Ok(running)
    }
    fn diagnostic(&self) -> String { self.log.lock().map(|log| String::from_utf8_lossy(&log).into_owned()).unwrap_or_default() }
    async fn stop(&mut self) -> anyhow::Result<()> {
        self.group.stop();
        tokio::time::timeout(Duration::from_secs(3), self.child.wait()).await
            .map_err(|_| anyhow::anyhow!("owned device process did not stop in time"))??;
        Ok(())
    }
}
impl Drop for Process { fn drop(&mut self) { self.group.stop(); } }

struct Device {
    state: DeviceState,
    serial: String,
    server_port: u16,
    home: PathBuf,
    emulator: Process,
    server: Process,
}
struct DeviceHandle { groups: Vec<Arc<ProcessGroup>>, inner: tokio::sync::Mutex<Device> }
impl Device {
    fn running(&mut self) -> anyhow::Result<bool> {
        Ok(self.state.status == DeviceStatus::Running && self.emulator.running()? && self.server.running()?)
    }
}
struct StopOnCancel { groups: Vec<Arc<ProcessGroup>>, armed: bool }
impl Drop for StopOnCancel {
    fn drop(&mut self) { if self.armed { for group in &self.groups { group.stop(); } } }
}

/// Android SDK adapter for newly launched, owned emulators. Existing phones and
/// emulators are never adopted. The caller must approve each requested action.
/// Network-disabled operation is rejected until an OS network boundary is available.
pub struct AndroidController {
    config: AndroidConfig,
    devices: tokio::sync::RwLock<HashMap<DeviceId, Arc<DeviceHandle>>>,
    boot_lock: tokio::sync::Mutex<()>,
}
impl AndroidController {
    pub fn new(mut config: AndroidConfig) -> anyhow::Result<Self> {
        anyhow::ensure!(cfg!(unix), "owned Android process groups require Unix");
        for (name, path) in [("Android Debug Bridge", &config.adb), ("Android emulator", &config.emulator)] {
            anyhow::ensure!(path.is_file(), "{name} is unavailable at {}", path.display());
        }
        anyhow::ensure!(config.avd_home.is_dir(), "Android device profiles are unavailable at {}", config.avd_home.display());
        config.avd_home = config.avd_home.canonicalize()?;
        config.app_root = config.app_root.canonicalize()?;
        private_directory(&config.artifact_dir)?;
        config.artifact_dir = config.artifact_dir.canonicalize()?;
        Ok(Self {config, devices: tokio::sync::RwLock::new(HashMap::new()), boot_lock: tokio::sync::Mutex::new(())})
    }
    fn command(&self, binary: &Path, home: &Path) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(binary);
        command.env_clear().env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin")
            .env("HOME", home).env("TMPDIR", home).env("ANDROID_AVD_HOME", &self.config.avd_home)
            // Restrict discovery to the emulator explicitly launched below.
            .env("ADB_LOCAL_TRANSPORT_MAX_PORT", "0").env("ADB_MDNS_AUTO_CONNECT", "")
            .env("ANDROID_USER_HOME", home.join(".android"));
        command
    }
    async fn device(&self, id: DeviceId) -> anyhow::Result<Arc<DeviceHandle>> {
        self.devices.read().await.get(&id).cloned().ok_or_else(|| anyhow::anyhow!("unknown Android device: {id}"))
    }
    async fn adb(&self, device: &mut Device, args: Vec<String>, seconds: u64, max: usize) -> anyhow::Result<Output> {
        anyhow::ensure!(device.emulator.running()? && device.server.running()?, "owned Android process is no longer running");
        let mut command = self.command(&self.config.adb, &device.home);
        // Numeric 127.0.0.1 is deliberately explicit. ADB treats an explicit
        // non-"localhost" host as remote and will not auto-start a lost server.
        command.args(["-H", "127.0.0.1", "-P", &device.server_port.to_string(), "-s", &device.serial]).args(args);
        let mut guard = StopOnCancel {groups: vec![device.emulator.group.clone(), device.server.group.clone()], armed: true};
        let output = run(command, Duration::from_secs(seconds), max).await?;
        guard.armed = false;
        Ok(output)
    }
    async fn shell(&self, device: &mut Device, command: String, seconds: u64) -> anyhow::Result<Output> {
        self.adb(device, vec!["shell".into(), command], seconds, 100_000).await
    }
    fn apk(&self, input: &str) -> anyhow::Result<PathBuf> {
        let input = Path::new(input);
        anyhow::ensure!(!input.components().any(|c| c == Component::ParentDir), "parent path traversal is not allowed");
        let joined = if input.is_absolute() {input.to_path_buf()} else {self.config.app_root.join(input)};
        let relative = joined.strip_prefix(&self.config.app_root).map_err(|_| anyhow::anyhow!("app must be inside the approved app root"))?;
        let mut cursor = self.config.app_root.clone();
        for component in relative.components() {
            let name = component.as_os_str().to_string_lossy();
            anyhow::ensure!(![".git", ".ssh", ".aws", ".codex", ".agents", ".chimera", ".supercache"].contains(&name.as_ref()) && !name.starts_with(".env"), "protected app path");
            cursor.push(component);
            anyhow::ensure!(!std::fs::symlink_metadata(&cursor)?.file_type().is_symlink(), "app path cannot contain symbolic links");
        }
        let file = joined.canonicalize()?;
        anyhow::ensure!(file.starts_with(&self.config.app_root) && file.is_file() && file.extension().is_some_and(|ext| ext == "apk"), "install requires an APK file in the approved app root");
        Ok(file)
    }
}

#[async_trait::async_trait]
impl MobileController for AndroidController {
    async fn boot(&self, spec: DeviceSpec) -> anyhow::Result<DeviceId> {
        validate_spec(&spec)?;
        let _boot = self.boot_lock.lock().await;
        anyhow::ensure!(self.list_devices().await?.len() < 4, "Android device limit reached");
        let mut profiles = self.command(&self.config.emulator, &self.config.artifact_dir);
        profiles.arg("-list-avds");
        let profiles = run(profiles, Duration::from_secs(10), 100_000).await?.checked()?;
        anyhow::ensure!(String::from_utf8_lossy(&profiles.stdout).lines().any(|name| name.trim() == spec.profile), "Android profile is not installed: {}", spec.profile);
        let id = Uuid::new_v4();
        let home = self.config.artifact_dir.join(id.to_string());
        private_directory(&home)?;
        private_directory(&home.join(".android"))?;
        // Own a foreground ADB server. Never stop or reuse the user's shared server.
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let server_port = listener.local_addr()?.port();
        let mut server_command = self.command(&self.config.adb, &home);
        server_command.args(["-L", &format!("tcp:127.0.0.1:{server_port}"), "--one-device", &format!("chimera-{id}"), "server", "nodaemon"]);
        drop(listener);
        let mut server = Process::spawn(server_command)?;
        let ready = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                anyhow::ensure!(server.running()?, "owned Android control server exited: {}", server.diagnostic());
                if tokio::net::TcpStream::connect(("127.0.0.1", server_port)).await.is_ok() { return Ok::<_, anyhow::Error>(()); }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }).await;
        ready.map_err(|_| anyhow::anyhow!("Android control server startup timed out"))??;
        let port = available_emulator_port()?;
        let mut emulator_command = self.command(&self.config.emulator, &home);
        emulator_command.env("ANDROID_ADB_SERVER_PORT", server_port.to_string()).args(emulator_args(&spec.profile, port));
        let emulator = Process::spawn(emulator_command)?;
        let mut device = Device {state: DeviceState {id, spec, status: DeviceStatus::Booting, booted_at: None},
            serial: format!("emulator-{port}"), server_port, home, emulator, server};
        let boot = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let output = self.shell(&mut device, "getprop sys.boot_completed".into(), 5).await?;
                if output.exit_code == 0 && String::from_utf8_lossy(&output.stdout).trim() == "1" { break; }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            if let Some(expected) = device.state.spec.os_version.clone() {
                let sdk = self.shell(&mut device, "getprop ro.build.version.sdk".into(), 5).await?.checked()?;
                let release = self.shell(&mut device, "getprop ro.build.version.release".into(), 5).await?.checked()?;
                anyhow::ensure!(expected == String::from_utf8_lossy(&sdk.stdout).trim() || expected == String::from_utf8_lossy(&release.stdout).trim(), "Android version does not match requested {expected}");
            }
            Ok::<_, anyhow::Error>(())
        }).await;
        boot.map_err(|_| anyhow::anyhow!("Android emulator did not finish booting within 120 seconds"))??;
        device.state.status = DeviceStatus::Running;
        device.state.booted_at = Some(Utc::now());
        let groups = vec![device.emulator.group.clone(), device.server.group.clone()];
        self.devices.write().await.insert(id, Arc::new(DeviceHandle {groups, inner: tokio::sync::Mutex::new(device)}));
        Ok(id)
    }

    async fn perform(&self, id: DeviceId, action: DeviceAction) -> anyhow::Result<DeviceArtifact> {
        let device = self.device(id).await?;
        let mut device = device.inner.lock().await;
        anyhow::ensure!(device.running()?, "Android device is not running");
        match action {
            DeviceAction::InstallApp {path} => {
                let apk = self.apk(&path)?;
                let output = self.adb(&mut device, vec!["install".into(), apk.to_string_lossy().into_owned()], 60, 100_000).await?.checked()?;
                anyhow::ensure!(String::from_utf8_lossy(&output.stdout).lines().any(|line| line.trim() == "Success"), "Android package manager did not confirm installation");
            }
            DeviceAction::Screenshot => {
                let output = self.adb(&mut device, vec!["exec-out".into(), "screencap -p".into()], 15, 16_000_000).await?.checked()?;
                anyhow::ensure!(!output.truncated && output.stdout.starts_with(b"\x89PNG\r\n\x1a\n"), "Android did not return a complete PNG screenshot");
                let uri = save_artifact(&device.home, "png", &output.stdout)?;
                return Ok(DeviceArtifact::Screenshot {uri, captured_at: Utc::now()});
            }
            DeviceAction::RecordVideo {duration_secs} => {
                anyhow::ensure!((1..=60).contains(&duration_secs), "video duration must be between 1 and 60 seconds");
                let remote = format!("/sdcard/chimera-{}.mp4", Uuid::new_v4());
                self.shell(&mut device, format!("screenrecord --bit-rate 1000000 --time-limit {duration_secs} {}", quote(&remote)), duration_secs + 15).await?.checked()?;
                let output = self.adb(&mut device, vec!["exec-out".into(), format!("cat {}", quote(&remote))], 30, 16_000_000).await?.checked()?;
                anyhow::ensure!(!output.truncated && output.stdout.get(4..8) == Some(b"ftyp"), "Android did not return a complete MP4 recording");
                let uri = save_artifact(&device.home, "mp4", &output.stdout)?;
                return Ok(DeviceArtifact::Video {uri, duration_secs});
            }
            action => {
                let is_shell = matches!(action, DeviceAction::ShellCommand {..});
                let command = action_command(&action)?;
                let output = self.shell(&mut device, command, 30).await?;
                if is_shell {
                    return Ok(DeviceArtifact::ShellOutput {stdout: output.text_stdout(), stderr: String::from_utf8_lossy(&output.stderr).into_owned(), exit_code: output.exit_code});
                }
                output.checked()?;
            }
        }
        Ok(DeviceArtifact::Ack)
    }
    async fn state(&self, id: DeviceId) -> anyhow::Result<DeviceState> {
        let device = self.device(id).await?;
        let mut device = device.inner.lock().await;
        if device.state.status == DeviceStatus::Running && !device.running()? { device.state.status = DeviceStatus::Failed; }
        Ok(device.state.clone())
    }
    async fn shutdown(&self, id: DeviceId) -> anyhow::Result<()> {
        let device = self.device(id).await?;
        for group in &device.groups { group.stop(); }
        let mut device = device.inner.lock().await;
        if device.state.status == DeviceStatus::Off { return Ok(()); }
        device.state.status = DeviceStatus::ShuttingDown;
        let emulator = device.emulator.stop().await;
        let server = device.server.stop().await;
        if let Err(error) = emulator.and(server) { device.state.status = DeviceStatus::Failed; return Err(error); }
        device.state.status = DeviceStatus::Off;
        Ok(())
    }
    async fn list_devices(&self) -> anyhow::Result<Vec<DeviceId>> {
        let devices: Vec<_> = self.devices.read().await.iter().map(|(id, device)| (*id, device.clone())).collect();
        let mut active = Vec::new();
        for (id, device) in devices { if device.groups.iter().all(|group| group.active.load(Ordering::SeqCst)) && device.inner.lock().await.running()? { active.push(id); } }
        Ok(active)
    }
}

fn validate_spec(spec: &DeviceSpec) -> anyhow::Result<()> {
    anyhow::ensure!(!spec.profile.is_empty() && spec.profile.len() <= 128 && spec.profile.bytes().all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c)), "invalid Android device profile name");
    anyhow::ensure!(spec.network, "network-disabled Android emulation is unsupported; an OS network boundary is required (network is not silently enabled)");
    Ok(())
}
fn emulator_args(profile: &str, port: u16) -> Vec<String> {
    ["-avd", profile, "-port", &port.to_string(), "-read-only", "-no-snapshot", "-no-window", "-no-audio", "-no-boot-anim", "-camera-front", "none", "-camera-back", "none"].iter().map(|v| v.to_string()).collect()
}
fn available_emulator_port() -> anyhow::Result<u16> {
    for port in (5554..=5682).step_by(2) {
        if let Ok(_console) = std::net::TcpListener::bind(("127.0.0.1", port)) {
            if let Ok(_adb) = std::net::TcpListener::bind(("127.0.0.1", port + 1)) { return Ok(port); }
        }
    }
    anyhow::bail!("no free Android emulator port pair is available")
}
fn quote(value: &str) -> String { format!("'{}'", value.replace('\'', "'\"'\"'")) }
fn action_command(action: &DeviceAction) -> anyhow::Result<String> {
    Ok(match action {
        DeviceAction::LaunchApp {package} => {
            anyhow::ensure!(!package.is_empty() && package.len() <= 255 && package.bytes().all(|c| c.is_ascii_alphanumeric() || b"._".contains(&c)), "invalid Android package name");
            format!("monkey -p {} -c android.intent.category.LAUNCHER 1", quote(package))
        }
        DeviceAction::Tap {x,y} => { coordinates(&[*x,*y])?; format!("input tap {x} {y}") }
        DeviceAction::Swipe {x1,y1,x2,y2} => { coordinates(&[*x1,*y1,*x2,*y2])?; format!("input swipe {x1} {y1} {x2} {y2}") }
        DeviceAction::TypeText {text} => {
            anyhow::ensure!(text.len() <= 4096 && text.bytes().all(|c| (32..=126).contains(&c)) && !text.contains("%s"), "Android input text supports printable ASCII without literal %s; use an app input method for other text");
            format!("input text {}", quote(&text.replace(' ', "%s")))
        }
        DeviceAction::PressButton {button} => {
            let key = match button.as_str() {"home" => "KEYCODE_HOME", "back" => "KEYCODE_BACK", "enter" => "KEYCODE_ENTER", "power" => "KEYCODE_POWER", "volume_up" => "KEYCODE_VOLUME_UP", "volume_down" => "KEYCODE_VOLUME_DOWN", "menu" => "KEYCODE_MENU", _ => anyhow::bail!("unsupported Android button: {button}")};
            format!("input keyevent {key}")
        }
        DeviceAction::ShellCommand {command} => {
            anyhow::ensure!(!command.is_empty() && command.len() <= 10_000 && !command.contains('\0'), "invalid Android shell command"); command.clone()
        }
        _ => anyhow::bail!("action needs a file transfer"),
    })
}
fn coordinates(values: &[i32]) -> anyhow::Result<()> { anyhow::ensure!(values.iter().all(|v| (0..=16384).contains(v)), "invalid device coordinates"); Ok(()) }
fn private_directory(path: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(path)?;
    anyhow::ensure!(!std::fs::symlink_metadata(path)?.file_type().is_symlink(), "device directory cannot be a symbolic link");
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?; }
    Ok(())
}
fn save_artifact(home: &Path, extension: &str, bytes: &[u8]) -> anyhow::Result<String> {
    use std::io::Write;
    let path = home.join(format!("{}.{}", Uuid::new_v4(), extension));
    let mut options = std::fs::OpenOptions::new(); options.write(true).create_new(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    let mut file = options.open(&path)?; file.write_all(bytes)?; file.sync_all()?;
    Ok(path.to_string_lossy().into_owned())
}
struct Output { stdout: Vec<u8>, stderr: Vec<u8>, exit_code: i32, truncated: bool }
impl Output {
    fn checked(self) -> anyhow::Result<Self> {
        anyhow::ensure!(self.exit_code == 0, "Android command exited {}: {}", self.exit_code, String::from_utf8_lossy(&self.stderr));
        Ok(self)
    }
    fn text_stdout(&self) -> String {
        let mut text = String::from_utf8_lossy(&self.stdout).into_owned();
        if self.truncated {text.push_str("\n[output truncated]");} text
    }
}
async fn run(mut command: tokio::process::Command, timeout: Duration, max: usize) -> anyhow::Result<Output> {
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    #[cfg(unix)] { use std::os::unix::process::CommandExt; command.as_std_mut().process_group(0); }
    let mut child = command.spawn()?;
    let group = ProcessGroup {pid: child.id().unwrap() as i32, active: AtomicBool::new(true)};
    let stdout = child.stdout.take().unwrap(); let stderr = child.stderr.take().unwrap();
    let operation = async {
        let (status, stdout, stderr) = tokio::join!(child.wait(), bounded(stdout, max), bounded(stderr, 16_384));
        let (stdout, truncated) = stdout?; let (stderr, error_truncated) = stderr?;
        Ok::<_, anyhow::Error>(Output {stdout, stderr, exit_code: status?.code().unwrap_or(-1), truncated: truncated || error_truncated})
    };
    let output = tokio::time::timeout(timeout, operation).await.map_err(|_| anyhow::anyhow!("Android command timed out; owned processes stopped"))??;
    group.stop();
    Ok(output)
}
async fn bounded(mut stream: impl tokio::io::AsyncRead + Unpin, max: usize) -> anyhow::Result<(Vec<u8>, bool)> {
    let mut output = Vec::new(); let mut buffer = [0; 8192]; let mut truncated = false;
    loop {
        let count = stream.read(&mut buffer).await?; if count == 0 {break;}
        let keep = count.min(max.saturating_sub(output.len())); output.extend_from_slice(&buffer[..keep]); truncated |= keep != count;
    }
    Ok((output, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn network_and_profile_are_validated_before_spawning() {
        let mut spec = DeviceSpec {profile:"pixel8".into(), os_version:None, network:false, label:None};
        assert!(validate_spec(&spec).unwrap_err().to_string().contains("network-disabled"));
        spec.network = true; validate_spec(&spec).unwrap();
        spec.profile = "../../other".into(); assert!(validate_spec(&spec).is_err());
    }
    #[test]
    fn no_implicit_host_shell_and_text_metacharacters_are_quoted() {
        assert_eq!(action_command(&DeviceAction::TypeText {text:"a';echo x".into()}).unwrap(), "input text 'a'\"'\"';echo%sx'");
        assert!(action_command(&DeviceAction::LaunchApp {package:"x;touch /tmp/x".into()}).is_err());
        assert!(action_command(&DeviceAction::Tap {x:-1,y:0}).is_err());
        assert!(action_command(&DeviceAction::TypeText {text:"hello 😀".into()}).is_err());
        assert!(action_command(&DeviceAction::PressButton {button:"unknown".into()}).is_err());
    }
    #[test]
    fn emulator_never_wipes_or_persists_user_profile() {
        let args = emulator_args("pixel8", 5554);
        assert!(args.contains(&"-read-only".into())); assert!(args.contains(&"-no-snapshot".into()));
        assert!(!args.contains(&"-wipe-data".into()));
    }
    #[test]
    fn missing_tools_return_real_error() {
        let config = AndroidConfig {adb:"/no-such-adb".into(), emulator:"/no-such-emulator".into(), avd_home:"/no-such-avd".into(), app_root:"/tmp".into(), artifact_dir:"/tmp".into()};
        assert!(matches!(AndroidController::new(config), Err(error) if error.to_string().contains("Android Debug Bridge is unavailable")));
    }
    #[cfg(unix)]
    #[test]
    fn app_install_paths_are_confined_and_reject_links() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("chimera-apk-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap(); std::fs::write(root.join("demo.apk"), b"test apk").unwrap();
        std::fs::create_dir(root.join(".private")).unwrap();
        std::fs::write(root.join(".private/secret.apk"), b"private").unwrap();
        symlink(root.join("demo.apk"), root.join("link.apk")).unwrap();
        let config = AndroidConfig {adb:"/bin/sh".into(), emulator:"/bin/sh".into(), avd_home:root.clone(), app_root:root.clone(), artifact_dir:root.join("artifacts")};
        let ctrl = AndroidController::new(config).unwrap();
        assert!(ctrl.apk("demo.apk").is_ok());
        assert!(ctrl.apk("../other.apk").is_err()); assert!(ctrl.apk("/tmp/outside.apk").is_err());
        assert!(ctrl.apk("link.apk").is_err());
        std::fs::create_dir(root.join(".env")).unwrap(); std::fs::write(root.join(".env/a.apk"), b"private").unwrap();
        assert!(ctrl.apk(".env/a.apk").is_err());
    }
    #[tokio::test]
    async fn command_output_is_bounded_utf8_safe_and_reports_exit() {
        let mut command = tokio::process::Command::new("/bin/sh");
        command.args(["-c", "printf 'éééé'; printf problem >&2; exit 7"]);
        let output = run(command, Duration::from_secs(2), 5).await.unwrap();
        assert_eq!(output.exit_code, 7); assert_eq!(output.stdout.len(),5); assert!(output.truncated);
        assert!(output.text_stdout().contains("output truncated")); assert!(output.checked().is_err());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_and_timeout_stop_owned_children() {
        let home = std::env::temp_dir().join(format!("chimera-mobile-test-{}", Uuid::new_v4())); std::fs::create_dir(&home).unwrap();
        let marker = home.join("should-not-exist");
        let script = format!("(sleep 1; printf escaped > {}) & wait", quote(marker.to_str().unwrap()));
        let mut command = tokio::process::Command::new("/bin/sh"); command.args(["-c", &script]);
        assert!(run(command, Duration::from_millis(50), 100).await.is_err());
        let mut command = tokio::process::Command::new("/bin/sh"); command.args(["-c", &script]);
        let task = tokio::spawn(run(command, Duration::from_secs(10), 100));
        tokio::time::sleep(Duration::from_millis(50)).await; task.abort(); let _ = task.await;
        tokio::time::sleep(Duration::from_millis(1100)).await; assert!(!marker.exists());
    }
}
