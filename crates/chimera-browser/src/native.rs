use super::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, atomic::{AtomicBool, AtomicI32, Ordering}};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    pub node: PathBuf,
    pub automation_module: PathBuf,
    pub executable_path: PathBuf,
    pub artifact_dir: PathBuf,
    /// Exact http(s) origins allowed for page requests. Empty means data/blank
    /// with page offline mode. This is not an OS network boundary; the caller
    /// must approve the browser process's network access before launching it.
    pub allowed_origins: Vec<String>,
}
impl BrowserConfig {
    pub fn discover(artifact_dir: PathBuf) -> anyhow::Result<Self> {
        let node = std::env::var_os("CHIMERA_BROWSER_NODE").map(PathBuf::from)
            .or_else(|| find_on_path("node")).ok_or_else(|| anyhow::anyhow!("Node.js is required; set CHIMERA_BROWSER_NODE"))?;
        let module = std::env::var_os("CHIMERA_BROWSER_MODULE").map(PathBuf::from).or_else(|| {
            ["/opt/homebrew/lib/node_modules/puppeteer/lib/cjs/puppeteer/puppeteer.js", "/usr/lib/node_modules/puppeteer/lib/cjs/puppeteer/puppeteer.js", "node_modules/puppeteer/lib/cjs/puppeteer/puppeteer.js"]
                .iter().map(PathBuf::from).find(|p| p.is_file())
        }).ok_or_else(|| anyhow::anyhow!("Puppeteer is required; set CHIMERA_BROWSER_MODULE to its entry file"))?;
        let chrome = std::env::var_os("CHIMERA_BROWSER_EXECUTABLE").map(PathBuf::from).or_else(|| {
            ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/usr/bin/google-chrome", "/usr/bin/chromium"]
                .iter().map(PathBuf::from).find(|p| p.is_file())
        }).ok_or_else(|| anyhow::anyhow!("Chrome is required; set CHIMERA_BROWSER_EXECUTABLE"))?;
        Ok(Self {node, automation_module: module, executable_path: chrome, artifact_dir, allowed_origins: vec![]})
    }
}
fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|p| p.join(name)).find(|p| p.is_file()))
}

struct OwnedProcesses { bridge: i32, browser: AtomicI32, active: AtomicBool }
impl OwnedProcesses {
    fn stop(&self) {
        if !self.active.swap(false, Ordering::SeqCst) { return; }
        #[cfg(unix)] unsafe {
            unsafe extern "C" { fn kill(pid: i32, signal: i32) -> i32; }
            let browser = self.browser.load(Ordering::SeqCst);
            if browser > 0 { kill(-browser, 9); kill(browser, 9); }
            // While launch is pending, Puppeteer's signal handler closes the
            // browser it owns. After its PID is known both groups are bounded.
            let signal = if browser > 0 { 9 } else { 15 };
            kill(-self.bridge, signal);
        }
    }
}
impl Drop for OwnedProcesses { fn drop(&mut self) { self.stop(); } }
struct CancelGuard { owned: Arc<OwnedProcesses>, armed: bool }
impl Drop for CancelGuard { fn drop(&mut self) { if self.armed { self.owned.stop(); } } }

struct Session {
    child: tokio::process::Child,
    input: tokio::process::ChildStdin,
    output: BufReader<tokio::process::ChildStdout>,
    owned: Arc<OwnedProcesses>,
    state: BrowserState,
    stderr: Arc<std::sync::Mutex<String>>,
}
impl Session {
    async fn request(&mut self, value: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        anyhow::ensure!(self.owned.active.load(Ordering::SeqCst), "browser session is closed");
        let mut guard = CancelGuard { owned: self.owned.clone(), armed: true };
        let bytes = serde_json::to_vec(&value)?;
        anyhow::ensure!(bytes.len() <= 200_000, "browser request exceeds size limit");
        let exchange = async {
            self.input.write_all(&bytes).await?;
            self.input.write_all(b"\n").await?;
            self.input.flush().await?;
            let mut line = Vec::new();
            // A bounded reader prevents a damaged bridge from exhausting memory.
            let count = (&mut self.output).take(1_000_001).read_until(b'\n', &mut line).await?;
            anyhow::ensure!(count > 0, "browser bridge closed: {}", self.stderr.lock().map(|v| v.clone()).unwrap_or_default());
            anyhow::ensure!(line.len() <= 1_000_000 && line.last() == Some(&b'\n'), "browser response exceeds size limit");
            Ok::<serde_json::Value, anyhow::Error>(serde_json::from_slice(&line)?)
        };
        let response = tokio::time::timeout(std::time::Duration::from_secs(35), exchange).await
            .map_err(|_| anyhow::anyhow!("browser action timed out; owned browser stopped"))??;
        guard.armed = false;
        anyhow::ensure!(response["ok"] == true, "{}\n{}", response["error"].as_str().unwrap_or("browser action failed"), self.stderr.lock().map(|v| v.clone()).unwrap_or_default());
        Ok(response["value"].clone())
    }
}
impl Drop for Session { fn drop(&mut self) { self.owned.stop(); } }
struct SessionHandle { owned: Arc<OwnedProcesses>, inner: tokio::sync::Mutex<Session> }

