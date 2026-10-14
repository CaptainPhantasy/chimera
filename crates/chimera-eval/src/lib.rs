use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// ID types
// ---------------------------------------------------------------------------

pub type CheckpointId = Uuid;

// ---------------------------------------------------------------------------
// Eval request
// ---------------------------------------------------------------------------

/// A request to evaluate an artifact or run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalRequest {
    /// Human-readable name for this evaluation.
    pub name: String,
    /// What to evaluate (session ID, artifact path, etc.).
    pub target: EvalTarget,
    /// Which graders to run.
    pub graders: Vec<GraderSpec>,
}

/// What the evaluation targets.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EvalTarget {
    /// Evaluate a session's output.
    Session { session_id: Uuid },
    /// Evaluate a specific file or artifact.
    Artifact { path: String },
    /// Evaluate a diff between two states.
    Diff { before: String, after: String },
}

/// Specification for a grader to run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraderSpec {
    /// Grader name (e.g. "test_pass", "lint_clean", "regression_check").
    pub name: String,
    /// Grader-specific configuration.
    pub config: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Eval report
// ---------------------------------------------------------------------------

/// The result of an evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalReport {
    /// Evaluation name.
    pub name: String,
    /// Overall pass/fail.
    pub passed: bool,
    /// Overall score (0.0 to 1.0).
    pub score: f64,
    /// Per-grader results.
    pub grades: Vec<GradeResult>,
    /// When the evaluation completed.
    pub completed_at: DateTime<Utc>,
}

/// Result from a single grader.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GradeResult {
    /// Grader name.
    pub grader: String,
    /// Pass/fail for this grader.
    pub passed: bool,
    /// Score from this grader (0.0 to 1.0).
    pub score: f64,
    /// Human-readable details.
    pub details: String,
    /// Structured evidence.
    pub evidence: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Diff report
// ---------------------------------------------------------------------------

/// A comparison between two checkpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffReport {
    /// First checkpoint.
    pub checkpoint_a: CheckpointId,
    /// Second checkpoint.
    pub checkpoint_b: CheckpointId,
    /// Files changed.
    pub files_changed: Vec<FileChange>,
    /// Summary of differences.
    pub summary: String,
    /// Regression detected.
    pub regression_detected: bool,
}

/// A file change in a diff report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub change_type: ChangeType,
    pub lines_added: u32,
    pub lines_removed: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeType {
    Added,
    Modified,
    Deleted,
    Renamed,
}

// ---------------------------------------------------------------------------
// Evaluator trait
// ---------------------------------------------------------------------------

/// Runs evaluations, grades artifacts, and compares checkpoints.
#[async_trait::async_trait]
pub trait Evaluator: Send + Sync {
    /// Run an evaluation and return a report.
    async fn grade(&self, req: EvalRequest) -> anyhow::Result<EvalReport>;

    /// Compare two checkpoints and return a diff report.
    async fn compare(&self, a: CheckpointId, b: CheckpointId) -> anyhow::Result<DiffReport>;
}

/// A real check result supplied by the current permission-enforcing runner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// The caller supplies the same authorized command route used by ordinary tools.
/// Evaluators never start an unrestricted shell behind the approval system.
#[async_trait::async_trait]
pub trait CheckRunner: Send + Sync {
    async fn run(
        &self,
        command: &str,
        cwd: &std::path::Path,
        timeout_secs: u64,
    ) -> anyhow::Result<CheckOutput>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvaluation {
    pub id: Uuid,
    pub request: EvalRequest,
    pub report: EvalReport,
}
#[derive(Default, Serialize, Deserialize)]
struct EvaluationRecords {
    reports: Vec<StoredEvaluation>,
    definitions: std::collections::BTreeMap<String, EvalRequest>,
}

pub struct FileEvaluator {
    records: storage::ProjectRecords,
    runner: std::sync::Arc<dyn CheckRunner>,
}
impl FileEvaluator {
    pub fn new(
        root: impl Into<std::path::PathBuf>,
        runner: std::sync::Arc<dyn CheckRunner>,
    ) -> Self {
        Self {
            records: storage::ProjectRecords::new(root, "evaluations"),
            runner,
        }
    }
    pub fn save_definition(&self, request: EvalRequest) -> anyhow::Result<()> {
        validate_request(&request)?;
        self.validate_target(&request.target)?;
        self.records
            .update("records", |records: &mut EvaluationRecords| {
                records.definitions.insert(request.name.clone(), request);
                Ok(())
            })
    }
    pub fn definitions(&self) -> anyhow::Result<Vec<String>> {
        let records: EvaluationRecords = self.records.read("records")?;
        Ok(records.definitions.into_keys().collect())
    }
    pub async fn run_named(&self, name: &str) -> anyhow::Result<EvalReport> {
        let records: EvaluationRecords = self.records.read("records")?;
        let request = records.definitions.get(name).cloned().ok_or_else(|| {
            anyhow::anyhow!(
                "evaluation definition not found: {name}; save explicit permitted checks first"
            )
        })?;
        self.grade(request).await
    }
    pub fn reports(&self) -> anyhow::Result<Vec<StoredEvaluation>> {
        let records: EvaluationRecords = self.records.read("records")?;
        Ok(records.reports)
    }

