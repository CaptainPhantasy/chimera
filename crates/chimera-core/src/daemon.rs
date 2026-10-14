//! One local service per project. No network listener and no automatic tool approval.
use crate::scheduler::{FileWorkflowScheduler, JobClaim, JobOutcome, JobRunStatus, WorkflowStatus};
use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::watch;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonAction {
    Status,
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub running: bool,
    pub stopping: bool,
    pub pid: u32,
    pub project: PathBuf,
    pub active_run: Option<JobClaim>,
    pub jobs: usize,
    pub recovered_interrupted_runs: usize,
}

/// Runs a trusted executable. This is not a shell command parser or an approval grant.
/// Production uses `current_executable`; `controlled` is for owner-provided children.
#[derive(Debug, Clone)]
pub struct CommandJobRunner {
    executable: PathBuf,
    prefix: Vec<String>,
    chimera: bool,
    timeout_secs: u64,
}
impl CommandJobRunner {
    pub fn current_executable() -> anyhow::Result<Self> {
        Ok(Self {
            executable: std::env::current_exe()?,
            prefix: Vec::new(),
            chimera: true,
            timeout_secs: 900,
        })
    }
    pub fn controlled(
        executable: impl Into<PathBuf>,
        prefix: Vec<String>,
        timeout_secs: u64,
    ) -> anyhow::Result<Self> {
        ensure!(
            (1..=3600).contains(&timeout_secs),
            "job timeout must be between 1 and 3600 seconds"
        );
        let executable = executable
            .into()
            .canonicalize()
            .context("job executable is unavailable")?;
        ensure!(executable.is_file(), "job executable is not a file");
        Ok(Self {
            executable,
            prefix,
            chimera: false,
            timeout_secs,
        })
    }
    pub fn with_timeout(mut self, timeout_secs: u64) -> anyhow::Result<Self> {
        ensure!(
            (1..=3600).contains(&timeout_secs),
            "job timeout must be between 1 and 3600 seconds"
        );
        self.timeout_secs = timeout_secs;
        Ok(self)
    }
    pub async fn run(
        &self,
        root: &Path,
        claim: &JobClaim,
        cancellation: watch::Receiver<bool>,
    ) -> anyhow::Result<JobOutcome> {
        #[cfg(unix)]
        {
            unix::run_child(self, root, claim, cancellation).await
        }
        #[cfg(not(unix))]
        {
            let _ = (root, claim, cancellation);
            anyhow::bail!("owned background children are unavailable on this platform")
        }
    }
}

/// Serve until an authenticated Stop request or Ctrl+C. Serial execution avoids
/// concurrent scheduled writers; each child still uses the normal permission router.
pub async fn serve(root: impl AsRef<Path>, runner: CommandJobRunner) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        unix::serve(root.as_ref(), runner).await
    }
    #[cfg(not(unix))]
    {
        let _ = (root, runner);
        anyhow::bail!("local background service requires Unix sockets")
    }
}

/// A missing service returns an error, never a fabricated "running" response.
pub async fn request(root: impl AsRef<Path>, action: DaemonAction) -> anyhow::Result<DaemonStatus> {
    #[cfg(unix)]
    {
        unix::request(root.as_ref(), action).await
    }
    #[cfg(not(unix))]
    {
        let _ = (root, action);
        anyhow::bail!("local background service requires Unix sockets")
    }
}

