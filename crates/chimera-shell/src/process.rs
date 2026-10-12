//! Bounded, owned subprocess execution. Dropping the future kills its process group.
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt};

const OUTPUT_LIMIT: usize = 50_000;

#[cfg(unix)]
struct ProcessGroup(i32);
#[cfg(unix)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        unsafe extern "C" { fn kill(pid: i32, signal: i32) -> i32; }
        // Only the new child group created below is addressed, never the caller's group.
        unsafe { kill(-self.0, 9); }
    }
}

async fn drain_bounded<R: AsyncRead + Unpin>(mut reader: R) -> std::io::Result<String> {
    let mut retained = Vec::with_capacity(OUTPUT_LIMIT);
    let mut chunk = [0u8; 8192];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 { break; }
        total += n as u64;
        let keep = n.min(OUTPUT_LIMIT.saturating_sub(retained.len()));
        retained.extend_from_slice(&chunk[..keep]);
    }
    let mut text = String::from_utf8_lossy(&retained).into_owned();
    if total > retained.len() as u64 {
        text.push_str(&format!("\n[truncated: {total} bytes received]"));
    }
    Ok(text)
}

/// Execute inside the project with no network and no inherited secrets.
/// macOS uses the native operating-system sandbox. Other platforms fail closed.
pub async fn run_scoped_command(
    root: &Path,
    cwd: &Path,
    command: &str,
    env: &HashMap<String, String>,
    timeout_secs: u64,
) -> anyhow::Result<crate::ShellOutput> {
    run_scoped_command_with_access(root, cwd, command, env, timeout_secs, true).await
}

/// Same process ownership, with an operating-system read-only filesystem option.
pub async fn run_scoped_command_with_access(
    root: &Path,
    cwd: &Path,
    command: &str,
    env: &HashMap<String, String>,
    timeout_secs: u64,
    writable: bool,
) -> anyhow::Result<crate::ShellOutput> {
    let root = root.canonicalize()?;
    let cwd = cwd.canonicalize()?;
    anyhow::ensure!(cwd.starts_with(&root), "command directory is outside the allowed project");
    anyhow::ensure!((1..=600).contains(&timeout_secs), "command timeout must be between 1 and 600 seconds");
    // Overrides cannot reintroduce credentials, startup scripts, or loader injection.
    for key in env.keys() {
        anyhow::ensure!(matches!(key.as_str(), "LANG" | "LC_ALL" | "TERM" | "NO_COLOR"),
            "environment override is not allowed: {key}");
    }
    #[cfg(not(target_os = "macos"))]
    anyhow::bail!("isolated shell execution is unavailable on this platform; no command was started");

    #[cfg(target_os = "macos")]
    {
        use std::os::unix::process::CommandExt;
        let root_text = root.to_str().ok_or_else(|| anyhow::anyhow!("project path is not valid UTF-8"))?;
        let root_quoted = serde_json::to_string(root_text)?;
        let protected = r#"(regex #"(^|/)(\.env($|\.)|\.ssh(/|$)|\.aws(/|$)|\.codex(/|$)|\.supercache(/|$))")"#;
        let private_state = serde_json::to_string(&root.join(".chimera").to_string_lossy())?;
        let write_rule = if writable { format!("(allow file-write* (subpath {root_quoted}))") } else { String::new() };
        let profile = format!(r#"(version 1)
(deny default)
(import "dyld-support.sb")
(allow process*)
(allow sysctl-read)
(allow mach-lookup)
(allow file-read-metadata)
(allow file-read* file-map-executable (subpath "/private/var/db/dyld") (subpath "/private/var/db/oah") (subpath "/dev"))
(allow file-read* file-map-executable (subpath {root_quoted}) (subpath "/bin") (subpath "/sbin") (subpath "/usr/bin") (subpath "/usr/sbin") (subpath "/usr/lib") (subpath "/usr/share") (subpath "/System") (subpath "/Library/Apple") (subpath "/opt/homebrew/bin") (subpath "/opt/homebrew/Cellar") (subpath "/opt/homebrew/opt") (subpath "/opt/homebrew/lib") (literal "/dev/null") (literal "/dev/urandom") (literal "/dev/zero") (literal "/private/etc/localtime"))
{write_rule}
(allow file-write* (literal "/dev/null"))
(deny file-read* file-write* {protected} (subpath {private_state}))
(deny file-write* (regex #"(^|/)(\.git|\.agents)(/|$)"))
(deny file-write-unlink)
(deny network*)
"#);
        let mut cmd = tokio::process::Command::new("/usr/bin/sandbox-exec");
        cmd.args(["-p", &profile, "/bin/sh", "-c", command])
            .current_dir(&cwd).env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin")
            .env("HOME", &root).env("TMPDIR", &root)
            .envs(env).kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        cmd.as_std_mut().process_group(0);
        let start = Instant::now();
        let mut child = cmd.spawn()?;
        let group = ProcessGroup(child.id().ok_or_else(|| anyhow::anyhow!("child has no process identifier"))? as i32);
        let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("missing command output pipe"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow::anyhow!("missing command error pipe"))?;
        let result = tokio::time::timeout(Duration::from_secs(timeout_secs), async {
            tokio::try_join!(child.wait(), drain_bounded(stdout), drain_bounded(stderr))
        }).await;
        let duration_ms = start.elapsed().as_millis() as u64;
        drop(group);
        match result {
            Ok(Ok((status, stdout, stderr))) => Ok(crate::ShellOutput {
                exit_code: status.code().unwrap_or(-1), stdout, stderr: if status.code().is_none() { format!("{stderr}\nprocess stopped: {status}") } else { stderr }, duration_ms,
            }),
            Ok(Err(error)) => { let _ = child.wait().await; Err(error.into()) },
            Err(_) => {
                let _ = child.wait().await;
                Ok(crate::ShellOutput { exit_code: 124, stdout: String::new(),
                    stderr: format!("command timed out after {timeout_secs}s; owned processes stopped"), duration_ms })
            }
        }
    }
}