/// A real Chrome controller using an owned Puppeteer process over standard pipes.
/// The caller must approve each action before invoking this low-level adapter.
pub struct PuppeteerBrowser {
    config: BrowserConfig,
    sessions: tokio::sync::RwLock<HashMap<BrowserSessionId, Arc<SessionHandle>>>,
    open_lock: tokio::sync::Mutex<()>,
}
impl PuppeteerBrowser {
    pub fn new(mut config: BrowserConfig) -> anyhow::Result<Self> {
        anyhow::ensure!(cfg!(unix), "owned browser process groups are supported only on Unix");
        for (name, path) in [("Node.js", &config.node), ("Puppeteer module", &config.automation_module), ("Chrome", &config.executable_path)] {
            anyhow::ensure!(path.is_file(), "{name} is unavailable at {}", path.display());
        }
        std::fs::create_dir_all(&config.artifact_dir)?;
        anyhow::ensure!(!std::fs::symlink_metadata(&config.artifact_dir)?.file_type().is_symlink(), "browser artifact directory cannot be a symbolic link");
        config.artifact_dir = config.artifact_dir.canonicalize()?;
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&config.artifact_dir, std::fs::Permissions::from_mode(0o700))?; }
        Ok(Self { config, sessions: tokio::sync::RwLock::new(HashMap::new()), open_lock: tokio::sync::Mutex::new(()) })
    }
    async fn session(&self, id: BrowserSessionId) -> anyhow::Result<Arc<SessionHandle>> {
        self.sessions.read().await.get(&id).cloned().ok_or_else(|| anyhow::anyhow!("unknown browser session: {id}"))
    }
    pub async fn process_ids(&self, id: BrowserSessionId) -> anyhow::Result<(u32, u32)> {
        let session = self.session(id).await?;
        let session = session.inner.lock().await;
        Ok((session.owned.bridge as u32, session.owned.browser.load(Ordering::SeqCst) as u32))
    }
    fn command(&self) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.config.node);
        command.env_clear().env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin")
            .env("HOME", &self.config.artifact_dir).env("TMPDIR", &self.config.artifact_dir);
        command
    }
}