/// The validated, short control address. The project keeps its private binding record.
pub fn control_socket_path(root: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
    #[cfg(unix)]
    {
        unix::control_socket_path(root.as_ref())
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        anyhow::bail!("local background service requires Unix sockets")
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::fs::{File, OpenOptions};
    use std::io::Write;
    use std::os::unix::fs::{
        DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt,
    };
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
    use tokio::net::{UnixListener, UnixStream};
    use uuid::Uuid;

    const OUTPUT_LIMIT: usize = 65_536;
    const MESSAGE_LIMIT: usize = 131_072;
    fn uid() -> u32 {
        unsafe extern "C" {
            fn geteuid() -> u32;
        }
        unsafe { geteuid() }
    }
    fn private_dir(path: &Path, create: bool) -> anyhow::Result<()> {
        if create && !path.exists() {
            match std::fs::DirBuilder::new().mode(0o700).create(path) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e.into()),
            }
        }
        let meta = std::fs::symlink_metadata(path)
            .with_context(|| format!("service directory is unavailable: {}", path.display()))?;
        ensure!(
            meta.is_dir() && !meta.file_type().is_symlink() && meta.uid() == uid(),
            "unsafe service directory: {}",
            path.display()
        );
        if create {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        }
        ensure!(
            std::fs::metadata(path)?.mode() & 0o077 == 0,
            "service directory is not private"
        );
        Ok(())
    }
    fn paths(root: &Path, create: bool) -> anyhow::Result<(PathBuf, PathBuf)> {
        let root = root.canonicalize().context("project is unavailable")?;
        private_dir(&root.join(".chimera"), create)?;
        let directory = root.join(".chimera/daemon");
        private_dir(&directory, create)?;
        Ok((root, directory))
    }
    fn owner_file(path: &Path) -> anyhow::Result<()> {
        let meta = std::fs::symlink_metadata(path)?;
        ensure!(
            meta.is_file()
                && !meta.file_type().is_symlink()
                && meta.uid() == uid()
                && meta.mode() & 0o077 == 0,
            "unsafe service file: {}",
            path.display()
        );
        Ok(())
    }
    fn archive(path: &Path) -> anyhow::Result<()> {
        let destination = path.with_file_name(format!(
            "{}.retired-{}",
            path.file_name()
                .context("invalid service path")?
                .to_string_lossy(),
            Uuid::new_v4()
        ));
        std::fs::rename(path, destination)?;
        File::open(path.parent().context("service path has no parent")?)?.sync_all()?;
        Ok(())
    }
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Endpoint {
        format_version: u32,
        id: Uuid,
        project: PathBuf,
        owner: u32,
    }
    fn runtime_dir(create: bool) -> anyhow::Result<PathBuf> {
        // Do not use TMPDIR: macOS user temporary paths can themselves exceed SUN_LEN.
        let directory = PathBuf::from("/tmp")
            .canonicalize()?
            .join(format!("chimera-control-{}", uid()));
        private_dir(&directory, create)?;
        Ok(directory)
    }
    fn read_endpoint(path: &Path) -> anyhow::Result<Endpoint> {
        owner_file(path)?;
        ensure!(
            std::fs::metadata(path)?.len() <= 16_384,
            "service endpoint record is too large"
        );
        let endpoint: Endpoint = serde_json::from_slice(&std::fs::read(path)?)
            .context("corrupt service endpoint record")?;
        ensure!(
            endpoint.format_version == 1 && endpoint.owner == uid(),
            "service endpoint version or owner mismatch"
        );
        Ok(endpoint)
    }
    fn write_new_record(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
        let parent = path.parent().context("endpoint has no parent")?;
        let pending = parent.join(format!(".endpoint-pending-{}", Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&pending)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        // Create-only install cannot overwrite a different mapping, even if raced.
        std::fs::hard_link(&pending, path)?;
        File::open(parent)?.sync_all()?;
        archive(&pending)?;
        Ok(())
    }
    fn endpoint_socket(root: &Path, directory: &Path, create: bool) -> anyhow::Result<PathBuf> {
        let runtime = runtime_dir(create)?;
        let project_record = directory.join("endpoint.json");
        let endpoint = if std::fs::symlink_metadata(&project_record).is_ok() {
            read_endpoint(&project_record)?
        } else {
            ensure!(create, "background service has no endpoint record");
            let endpoint = Endpoint {
                format_version: 1,
                id: Uuid::new_v4(),
                project: root.into(),
                owner: uid(),
            };
            let bytes = serde_json::to_vec(&endpoint)?;
            write_new_record(
                &runtime.join(format!("{}.json", endpoint.id.simple())),
                &bytes,
            )?;
            write_new_record(&project_record, &bytes)?;
            endpoint
        };
        ensure!(
            endpoint.project == root,
            "service endpoint belongs to a different project"
        );
        let binding = runtime.join(format!("{}.json", endpoint.id.simple()));
        if create
            && std::fs::symlink_metadata(&binding)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        {
            // The OS may clear /tmp during reboot; restore only this project's immutable binding.
            write_new_record(&binding, &serde_json::to_vec(&endpoint)?)?;
        }
        ensure!(
            read_endpoint(&binding)? == endpoint,
            "short service address belongs to a different project"
        );
        Ok(runtime.join(format!("{}.sock", endpoint.id.simple())))
    }
    pub(super) fn control_socket_path(root: &Path) -> anyhow::Result<PathBuf> {
        let (root, directory) = paths(root, false)?;
        endpoint_socket(&root, &directory, false)
    }
    struct ServiceLease {
        _file: File,
        socket: PathBuf,
        socket_inode: u64,
    }
    impl Drop for ServiceLease {
        fn drop(&mut self) {
            if std::fs::symlink_metadata(&self.socket).is_ok_and(|m| {
                m.file_type().is_socket() && m.ino() == self.socket_inode && m.uid() == uid()
            }) {
                let _ = archive(&self.socket);
            }
        }
    }
    fn bind(root: &Path, directory: &Path) -> anyhow::Result<(ServiceLease, UnixListener, String)> {
        let lock_path = directory.join("service.lock");
        if lock_path.exists() {
            owner_file(&lock_path)?;
        } else if std::fs::symlink_metadata(&lock_path).is_ok() {
            anyhow::bail!("unsafe service lock");
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(&lock_path)?;
        owner_file(&lock_path)?;
        lock.try_lock()
            .map_err(|e| anyhow::anyhow!("another service holds the project lease: {e}"))?;
        let socket = endpoint_socket(root, directory, true)?;
        if let Ok(meta) = std::fs::symlink_metadata(&socket) {
            ensure!(
                meta.file_type().is_socket() && meta.uid() == uid(),
                "service socket path is occupied by an unsafe object"
            );
            match std::os::unix::net::UnixStream::connect(&socket) {
                Ok(_) => anyhow::bail!("service socket is occupied by a live listener"),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                    ) =>
                {
                    archive(&socket)?
                }
                Err(e) => return Err(e).context("cannot determine whether the socket is occupied"),
            }
        }
        let listener = UnixListener::bind(&socket)
            .inspect_err(|e| tracing::error!("local socket bind failed: {e}"))
            .with_context(|| {
                format!(
                    "cannot bind local socket {} (Unix socket path length is limited)",
                    socket.display()
                )
            })?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        let lease = ServiceLease {
            _file: lock,
            socket: socket.clone(),
            socket_inode: std::fs::symlink_metadata(&socket)?.ino(),
        };
        let token_path = directory.join("token");
        if std::fs::symlink_metadata(&token_path).is_ok() {
            owner_file(&token_path)?;
            archive(&token_path)?;
        }
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let mut token_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(token_path)?;
        token_file.write_all(token.as_bytes())?;
        token_file.sync_all()?;
        File::open(directory)?.sync_all()?;
        Ok((lease, listener, token))
    }
    #[derive(Serialize, Deserialize)]
    struct ControlRequest {
        token: String,
        action: DaemonAction,
    }
    #[derive(Serialize, Deserialize)]
    struct ControlReply {
        status: Option<DaemonStatus>,
        error: Option<String>,
    }
    async fn read_message(stream: &mut UnixStream) -> anyhow::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        loop {
            let byte = stream
                .read_u8()
                .await
                .context("incomplete service message")?;
            if byte == b'\n' {
                return Ok(bytes);
            }
            ensure!(bytes.len() < MESSAGE_LIMIT, "service message exceeds limit");
            bytes.push(byte);
        }
    }
    fn same_token(a: &str, b: &str) -> bool {
        if a.len() != b.len() {
            return false;
        }
        a.bytes()
            .zip(b.bytes())
            .fold(0u8, |different, (a, b)| different | (a ^ b))
            == 0
    }
    async fn handle(
        mut stream: UnixStream,
        token: &str,
        mut status: DaemonStatus,
    ) -> anyhow::Result<bool> {
        ensure!(
            stream.peer_cred()?.uid() == uid(),
            "service peer has a different owner"
        );
        let request = tokio::time::timeout(Duration::from_secs(2), read_message(&mut stream))
            .await
            .context("service request timed out")??;
        let request = serde_json::from_slice::<ControlRequest>(&request);
        let mut stop = false;
        let reply = match request {
            Ok(request) if same_token(&request.token, token) => {
                stop = matches!(request.action, DaemonAction::Stop);
                if stop {
                    status.stopping = true;
                }
                ControlReply {
                    status: Some(status),
                    error: None,
                }
            }
            _ => ControlReply {
                status: None,
                error: Some("service authentication failed".into()),
            },
        };
        let mut bytes = serde_json::to_vec(&reply)?;
        bytes.push(b'\n');
        tokio::time::timeout(Duration::from_secs(2), stream.write_all(&bytes))
            .await
            .context("service reply timed out")??;
        Ok(stop)
    }
    pub(super) async fn request(root: &Path, action: DaemonAction) -> anyhow::Result<DaemonStatus> {
        let (root, directory) = paths(root, false)?;
        let token_path = directory.join("token");
        owner_file(&token_path)?;
        let token = std::fs::read_to_string(token_path)?;
        ensure!(
            token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid local service token"
        );
        let socket = endpoint_socket(&root, &directory, false)?;
        let meta =
            std::fs::symlink_metadata(&socket).context("background service is not running")?;
        ensure!(
            meta.file_type().is_socket() && meta.uid() == uid() && meta.mode() & 0o077 == 0,
            "unsafe service socket"
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut stream = UnixStream::connect(socket)
                .await
                .context("background service is unavailable")?;
            ensure!(
                stream.peer_cred()?.uid() == uid(),
                "service belongs to a different owner"
            );
            let mut request = serde_json::to_vec(&ControlRequest { token, action })?;
            request.push(b'\n');
            stream.write_all(&request).await?;
            let reply: ControlReply = serde_json::from_slice(&read_message(&mut stream).await?)?;
            if let Some(error) = reply.error {
                anyhow::bail!("{error}");
            }
            let status = reply.status.context("service returned no status")?;
            ensure!(status.project == root, "service project identity mismatch");
            Ok(status)
        })
        .await
        .context("background service request timed out")?
    }
    struct ActiveRun {
        claim: JobClaim,
        cancel: watch::Sender<bool>,
        task: tokio::task::JoinHandle<anyhow::Result<JobOutcome>>,
    }
    impl Drop for ActiveRun {
        fn drop(&mut self) {
            let _ = self.cancel.send(true);
            self.task.abort();
        }
    }
    pub(super) async fn serve(root: &Path, runner: CommandJobRunner) -> anyhow::Result<()> {
        let (root, directory) = paths(root, true)?;
        let (_lease, listener, token) = bind(&root, &directory)?;
        let scheduler = FileWorkflowScheduler::new(&root);
        let recovered = scheduler.recover_interrupted()?;
        let mut active: Option<ActiveRun> = None;
        let mut stopping = false;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if stopping && active.is_none() {
                return Ok(());
            }
            tokio::select! {
                signal = tokio::signal::ctrl_c(), if !stopping => { signal?; stopping = true; if let Some(run) = &active { let _ = run.cancel.send(true); } }
                connection = listener.accept() => {
                    let (stream, _) = connection?;
                    let status = DaemonStatus { running: true, stopping, pid: std::process::id(), project: root.clone(), active_run: active.as_ref().map(|a| a.claim.clone()), jobs: scheduler.all()?.len(), recovered_interrupted_runs: recovered };
                    match handle(stream, &token, status).await {
                        Ok(true) => { stopping = true; if let Some(run) = &active { let _ = run.cancel.send(true); } }
                        Ok(false) => (),
                        Err(error) => tracing::warn!("local service request rejected: {error:#}"),
                    }
                }
                result = async { match &mut active { Some(run) => (&mut run.task).await, None => std::future::pending().await } } => {
                    let run = active.take().context("completed child without a claim")?;
                    let outcome = match result {
                        Ok(Ok(outcome)) => outcome,
                        Ok(Err(error)) => JobOutcome { status: JobRunStatus::Failed, exit_code: None, stdout: String::new(), stderr: bounded_text(&format!("child failed: {error:#}")), output_truncated: false },
                        Err(error) => JobOutcome { status: JobRunStatus::Interrupted, exit_code: None, stdout: String::new(), stderr: format!("child task interrupted; effects may have happened: {error}"), output_truncated: false },
                    };
                    scheduler.finish(&run.claim, outcome)?;
                }
                _ = tick.tick() => {
                    if let Some(run) = &active {
                        if scheduler.details(run.claim.workflow_id)?.handle.status != WorkflowStatus::Active { let _ = run.cancel.send(true); }
                    } else if !stopping {
                        if let Some(claim) = scheduler.claim_next(chrono::Utc::now())? {
                            let (cancel, receiver) = watch::channel(false);
                            let runner = runner.clone(); let project = root.clone(); let child_claim = claim.clone();
                            let task = tokio::spawn(async move { runner.run(&project, &child_claim, receiver).await });
                            active = Some(ActiveRun { claim, cancel, task });
                        }
                    }
                }
            }
        }
    }
    struct ProcessGroup(i32);
    impl ProcessGroup {
        fn signal(&self, signal: i32) {
            unsafe extern "C" {
                fn kill(pid: i32, signal: i32) -> i32;
            }
            unsafe {
                kill(-self.0, signal);
            }
        }
    }
    impl Drop for ProcessGroup {
        fn drop(&mut self) {
            self.signal(9);
        }
    }
    #[derive(Default)]
    struct Output {
        bytes: Vec<u8>,
        total: u64,
    }
    async fn drain(
        reader: &mut (impl AsyncRead + Unpin),
        output: &mut Output,
    ) -> std::io::Result<()> {
        let mut buf = [0; 8192];
        loop {
            let n = reader.read(&mut buf).await?;
            if n == 0 {
                return Ok(());
            }
            output.total += n as u64;
            let keep = n.min(OUTPUT_LIMIT.saturating_sub(output.bytes.len()));
            output.bytes.extend_from_slice(&buf[..keep]);
        }
    }
    fn bounded_text(text: &str) -> String {
        let mut end = text.len().min(OUTPUT_LIMIT);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].into()
    }
    fn output_text(output: &Output) -> String {
        bounded_text(&String::from_utf8_lossy(&output.bytes))
    }
    pub(super) async fn run_child(
        runner: &CommandJobRunner,
        root: &Path,
        claim: &JobClaim,
        mut cancellation: watch::Receiver<bool>,
    ) -> anyhow::Result<JobOutcome> {
        if *cancellation.borrow() {
            return Ok(JobOutcome {
                status: JobRunStatus::Cancelled,
                exit_code: None,
                stdout: String::new(),
                stderr: "cancelled before launch".into(),
                output_truncated: false,
            });
        }
        let root = root.canonicalize()?;
        let mut cmd = tokio::process::Command::new(&runner.executable);
        cmd.args(&runner.prefix);
        if runner.chimera {
            cmd.args(["--format", "json"]);
            if let Some(pack) = &claim.spec.pack {
                cmd.arg("pack")
                    .arg(pack)
                    .arg("--objective")
                    .arg(&claim.spec.objective);
            } else {
                cmd.arg("run").arg(&claim.spec.objective);
            }
        } else {
            cmd.arg(&claim.spec.objective);
        }
        // The actual CLI owns provider credentials/config and exact-call approval.
        // Null input prevents a scheduled run from consuming a foreground terminal answer.
        cmd.current_dir(&root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("CHIMERA_NON_INTERACTIVE", "1")
            .env("CHIMERA_WAIT_APPROVAL", "1")
            .env("CHIMERA_JOB_ID", claim.workflow_id.to_string())
            .env("CHIMERA_JOB_RUN_ID", claim.run_id.to_string())
            .kill_on_drop(true);
        cmd.as_std_mut().process_group(0);
        let mut child = cmd.spawn().context("could not launch scheduled child")?;
        let group = ProcessGroup(child.id().context("child has no process id")? as i32);
        let mut stdout = child.stdout.take().context("missing child output")?;
        let mut stderr = child.stderr.take().context("missing child errors")?;
        let mut out = Output::default();
        let mut err = Output::default();
        enum End {
            Result(std::io::Result<(std::process::ExitStatus, (), ())>),
            Cancelled,
            Timeout,
        }
        let end = tokio::select! {
            biased;
            _ = async { if !*cancellation.borrow() { let _ = cancellation.changed().await; } } => End::Cancelled,
            result = async { tokio::try_join!(child.wait(), drain(&mut stdout, &mut out), drain(&mut stderr, &mut err)) } => End::Result(result),
            _ = tokio::time::sleep(Duration::from_secs(runner.timeout_secs)) => End::Timeout,
        };
        let (status, exit_code, notice) = match end {
            End::Result(result) => {
                let (status, _, _) = result?;
                (
                    if status.success() {
                        JobRunStatus::Completed
                    } else {
                        JobRunStatus::Failed
                    },
                    status.code(),
                    if status.code().is_none() {
                        format!("child ended by signal: {status}")
                    } else {
                        String::new()
                    },
                )
            }
            End::Cancelled | End::Timeout => {
                group.signal(2); // Give the real CLI a short chance to save its partial conversation.
                let settled = tokio::time::timeout(Duration::from_secs(3), async {
                    tokio::try_join!(
                        child.wait(),
                        drain(&mut stdout, &mut out),
                        drain(&mut stderr, &mut err)
                    )
                })
                .await;
                let code = if let Ok(Ok((status, _, _))) = settled {
                    status.code()
                } else {
                    group.signal(9);
                    let status = child.wait().await?;
                    let _ = tokio::time::timeout(Duration::from_millis(200), async {
                        tokio::try_join!(drain(&mut stdout, &mut out), drain(&mut stderr, &mut err))
                    })
                    .await;
                    status.code()
                };
                if matches!(end, End::Timeout) {
                    (
                        JobRunStatus::TimedOut,
                        code,
                        format!(
                            "job timed out after {} seconds; child process group stopped",
                            runner.timeout_secs
                        ),
                    )
                } else {
                    (
                        JobRunStatus::Cancelled,
                        code,
                        "job cancelled; child process group stopped".into(),
                    )
                }
            }
        };
        drop(group);
        let truncated = out.total > OUTPUT_LIMIT as u64
            || err.total > OUTPUT_LIMIT as u64
            || String::from_utf8_lossy(&out.bytes).len() > OUTPUT_LIMIT
            || String::from_utf8_lossy(&err.bytes).len() > OUTPUT_LIMIT;
        let error_text = output_text(&err);
        Ok(JobOutcome {
            status,
            exit_code,
            stdout: output_text(&out),
            stderr: bounded_text(&format!("{error_text}\n{notice}")),
            output_truncated: truncated,
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::scheduler::{ScheduleFrequency, ScheduleSpec, WorkflowScheduler};
    use chrono::Utc;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    use uuid::Uuid;
    fn root() -> PathBuf {
        let path = PathBuf::from("/tmp").join(format!("ch-d-{}", Uuid::new_v4().simple()));
        std::fs::create_dir(&path).unwrap();
        path
    }
    async fn create(root: &Path) -> JobClaim {
        let store = FileWorkflowScheduler::new(root);
        store
            .create(ScheduleSpec {
                label: "test child".into(),
                objective: "argument with spaces and $literal".into(),
                frequency: ScheduleFrequency::Once { at: Utc::now() },
                pack: None,
                enabled: true,
            })
            .await
            .unwrap();
        store.claim_next(Utc::now()).unwrap().unwrap()
    }
    fn shell(script: &str, timeout: u64) -> CommandJobRunner {
        CommandJobRunner::controlled(
            "/bin/sh",
            vec!["-c".into(), script.into(), "test".into()],
            timeout,
        )
        .unwrap()
    }
    async fn wait_until(mut check: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(8), async {
            while !check() {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
    }
    async fn ready(root: &Path) {
        let mut last_error = String::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match request(root, DaemonAction::Status).await {
                    Ok(_) => return,
                    Err(e) => last_error = format!("{e:#}"),
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|e| panic!("service readiness timed out: {e}; last error: {last_error}"));
    }
    #[tokio::test]
    async fn actual_child_records_output_failure_and_argument_boundaries() {
        let root = root();
        let claim = create(&root).await;
        let (_tx, rx) = watch::channel(false);
        let outcome = shell("printf '%s' \"$1\"; printf actual-error >&2; exit 7", 5)
            .run(&root, &claim, rx)
            .await
            .unwrap();
        assert_eq!(outcome.status, JobRunStatus::Failed);
        assert_eq!(outcome.exit_code, Some(7));
        assert_eq!(outcome.stdout, claim.spec.objective);
        assert!(outcome.stderr.contains("actual-error"));
    }
    #[tokio::test]
    async fn output_is_bounded_and_cancellation_stops_owned_child() {
        let root = root();
        let claim = create(&root).await;
        let (_tx, rx) = watch::channel(false);
        let output = shell("head -c 100000 /dev/zero", 5)
            .run(&root, &claim, rx)
            .await
            .unwrap();
        assert_eq!(output.status, JobRunStatus::Completed);
        assert!(output.output_truncated);
        assert_eq!(output.stdout.len(), 65_536);
        let (_tx, rx) = watch::channel(false);
        let output = shell("head -c 50000 /dev/zero | tr '\\000' '\\377'", 5)
            .run(&root, &claim, rx)
            .await
            .unwrap();
        assert!(output.output_truncated);
        assert!(output.stdout.len() <= 65_536);
        let (tx, rx) = watch::channel(false);
        let runner = shell(
            "printf started > marker; printf partial; sleep 30; printf wrong >> marker",
            40,
        );
        let project = root.clone();
        let task = tokio::spawn(async move { runner.run(&project, &claim, rx).await });
        wait_until(|| root.join("marker").exists()).await;
        tx.send(true).unwrap();
        let output = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(output.status, JobRunStatus::Cancelled);
        assert_eq!(output.stdout, "partial");
        assert_eq!(
            std::fs::read_to_string(root.join("marker")).unwrap(),
            "started"
        );
    }
    #[tokio::test]
    async fn timeout_is_not_reported_as_success() {
        let root = root();
        let claim = create(&root).await;
        let (_tx, rx) = watch::channel(false);
        let output = shell("printf before; sleep 30", 1)
            .run(&root, &claim, rx)
            .await
            .unwrap();
        assert_eq!(output.status, JobRunStatus::TimedOut);
        assert_eq!(output.stdout, "before");
        assert!(output.stderr.contains("timed out"));
    }
    #[tokio::test]
    async fn service_runs_marker_once_across_restart_and_requires_authentication() {
        let root = root();
        let store = FileWorkflowScheduler::new(&root);
        let id = store
            .create(ScheduleSpec {
                label: "once".into(),
                objective: "test".into(),
                frequency: ScheduleFrequency::Once { at: Utc::now() },
                pack: None,
                enabled: true,
            })
            .await
            .unwrap();
        let project = root.clone();
        let service =
            tokio::spawn(async move { serve(project, shell("printf x >> marker", 5)).await });
        ready(&root).await;
        wait_until(|| {
            store
                .details(id)
                .is_ok_and(|job| job.handle.status == WorkflowStatus::Completed)
        })
        .await;
        let mut stream = tokio::net::UnixStream::connect(control_socket_path(&root).unwrap())
            .await
            .unwrap();
        stream
            .write_all(b"{\"token\":\"wrong\",\"action\":\"stop\"}\n")
            .await
            .unwrap();
        let mut reply = String::new();
        tokio::io::BufReader::new(stream)
            .read_line(&mut reply)
            .await
            .unwrap();
        assert!(reply.contains("authentication failed"));
        assert!(request(&root, DaemonAction::Status).await.unwrap().running);
        assert!(request(&root, DaemonAction::Stop).await.unwrap().stopping);
        service.await.unwrap().unwrap();
        let project = root.clone();
        let service =
            tokio::spawn(async move { serve(project, shell("printf x >> marker", 5)).await });
        ready(&root).await;
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert_eq!(std::fs::read_to_string(root.join("marker")).unwrap(), "x");
        request(&root, DaemonAction::Stop).await.unwrap();
        service.await.unwrap().unwrap();
        assert!(request(&root, DaemonAction::Status).await.is_err());
    }
    #[tokio::test]
    async fn pause_running_job_and_service_crash_do_not_replay_effects() {
        let root = root();
        let store = FileWorkflowScheduler::new(&root);
        let id = store
            .create(ScheduleSpec {
                label: "pause".into(),
                objective: "test".into(),
                frequency: ScheduleFrequency::Once { at: Utc::now() },
                pack: None,
                enabled: true,
            })
            .await
            .unwrap();
        let project = root.clone();
        let service = tokio::spawn(async move {
            serve(
                project,
                shell("printf x >> marker; sleep 30; printf wrong >> marker", 40),
            )
            .await
        });
        ready(&root).await;
        wait_until(|| root.join("marker").exists()).await;
        store.pause(id).await.unwrap();
        wait_until(|| store.details(id).unwrap().in_flight.is_none()).await;
        assert_eq!(
            store.details(id).unwrap().handle.status,
            WorkflowStatus::Paused
        );
        assert_eq!(
            store.details(id).unwrap().runs[0].outcome.status,
            JobRunStatus::Cancelled
        );
        store.resume(id).await.unwrap();
        wait_until(|| store.details(id).unwrap().in_flight.is_some()).await;
        wait_until(|| std::fs::read_to_string(root.join("marker")).unwrap() == "xx").await;
        service.abort();
        assert!(service.await.unwrap_err().is_cancelled());
        let project = root.clone();
        let service =
            tokio::spawn(async move { serve(project, shell("printf x >> marker", 5)).await });
        ready(&root).await;
        assert_eq!(
            request(&root, DaemonAction::Status)
                .await
                .unwrap()
                .recovered_interrupted_runs,
            1
        );
        assert_eq!(
            store.details(id).unwrap().runs[1].outcome.status,
            JobRunStatus::Interrupted
        );
        assert_eq!(std::fs::read_to_string(root.join("marker")).unwrap(), "xx");
        request(&root, DaemonAction::Stop).await.unwrap();
        service.await.unwrap().unwrap();
    }
    #[tokio::test]
    async fn service_process_entry() {
        let Ok(root) = std::env::var("CHIMERA_DAEMON_TEST_ROOT") else {
            return;
        };
        serve(root, shell("printf x >> marker; while [ ! -f release ]; do sleep 0.05; done; printf done > child-done", 20)).await.unwrap();
    }
    #[tokio::test]
    async fn terminated_service_process_recovers_ambiguous_claim_without_second_effect() {
        let root = root();
        let store = FileWorkflowScheduler::new(&root);
        let id = store
            .create(ScheduleSpec {
                label: "crash".into(),
                objective: "test".into(),
                frequency: ScheduleFrequency::Once { at: Utc::now() },
                pack: None,
                enabled: true,
            })
            .await
            .unwrap();
        let mut process = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "daemon::tests::service_process_entry",
                "--nocapture",
            ])
            .env("CHIMERA_DAEMON_TEST_ROOT", &root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        ready(&root).await;
        wait_until(|| root.join("marker").exists()).await;
        process.kill().await.unwrap();
        // SIGKILL cannot run the service's cleanup code. Let the controlled test child
        // exit explicitly; production reports this uncertainty and never replays effects.
        std::fs::write(root.join("release"), "stop controlled child").unwrap();
        wait_until(|| root.join("child-done").exists()).await;
        let project = root.clone();
        let service =
            tokio::spawn(async move { serve(project, shell("printf wrong >> marker", 5)).await });
        ready(&root).await;
        assert_eq!(
            store.details(id).unwrap().runs[0].outcome.status,
            JobRunStatus::Interrupted
        );
        assert_eq!(
            store.details(id).unwrap().handle.status,
            WorkflowStatus::Paused
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(std::fs::read_to_string(root.join("marker")).unwrap(), "x");
        request(&root, DaemonAction::Stop).await.unwrap();
        service.await.unwrap().unwrap();
    }
    #[tokio::test]
    async fn long_unicode_projects_use_short_stable_owner_bound_addresses() {
        use std::os::unix::fs::PermissionsExt;
        let mut root = root();
        for _ in 0..3 {
            root.push("長いプロジェクト名".repeat(5));
        }
        std::fs::create_dir_all(&root).unwrap();
        assert!(root.as_os_str().len() > 300);
        let project = root.clone();
        let service = tokio::spawn(async move { serve(project, shell("true", 5)).await });
        ready(&root).await;
        let first_address = control_socket_path(&root).unwrap();
        assert!(first_address.as_os_str().len() < 100);
        assert!(!first_address.starts_with(&root));
        assert_eq!(
            std::fs::metadata(first_address.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&first_address)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            request(&root, DaemonAction::Status).await.unwrap().project,
            root.canonicalize().unwrap()
        );
        request(&root, DaemonAction::Stop).await.unwrap();
        service.await.unwrap().unwrap();
        let project = root.clone();
        let service = tokio::spawn(async move { serve(project, shell("true", 5)).await });
        ready(&root).await;
        assert_eq!(control_socket_path(&root).unwrap(), first_address);
        request(&root, DaemonAction::Stop).await.unwrap();
        service.await.unwrap().unwrap();
        let endpoint_path = root.join(".chimera/daemon/endpoint.json");
        let original = std::fs::read(&endpoint_path).unwrap();
        let mut endpoint: serde_json::Value = serde_json::from_slice(&original).unwrap();
        endpoint["owner"] = serde_json::json!(endpoint["owner"].as_u64().unwrap() + 1);
        std::fs::write(&endpoint_path, serde_json::to_vec(&endpoint).unwrap()).unwrap();
        assert!(control_socket_path(&root)
            .unwrap_err()
            .to_string()
            .contains("owner mismatch"));
        std::fs::write(&endpoint_path, &original).unwrap();
        std::fs::set_permissions(&endpoint_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(control_socket_path(&root)
            .unwrap_err()
            .to_string()
            .contains("unsafe service file"));
        std::fs::set_permissions(&endpoint_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let other = root.with_file_name(format!("other-project-{}", Uuid::new_v4()));
        std::fs::create_dir(&other).unwrap();
        chimera_memory::storage::ProjectRecords::new(&other, "daemon")
            .path("unused")
            .unwrap();
        std::fs::copy(&endpoint_path, other.join(".chimera/daemon/endpoint.json")).unwrap();
        assert!(control_socket_path(&other)
            .unwrap_err()
            .to_string()
            .contains("different project"));
        // A symlink in place of the private mapping is rejected without following it.
        let third = root.with_file_name(format!("symlink-project-{}", Uuid::new_v4()));
        std::fs::create_dir(&third).unwrap();
        chimera_memory::storage::ProjectRecords::new(&third, "daemon")
            .path("unused")
            .unwrap();
        std::os::unix::fs::symlink(&endpoint_path, third.join(".chimera/daemon/endpoint.json"))
            .unwrap();
        assert!(control_socket_path(&third)
            .unwrap_err()
            .to_string()
            .contains("unsafe service file"));
    }
    #[tokio::test]
    async fn occupied_socket_and_duplicate_service_are_errors_and_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let root = root();
        let project = root.clone();
        let service = tokio::spawn(async move {
            serve(project, shell("true", 5))
                .await
                .inspect_err(|e| eprintln!("test service failed: {e:#}"))
        });
        ready(&root).await;
        assert!(serve(&root, shell("true", 5))
            .await
            .unwrap_err()
            .to_string()
            .contains("lease"));
        for name in ["token", "endpoint.json", "service.lock"] {
            assert_eq!(
                std::fs::metadata(root.join(".chimera/daemon").join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        request(&root, DaemonAction::Stop).await.unwrap();
        service.await.unwrap().unwrap();
        let _listener =
            tokio::net::UnixListener::bind(control_socket_path(&root).unwrap()).unwrap();
        assert!(serve(&root, shell("true", 5))
            .await
            .unwrap_err()
            .to_string()
            .contains("live listener"));
    }
}