    fn validate_target(&self, target: &EvalTarget) -> anyhow::Result<()> {
        match target {
            EvalTarget::Artifact { path } => {
                let file = scoped_file(self.records.root(), std::path::Path::new(path))?;
                if !file.is_file() {
                    anyhow::bail!("evaluation artifact must exist and be a regular file");
                }
            }
            EvalTarget::Session { session_id } => {
                let record = read_internal_json(
                    self.records.root(),
                    &["sessions"],
                    &format!("{session_id}.json"),
                )?;
                let data = if let Some(version) = record.get("format_version") {
                    if version.as_u64() != Some(1) {
                        anyhow::bail!("unsupported session format");
                    }
                    &record["data"]
                } else {
                    &record
                };
                if data["id"].as_str() != Some(session_id.to_string().as_str()) {
                    anyhow::bail!("evaluation session identity mismatch");
                }
            }
            EvalTarget::Diff { before, after } => {
                self.checkpoint(Uuid::parse_str(before)?)?;
                self.checkpoint(Uuid::parse_str(after)?)?;
            }
        }
        Ok(())
    }
    fn checkpoint(&self, id: Uuid) -> anyhow::Result<CheckpointContents> {
        let envelope = read_internal_json(
            self.records.root(),
            &["sessions", "checkpoints"],
            &format!("{id}.json"),
        )?;
        if envelope["format_version"].as_u64() != Some(1) {
            anyhow::bail!("unsupported checkpoint format");
        }
        let cp: CheckpointContents = serde_json::from_value(envelope["data"].clone())?;
        if cp.id != id
            || cp.state.get("id").and_then(|v| v.as_str())
                != Some(cp.session_id.to_string().as_str())
        {
            anyhow::bail!("checkpoint identity mismatch");
        }
        for file in &cp.files {
            validate_relative(&file.path)?;
        }
        Ok(cp)
    }
}

fn validate_request(req: &EvalRequest) -> anyhow::Result<()> {
    if req.name.trim().is_empty() || req.name.len() > 128 {
        anyhow::bail!("evaluation name is required and limited to 128 bytes");
    }
    if req.graders.is_empty() || req.graders.len() > 32 {
        anyhow::bail!("evaluation requires 1 to 32 explicit checks");
    }
    let mut names = std::collections::HashSet::new();
    for grader in &req.graders {
        if grader.name.trim().is_empty() || !names.insert(&grader.name) {
            anyhow::bail!("each check requires a unique nonempty name");
        }
        let command = grader
            .config
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("check {} has no command", grader.name))?;
        if command.trim().is_empty() || command.len() > 16 * 1024 {
            anyhow::bail!("check command must be nonempty and <=16 KiB");
        }
        if let Some(timeout) = grader.config.get("timeout_secs") {
            if !timeout.as_u64().is_some_and(|n| (1..=600).contains(&n)) {
                anyhow::bail!("check timeout must be 1 to 600 seconds");
            }
        }
    }
    Ok(())
}