#[async_trait::async_trait]
impl BrowserController for PuppeteerBrowser {
    async fn open(&self, target: BrowserTarget) -> anyhow::Result<BrowserSessionId> {
        let _open = self.open_lock.lock().await;
        let url = match target { BrowserTarget::Blank => "about:blank".into(), BrowserTarget::Url(url) => url,
            BrowserTarget::Environment(name) => anyhow::bail!("named browser environment is not configured: {name}") };
        anyhow::ensure!(self.list_sessions().await?.len() < 16, "browser session limit reached; close a session first");
        let mut command = self.command();
        command.args(["--input-type=module", "-e", include_str!("bridge.mjs")])
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        #[cfg(unix)] { use std::os::unix::process::CommandExt; command.as_std_mut().process_group(0); }
        let mut child = command.spawn().map_err(|error| anyhow::anyhow!("cannot start browser bridge: {error}"))?;
        let owned = Arc::new(OwnedProcesses {bridge: child.id().unwrap() as i32, browser: AtomicI32::new(0), active: AtomicBool::new(true)});
        let mut errors = child.stderr.take().unwrap();
        let stderr = Arc::new(std::sync::Mutex::new(String::new()));
        let stderr_copy = stderr.clone();
        tokio::spawn(async move {
            let mut buffer = [0u8; 2048];
            while let Ok(count) = errors.read(&mut buffer).await {
                if count == 0 { break; }
                if let Ok(mut captured) = stderr_copy.lock() {
                    if captured.len() < 8000 { captured.push_str(&String::from_utf8_lossy(&buffer[..count])); }
                }
            }
        });
        let id = Uuid::new_v4();
        let mut session = Session {input: child.stdin.take().unwrap(), output: BufReader::new(child.stdout.take().unwrap()),
            child, owned, stderr, state: BrowserState {id, current_url: "about:blank".into(), title: String::new(), active: true, opened_at: Utc::now()}};
        let started = session.request(serde_json::json!({"op":"start", "config": self.config})).await?;
        let browser_pid = started["browser_pid"].as_i64().filter(|pid| *pid > 0 && *pid <= i32::MAX as i64)
            .ok_or_else(|| anyhow::anyhow!("browser bridge returned no owned process identifier"))?;
        session.owned.browser.store(browser_pid as i32, Ordering::SeqCst);
        session.request(serde_json::json!({"op":"act", "action":{"action":"navigate", "url":url}})).await?;
        self.sessions.write().await.insert(id, Arc::new(SessionHandle {owned: session.owned.clone(), inner: tokio::sync::Mutex::new(session)}));
        Ok(id)
    }
    async fn act(&self, id: BrowserSessionId, action: BrowserAction) -> anyhow::Result<BrowserArtifact> {
        let session = self.session(id).await?;
        let mut session = session.inner.lock().await;
        let screenshot_path = self.config.artifact_dir.join(format!("{id}-{}.png", Uuid::new_v4()));
        let result = session.request(serde_json::json!({"op":"act", "action":action, "screenshot_path":screenshot_path})).await?;
        Ok(serde_json::from_value(result)?)
    }
    async fn screenshot(&self, id: BrowserSessionId) -> anyhow::Result<BrowserEvidence> {
        match self.act(id, BrowserAction::Screenshot).await? {
            BrowserArtifact::Screenshot {uri, captured_at} => {
                let bytes = std::fs::read(&uri)?;
                anyhow::ensure!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"), "browser did not produce a PNG screenshot");
                Ok(BrowserEvidence {kind: "screenshot".into(), uri, label: "Chrome screenshot".into(), captured_at})
            },
            _ => anyhow::bail!("browser did not return a screenshot"),
        }
    }
    async fn state(&self, id: BrowserSessionId) -> anyhow::Result<BrowserState> {
        let session = self.session(id).await?;
        let mut session = session.inner.lock().await;
        if session.owned.active.load(Ordering::SeqCst) {
            let value = session.request(serde_json::json!({"op":"state"})).await?;
            session.state.current_url = value["current_url"].as_str().unwrap_or("").into();
            session.state.title = value["title"].as_str().unwrap_or("").into();
            session.state.active = value["active"] == true;
        } else { session.state.active = false; }
        Ok(session.state.clone())
    }
    async fn close(&self, id: BrowserSessionId) -> anyhow::Result<()> {
        let handle = self.session(id).await?;
        // An in-flight action must not keep close waiting for its timeout.
        if handle.inner.try_lock().is_err() { handle.owned.stop(); }
        let mut session = handle.inner.lock().await;
        if session.owned.active.load(Ordering::SeqCst) {
            let result = session.request(serde_json::json!({"op":"close"})).await;
            if result.is_ok() { session.owned.browser.store(0, Ordering::SeqCst); }
            session.owned.stop();
            tokio::time::timeout(std::time::Duration::from_secs(3), session.child.wait()).await
                .map_err(|_| anyhow::anyhow!("owned browser bridge did not stop in time"))??;
            session.state.active = false;
            result?;
        } else {
            tokio::time::timeout(std::time::Duration::from_secs(3), session.child.wait()).await
                .map_err(|_| anyhow::anyhow!("owned browser bridge did not stop in time"))??;
            session.state.active = false;
        }
        Ok(())
    }
    async fn list_sessions(&self) -> anyhow::Result<Vec<BrowserSessionId>> {
        let sessions: Vec<_> = self.sessions.read().await.iter().map(|(id, session)| (*id, session.clone())).collect();
        let mut ids = Vec::new();
        for (id, session) in sessions { if session.owned.active.load(Ordering::SeqCst) { ids.push(id); } }
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn artifact_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!("chimera-browser-{}", Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        path
    }
    #[test]
    fn missing_dependencies_fail_before_opening_a_session() {
        let config = BrowserConfig {node: "/no-such-node".into(), automation_module: "/no-such-module".into(), executable_path: "/no-such-chrome".into(), artifact_dir: artifact_dir(), allowed_origins: vec![]};
        assert!(PuppeteerBrowser::new(config).is_err());
    }
    #[tokio::test]
    #[ignore = "requires installed Chrome and Puppeteer; run explicitly for real browser verification"]
    async fn real_browser_page_actions_screenshot_and_close() {
        let browser = PuppeteerBrowser::new(BrowserConfig::discover(artifact_dir()).unwrap()).unwrap();
        let page = "data:text/html,<title>CHIMERA browser check</title><style>body{font:24px sans-serif;padding:40px}button{font-size:24px}</style><h1>Browser check</h1><input id=name><button id=go onclick=\"document.querySelector('h1').innerText='Hello '+document.querySelector('input').value;console.log('clicked')\">Greet</button>";
        let id = browser.open(BrowserTarget::Url(page.into())).await.unwrap();
        browser.act(id, BrowserAction::Type {selector: "#name".into(), text: "Floyd".into()}).await.unwrap();
        browser.act(id, BrowserAction::Click {selector: "#go".into()}).await.unwrap();
        let text = browser.act(id, BrowserAction::ReadText {selector: Some("h1".into())}).await.unwrap();
        assert!(matches!(text, BrowserArtifact::Text {content} if content == "Hello Floyd"));
        assert_eq!(browser.state(id).await.unwrap().title, "CHIMERA browser check");
        let screenshot = browser.screenshot(id).await.unwrap();
        assert!(std::fs::metadata(&screenshot.uri).unwrap().len() > 1000);
        println!("REAL_BROWSER_SCREENSHOT={}", screenshot.uri);
        let console = browser.act(id, BrowserAction::ReadConsole).await.unwrap();
        assert!(matches!(console, BrowserArtifact::Console {messages} if messages.iter().any(|m| m.text == "clicked")));
        assert!(browser.act(id, BrowserAction::Navigate {url: "https://example.com".into()}).await.is_err());
        browser.close(id).await.unwrap();
        assert!(!browser.state(id).await.unwrap().active);
        assert!(!browser.list_sessions().await.unwrap().contains(&id));
        assert!(browser.act(id, BrowserAction::ReadConsole).await.is_err());
    }
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "requires installed Chrome and Puppeteer; run explicitly for real browser verification"]
    async fn cancelling_action_stops_owned_browser() {
        let browser = Arc::new(PuppeteerBrowser::new(BrowserConfig::discover(artifact_dir()).unwrap()).unwrap());
        let id = browser.open(BrowserTarget::Blank).await.unwrap();
        let (_, chrome_pid) = browser.process_ids(id).await.unwrap();
        let worker = browser.clone();
        let task = tokio::spawn(async move { worker.act(id, BrowserAction::EvalJs {script: "new Promise(() => {})".into()}).await });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        task.abort(); let _ = task.await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        unsafe extern "C" { fn kill(pid: i32, signal: i32) -> i32; }
        assert_ne!(unsafe { kill(chrome_pid as i32, 0) }, 0, "owned browser survived cancellation");
        assert!(!browser.state(id).await.unwrap().active);
    }
    #[tokio::test]
    #[ignore = "requires installed Chrome and Puppeteer; run explicitly for real browser verification"]
    async fn closing_interrupts_an_active_browser_action() {
        let browser = Arc::new(PuppeteerBrowser::new(BrowserConfig::discover(artifact_dir()).unwrap()).unwrap());
        let id = browser.open(BrowserTarget::Blank).await.unwrap();
        let worker = browser.clone();
        let task = tokio::spawn(async move { worker.act(id, BrowserAction::EvalJs {script: "new Promise(() => {})".into()}).await });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        tokio::time::timeout(std::time::Duration::from_secs(3), browser.close(id)).await.unwrap().unwrap();
        assert!(task.await.unwrap().is_err());
        assert!(!browser.state(id).await.unwrap().active);
    }
}