#[async_trait::async_trait]
impl Evaluator for FileEvaluator {
    async fn grade(&self, req: EvalRequest) -> anyhow::Result<EvalReport> {
        validate_request(&req)?;
        self.validate_target(&req.target)?;
        let root = self.records.root().canonicalize()?;
        let id = Uuid::new_v4();
        let mut grades = Vec::new();
        for grader in &req.graders {
            let command = grader.config["command"]
                .as_str()
                .expect("validated command");
            let timeout = grader
                .config
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(120);
            let started = Utc::now();
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(timeout),
                self.runner.run(command, &root, timeout),
            )
            .await;
            let (passed, details, evidence) = match result {
                Ok(Ok(output)) => {
                    let too_large =
                        output.stdout.len() > 512 * 1024 || output.stderr.len() > 512 * 1024;
                    let passed = output.exit_code == 0 && !output.timed_out && !too_large;
                    let details = if too_large {
                        "check output exceeded 512 KiB per-stream evidence limit".into()
                    } else if output.timed_out {
                        "check timed out".into()
                    } else {
                        format!("check exited with status {}", output.exit_code)
                    };
                    (
                        passed,
                        details,
                        serde_json::json!({"evaluation_id":id,"command":command,"cwd":root,"started_at":started,"finished_at":Utc::now(),"exit_code":output.exit_code,"timed_out":output.timed_out,"stdout":bounded_text(&output.stdout),"stderr":bounded_text(&output.stderr),"stdout_bytes":output.stdout.len(),"stderr_bytes":output.stderr.len(),"output_truncated":too_large}),
                    )
                }
                Ok(Err(error)) => (
                    false,
                    format!("check failed before returning a result: {error}"),
                    serde_json::json!({"evaluation_id":id,"command":command,"started_at":started,"finished_at":Utc::now(),"error":error.to_string()}),
                ),
                Err(_) => (
                    false,
                    "check timed out before a result was returned".into(),
                    serde_json::json!({"evaluation_id":id,"command":command,"started_at":started,"finished_at":Utc::now(),"timed_out":true}),
                ),
            };
            grades.push(GradeResult {
                grader: grader.name.clone(),
                passed,
                score: if passed { 1.0 } else { 0.0 },
                details,
                evidence,
            });
        }
        let report = EvalReport {
            name: req.name.clone(),
            passed: grades.iter().all(|g| g.passed),
            score: grades.iter().filter(|g| g.passed).count() as f64 / grades.len() as f64,
            grades,
            completed_at: Utc::now(),
        };
        self.records
            .update("records", |records: &mut EvaluationRecords| {
                records.reports.push(StoredEvaluation {
                    id,
                    request: req,
                    report: report.clone(),
                });
                Ok(())
            })?;
        Ok(report)
    }
    async fn compare(&self, a: CheckpointId, b: CheckpointId) -> anyhow::Result<DiffReport> {
        let before = self.checkpoint(a)?;
        let after = self.checkpoint(b)?;
        let left: std::collections::BTreeMap<_, _> =
            before.files.iter().map(|f| (f.path.clone(), f)).collect();
        let right: std::collections::BTreeMap<_, _> =
            after.files.iter().map(|f| (f.path.clone(), f)).collect();
        if left.len() != before.files.len() || right.len() != after.files.len() {
            anyhow::bail!("duplicate file paths in checkpoint");
        }
        // A missing capture is not proof of file absence. Both checkpoints must
        // cover the same explicit scope; bytes=None records actual absence.
        if left.keys().ne(right.keys()) {
            anyhow::bail!("checkpoint file scopes differ; capture the same paths before comparing");
        }
        let mut changes = Vec::new();
        let mut binary = 0;
        for (path, old) in left {
            let new = right[&path];
            if old.bytes == new.bytes && old.unix_mode == new.unix_mode {
                continue;
            }
            let change_type = match (&old.bytes, &new.bytes) {
                (None, Some(_)) => ChangeType::Added,
                (Some(_), None) => ChangeType::Deleted,
                _ => ChangeType::Modified,
            };
            let (lines_added, lines_removed) = match (
                std::str::from_utf8(old.bytes.as_deref().unwrap_or(&[])),
                std::str::from_utf8(new.bytes.as_deref().unwrap_or(&[])),
            ) {
                (Ok(old_text), Ok(new_text)) => line_changes(old_text, new_text)?,
                _ => {
                    binary += 1;
                    (0, 0)
                }
            };
            changes.push(FileChange {
                path: path.to_string_lossy().into_owned(),
                change_type,
                lines_added,
                lines_removed,
            });
        }
        Ok(DiffReport{checkpoint_a:a,checkpoint_b:b,summary:format!("{} captured files changed; saved session state {}. {binary} binary file(s) compared by bytes with no text-line counts. Content comparison only; behavioral regression checks were not run.",changes.len(),if before.state == after.state { "unchanged" } else { "changed" }),files_changed:changes,regression_detected:false})
    }
}
fn bounded_text(text: &str) -> &str {
    let mut end = text.len().min(512 * 1024);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[derive(Deserialize)]
struct CheckpointContents {
    id: Uuid,
    session_id: Uuid,
    state: serde_json::Value,
    files: Vec<CheckpointFile>,
}
#[derive(Deserialize)]
struct CheckpointFile {
    path: std::path::PathBuf,
    bytes: Option<Vec<u8>>,
    unix_mode: Option<u32>,
}

fn line_changes(old: &str, new: &str) -> anyhow::Result<(u32, u32)> {
    let old: Vec<_> = old.lines().collect();
    let new: Vec<_> = new.lines().collect();
    if old.len().saturating_mul(new.len()) > 2_000_000 {
        anyhow::bail!(
            "text comparison exceeds bounded 2-million-cell limit; use a narrower checkpoint"
        );
    }
    let mut row = vec![0usize; new.len() + 1];
    for line in &old {
        let mut previous = 0;
        for (j, other) in new.iter().enumerate() {
            let saved = row[j + 1];
            row[j + 1] = if line == other {
                previous + 1
            } else {
                row[j + 1].max(row[j])
            };
            previous = saved;
        }
    }
    let common = row[new.len()];
    Ok(((new.len() - common) as u32, (old.len() - common) as u32))
}
fn validate_relative(path: &std::path::Path) -> anyhow::Result<()> {
    if path.as_os_str().is_empty() {
        anyhow::bail!("empty artifact path");
    }
    for part in path.components() {
        let std::path::Component::Normal(name) = part else {
            anyhow::bail!("artifact paths must be relative without traversal");
        };
        let name = name.to_string_lossy();
        if [
            ".git",
            ".chimera",
            ".floyd",
            ".supercache",
            ".agents",
            ".codex",
            ".aws",
        ]
        .contains(&name.as_ref())
            || name == ".env"
            || name.starts_with(".env.")
        {
            anyhow::bail!("protected artifact path");
        }
    }
    Ok(())
}
fn scoped_file(
    root: &std::path::Path,
    path: &std::path::Path,
) -> anyhow::Result<std::path::PathBuf> {
    validate_relative(path)?;
    let mut current = root.canonicalize()?;
    for part in path.components() {
        current.push(part);
        if std::fs::symlink_metadata(&current)?
            .file_type()
            .is_symlink()
        {
            anyhow::bail!("symlink artifact path rejected");
        }
    }
    Ok(current)
}
fn read_internal_json(
    root: &std::path::Path,
    parts: &[&str],
    name: &str,
) -> anyhow::Result<serde_json::Value> {
    use std::io::Read;
    let mut current = root.canonicalize()?.join(".chimera");
    for part in std::iter::once("").chain(parts.iter().copied()) {
        if !part.is_empty() {
            current.push(part);
        }
        let meta = std::fs::symlink_metadata(&current)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            anyhow::bail!("unsafe checkpoint directory");
        }
    }
    current.push(name);
    let meta = std::fs::symlink_metadata(&current)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        anyhow::bail!("unsafe checkpoint file");
    }
    let mut bytes = Vec::new();
    std::fs::File::open(current)?
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 32 * 1024 * 1024 {
        anyhow::bail!("checkpoint exceeds 32 MiB limit");
    }
    Ok(serde_json::from_slice(&bytes)?)
}

mod storage {
    use anyhow::{bail, Context};
    use serde::{de::DeserializeOwned, Serialize};
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};
    use uuid::Uuid;
    const LIMIT: u64 = 32 * 1024 * 1024;

    pub struct ProjectRecords {
        root: PathBuf,
        namespace: String,
    }
    impl ProjectRecords {
        pub fn new(root: impl Into<PathBuf>, namespace: &str) -> Self {
            Self {
                root: root.into(),
                namespace: namespace.into(),
            }
        }
        pub fn root(&self) -> &Path {
            &self.root
        }
        fn directory(&self) -> anyhow::Result<PathBuf> {
            valid_name(&self.namespace)?;
            let root = self
                .root
                .canonicalize()
                .context("project folder is unavailable")?;
            let state = private_dir(&root.join(".chimera"))?;
            private_dir(&state.join(&self.namespace))
        }
        pub fn path(&self, name: &str) -> anyhow::Result<PathBuf> {
            valid_name(name)?;
            Ok(self.directory()?.join(format!("{name}.json")))
        }
        fn lock(&self) -> anyhow::Result<File> {
            let path = self.directory()?.join(".lock");
            regular(&path)?;
            let mut opts = OpenOptions::new();
            opts.read(true).write(true).create(true).truncate(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let file = opts.open(path)?;
            let start = Instant::now();
            loop {
                match file.try_lock() {
                    Ok(()) => return Ok(file),
                    Err(std::fs::TryLockError::WouldBlock)
                        if start.elapsed() < Duration::from_secs(5) =>
                    {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(e) => bail!("project record lock unavailable: {e}"),
                }
            }
        }
        fn load<T: Default + DeserializeOwned>(&self, name: &str) -> anyhow::Result<T> {
            let path = self.path(name)?;
            regular(&path)?;
            if !path.exists() {
                return Ok(T::default());
            }
            let value: serde_json::Value = serde_json::from_slice(&read_bytes(&path)?)
                .with_context(|| {
                    format!(
                        "corrupt project record at {}; previous version is {}.previous",
                        path.display(),
                        path.display()
                    )
                })?;
            if value["format_version"].as_u64() != Some(1) {
                bail!("unsupported project record version");
            }
            if value["project_root"].as_str() != self.root.canonicalize()?.to_str() {
                bail!("project identity mismatch in {}", path.display());
            }
            serde_json::from_value(value["data"].clone()).context("invalid project record contents")
        }
        pub fn read<T: Default + DeserializeOwned>(&self, name: &str) -> anyhow::Result<T> {
            let _lock = self.lock()?;
            self.load(name)
        }
        pub fn update<T: Default + DeserializeOwned + Serialize, R>(
            &self,
            name: &str,
            update: impl FnOnce(&mut T) -> anyhow::Result<R>,
        ) -> anyhow::Result<R> {
            let _lock = self.lock()?;
            let mut data = self.load(name)?;
            let result = update(&mut data)?;
            let bytes = serde_json::to_vec(
                &serde_json::json!({"format_version":1,"project_root":self.root.canonicalize()?,"data":data}),
            )?;
            if bytes.len() as u64 > LIMIT {
                bail!("project record exceeds 32 MiB limit");
            }
            let path = self.path(name)?;
            if path.exists() {
                atomic_write(
                    &PathBuf::from(format!("{}.previous", path.display())),
                    &read_bytes(&path)?,
                )?;
            }
            atomic_write(&path, &bytes)?;
            Ok(result)
        }
    }
    fn valid_name(name: &str) -> anyhow::Result<()> {
        if name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            bail!("invalid record name");
        }
        Ok(())
    }
    fn private_dir(path: &Path) -> anyhow::Result<PathBuf> {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => (),
            Ok(_) => bail!("unsafe project state directory: {}", path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut builder = std::fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                match builder.create(path) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                    Err(e) => return Err(e.into()),
                }
                let meta = std::fs::symlink_metadata(path)?;
                if !meta.is_dir() || meta.file_type().is_symlink() {
                    bail!("unsafe project state directory");
                }
            }
            Err(e) => return Err(e.into()),
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(path.into())
    }
    fn regular(path: &Path) -> anyhow::Result<()> {
        match std::fs::symlink_metadata(path) {
            Ok(m) if !m.is_file() || m.file_type().is_symlink() => {
                bail!("unsafe project state file: {}", path.display())
            }
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
    fn read_bytes(path: &Path) -> anyhow::Result<Vec<u8>> {
        regular(path)?;
        let mut bytes = Vec::new();
        File::open(path)?.take(LIMIT + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > LIMIT {
            bail!("project record exceeds 32 MiB limit");
        }
        Ok(bytes)
    }
    fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
        regular(path)?;
        let parent = path.parent().context("record parent missing")?;
        let temp = parent.join(format!(".pending-{}", Uuid::new_v4()));
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(temp, path)?;
        #[cfg(unix)]
        File::open(parent)?.sync_all()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Mock evaluator for testing
// ---------------------------------------------------------------------------

/// A mock evaluator that runs graders as simple pass/fail checks.
#[cfg(test)]
pub struct MockEvaluator;

#[cfg(test)]
#[async_trait::async_trait]
impl Evaluator for MockEvaluator {
    async fn grade(&self, req: EvalRequest) -> anyhow::Result<EvalReport> {
        if req.graders.is_empty() {
            anyhow::bail!("evaluation requires at least one check");
        }
        let grades: Vec<GradeResult> = req
            .graders
            .iter()
            .map(|g| {
                let passed = g
                    .config
                    .get("pass")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let score = if passed { 1.0 } else { 0.0 };
                GradeResult {
                    grader: g.name.clone(),
                    passed,
                    score,
                    details: if passed {
                        "all checks passed".into()
                    } else {
                        "check failed".into()
                    },
                    evidence: serde_json::json!({}),
                }
            })
            .collect();

        let all_passed = grades.iter().all(|g| g.passed);
        let avg_score = if grades.is_empty() {
            1.0
        } else {
            grades.iter().map(|g| g.score).sum::<f64>() / grades.len() as f64
        };

        Ok(EvalReport {
            name: req.name,
            passed: all_passed,
            score: avg_score,
            grades,
            completed_at: Utc::now(),
        })
    }

    async fn compare(&self, a: CheckpointId, b: CheckpointId) -> anyhow::Result<DiffReport> {
        Ok(DiffReport {
            checkpoint_a: a,
            checkpoint_b: b,
            files_changed: vec![FileChange {
                path: "src/mock.rs".into(),
                change_type: ChangeType::Modified,
                lines_added: 10,
                lines_removed: 3,
            }],
            summary: "mock diff comparison".into(),
            regression_detected: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn grade_all_pass() {
        let eval = MockEvaluator;
        let req = EvalRequest {
            name: "test-eval".into(),
            target: EvalTarget::Artifact {
                path: "src/lib.rs".into(),
            },
            graders: vec![
                GraderSpec {
                    name: "lint".into(),
                    config: serde_json::json!({"pass": true}),
                },
                GraderSpec {
                    name: "tests".into(),
                    config: serde_json::json!({"pass": true}),
                },
            ],
        };

        let report = eval.grade(req).await.unwrap();
        assert!(report.passed);
        assert_eq!(report.score, 1.0);
        assert_eq!(report.grades.len(), 2);
        assert!(report.grades.iter().all(|g| g.passed));
    }

    #[tokio::test]
    async fn grade_with_failure() {
        let eval = MockEvaluator;
        let req = EvalRequest {
            name: "failing-eval".into(),
            target: EvalTarget::Artifact {
                path: "src/bad.rs".into(),
            },
            graders: vec![
                GraderSpec {
                    name: "lint".into(),
                    config: serde_json::json!({"pass": true}),
                },
                GraderSpec {
                    name: "security".into(),
                    config: serde_json::json!({"pass": false}),
                },
            ],
        };

        let report = eval.grade(req).await.unwrap();
        assert!(!report.passed);
        assert_eq!(report.score, 0.5);
        assert!(!report.grades[1].passed);
    }

    #[tokio::test]
    async fn grade_empty_graders() {
        let eval = MockEvaluator;
        let req = EvalRequest {
            name: "empty".into(),
            target: EvalTarget::Session {
                session_id: Uuid::new_v4(),
            },
            graders: vec![],
        };

        assert!(eval.grade(req).await.is_err());
    }

    #[tokio::test]
    async fn compare_checkpoints() {
        let eval = MockEvaluator;
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();

        let diff = eval.compare(a, b).await.unwrap();
        assert_eq!(diff.checkpoint_a, a);
        assert_eq!(diff.checkpoint_b, b);
        assert_eq!(diff.files_changed.len(), 1);
        assert_eq!(diff.files_changed[0].path, "src/mock.rs");
        assert!(!diff.regression_detected);
    }
    struct PermittedTestRunner;
    #[async_trait::async_trait]
    impl CheckRunner for PermittedTestRunner {
        async fn run(
            &self,
            command: &str,
            cwd: &std::path::Path,
            _timeout_secs: u64,
        ) -> anyhow::Result<CheckOutput> {
            let script = match command {
                "permitted-pass" => "printf 'actual checked output'",
                "permitted-fail" => "printf 'actual failure output' >&2; exit 7",
                _ => anyhow::bail!("command permission denied"),
            };
            let output = tokio::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(script)
                .current_dir(cwd)
                .output()
                .await?;
            Ok(CheckOutput {
                exit_code: output.status.code().unwrap_or(-1),
                stdout: String::from_utf8(output.stdout)?,
                stderr: String::from_utf8(output.stderr)?,
                timed_out: false,
            })
        }
    }
    fn root() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("chimera-eval-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("sample.txt"), b"sample").unwrap();
        root
    }
    fn request(command: &str) -> EvalRequest {
        EvalRequest {
            name: "sample-evaluation".into(),
            target: EvalTarget::Artifact {
                path: "sample.txt".into(),
            },
            graders: vec![GraderSpec {
                name: "actual-check".into(),
                config: serde_json::json!({"command":command,"timeout_secs":5}),
            }],
        }
    }
    #[tokio::test]
    async fn real_checks_keep_failed_output_and_definitions_survive_restart() {
        let root = root();
        let eval = FileEvaluator::new(&root, std::sync::Arc::new(PermittedTestRunner));
        eval.save_definition(request("permitted-fail")).unwrap();
        let reopened = FileEvaluator::new(&root, std::sync::Arc::new(PermittedTestRunner));
        let report = reopened.run_named("sample-evaluation").await.unwrap();
        assert!(!report.passed);
        assert_eq!(report.grades[0].evidence["exit_code"], 7);
        assert_eq!(report.grades[0].evidence["stderr"], "actual failure output");
        assert_eq!(reopened.reports().unwrap().len(), 1);
        let pass = reopened.grade(request("permitted-pass")).await.unwrap();
        assert!(pass.passed);
        assert_eq!(pass.grades[0].evidence["stdout"], "actual checked output");
        let denied = reopened.grade(request("not-authorized")).await.unwrap();
        assert!(!denied.passed);
        assert!(denied.grades[0].details.contains("permission denied"));
    }
    #[tokio::test]
    async fn missing_checks_commands_targets_and_foreign_records_fail() {
        let first = root();
        let eval = FileEvaluator::new(&first, std::sync::Arc::new(PermittedTestRunner));
        let mut req = request("permitted-pass");
        req.graders.clear();
        assert!(eval.grade(req).await.is_err());
        let mut req = request("permitted-pass");
        req.graders[0].config = serde_json::json!({"pass":true});
        assert!(eval.grade(req).await.is_err());
        assert!(eval.reports().unwrap().is_empty());
        let mut req = request("permitted-pass");
        req.target = EvalTarget::Artifact {
            path: "../sample.txt".into(),
        };
        assert!(eval.grade(req).await.is_err());
        eval.save_definition(request("permitted-pass")).unwrap();
        let second = root();
        let other = FileEvaluator::new(&second, std::sync::Arc::new(PermittedTestRunner));
        std::fs::copy(
            eval.records.path("records").unwrap(),
            other.records.path("records").unwrap(),
        )
        .unwrap();
        assert!(other.definitions().is_err());
    }
    fn save_checkpoint(
        root: &std::path::Path,
        session: Uuid,
        id: Uuid,
        contents: &[u8],
        path: &str,
    ) {
        let directory = root.join(".chimera/sessions/checkpoints");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(format!("{id}.json")),serde_json::to_vec(&serde_json::json!({"format_version":1,"data":{"id":id,"session_id":session,"state":{"id":session},"files":[{"path":path,"bytes":contents,"unix_mode":420}]}})).unwrap()).unwrap();
    }
    #[tokio::test]
    async fn checkpoint_comparison_reads_real_contents_and_rejects_unknown_scope() {
        let root = root();
        let eval = FileEvaluator::new(&root, std::sync::Arc::new(PermittedTestRunner));
        let session = Uuid::new_v4();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        save_checkpoint(&root, session, a, b"one\ntwo\nthree\n", "sample.txt");
        save_checkpoint(
            &root,
            session,
            b,
            b"one\nchanged\nthree\nnew\n",
            "sample.txt",
        );
        let diff = eval.compare(a, b).await.unwrap();
        assert_eq!(diff.files_changed.len(), 1);
        assert_eq!(diff.files_changed[0].lines_added, 2);
        assert_eq!(diff.files_changed[0].lines_removed, 1);
        assert!(diff
            .summary
            .contains("behavioral regression checks were not run"));
        save_checkpoint(&root, session, b, b"different", "other.txt");
        assert!(eval.compare(a, b).await.is_err());
        assert!(eval.compare(a, Uuid::new_v4()).await.is_err());
    }
    struct LargeOutputRunner;
    #[async_trait::async_trait]
    impl CheckRunner for LargeOutputRunner {
        async fn run(
            &self,
            _command: &str,
            _cwd: &std::path::Path,
            _timeout_secs: u64,
        ) -> anyhow::Result<CheckOutput> {
            Ok(CheckOutput {
                exit_code: 0,
                stdout: "é".repeat(300_000),
                stderr: String::new(),
                timed_out: false,
            })
        }
    }
    #[tokio::test]
    async fn oversized_evidence_is_explicit_and_cannot_pass() {
        let root = root();
        let eval = FileEvaluator::new(root, std::sync::Arc::new(LargeOutputRunner));
        let report = eval.grade(request("permitted-pass")).await.unwrap();
        assert!(!report.passed);
        assert_eq!(report.grades[0].evidence["output_truncated"], true);
        assert_eq!(report.grades[0].evidence["stdout_bytes"], 600_000);
        assert!(report.grades[0].evidence["stdout"].as_str().unwrap().len() <= 512 * 1024);
    }
    #[cfg(unix)]
    #[test]
    fn record_write_limit_worker() {
        let Ok(root) = std::env::var("CHIMERA_RECORD_IO_ROOT") else {
            return;
        };
        #[repr(C)]
        struct Limit {
            current: u64,
            maximum: u64,
        }
        unsafe extern "C" {
            fn setrlimit(resource: i32, limit: *const Limit) -> i32;
            fn signal(number: i32, handler: usize) -> usize;
        }
        unsafe {
            signal(25, 1);
            assert_eq!(
                setrlimit(
                    1,
                    &Limit {
                        current: 4096,
                        maximum: 4096
                    }
                ),
                0
            );
        }
        let records = storage::ProjectRecords::new(root, "io-test");
        let result = records.update("record", |rows: &mut Vec<String>| {
            rows.push("new value".repeat(10_000));
            Ok(())
        });
        assert!(
            result.is_err(),
            "partial file write must not report a saved update"
        );
    }
    #[cfg(unix)]
    #[test]
    fn real_partial_write_error_preserves_project_records_and_previous_version() {
        let root = root();
        let records = storage::ProjectRecords::new(&root, "io-test");
        records
            .update("record", |rows: &mut Vec<String>| {
                rows.push("committed value".into());
                Ok(())
            })
            .unwrap();
        let path = records.path("record").unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(before.len() < 4096);
        assert!(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::record_write_limit_worker"])
            .env("CHIMERA_RECORD_IO_ROOT", &root)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap()
            .success());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            std::fs::read(std::path::PathBuf::from(format!(
                "{}.previous",
                path.display()
            )))
            .unwrap(),
            before
        );
        assert_eq!(
            records.read::<Vec<String>>("record").unwrap(),
            vec!["committed value"]
        );
        assert!(std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with(".pending-")
                && e.metadata().unwrap().len() == 4096));
        records
            .update("record", |rows: &mut Vec<String>| {
                rows.push("after failed write".into());
                Ok(())
            })
            .unwrap();
        assert_eq!(
            records.read::<Vec<String>>("record").unwrap(),
            vec!["committed value", "after failed write"]
        );
    }
}
