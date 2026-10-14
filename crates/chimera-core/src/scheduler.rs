//! Durable project schedules. A run is claimed on disk before any child is started.
//! Claims surviving a service restart are ambiguous and require explicit resumption.
use anyhow::{ensure, Context};
use chimera_memory::storage::ProjectRecords;
use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub type WorkflowId = Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduleFrequency {
    Once {
        at: DateTime<Utc>,
    },
    Interval {
        every_secs: u64,
    },
    /// Five numeric fields in UTC: minute hour day-of-month month day-of-week.
    Cron {
        expression: String,
    },
    /// Project-relative glob, supporting *, ** and ?. Protected state is excluded.
    Watch {
        pattern: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleSpec {
    pub label: String,
    pub objective: String,
    pub frequency: ScheduleFrequency,
    pub pack: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowHandle {
    pub id: WorkflowId,
    pub spec: ScheduleSpec,
    pub status: WorkflowStatus,
    pub created_at: DateTime<Utc>,
    pub last_run: Option<DateTime<Utc>>,
    /// Number of durable claims, including claims interrupted before child launch.
    pub run_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    Active,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[async_trait::async_trait]
pub trait WorkflowScheduler: Send + Sync {
    async fn create(&self, spec: ScheduleSpec) -> anyhow::Result<WorkflowId>;
    async fn get(&self, id: WorkflowId) -> anyhow::Result<WorkflowHandle>;
    async fn list(&self) -> anyhow::Result<Vec<WorkflowId>>;
    async fn pause(&self, id: WorkflowId) -> anyhow::Result<()>;
    async fn resume(&self, id: WorkflowId) -> anyhow::Result<()>;
    async fn cancel(&self, id: WorkflowId) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobRunStatus {
    Claimed,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobOutcome {
    pub status: JobRunStatus,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub output_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRun {
    pub id: Uuid,
    pub claimed_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub outcome: JobOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchScanFailure {
    pub at: DateTime<Utc>,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDetails {
    pub handle: WorkflowHandle,
    pub next_run: Option<DateTime<Utc>>,
    pub in_flight: Option<Uuid>,
    pub runs: Vec<JobRun>,
    /// Scan failures stop this watcher without inventing a child run or claim.
    #[serde(default)]
    pub scan_failures: Vec<WatchScanFailure>,
    watch_snapshot: BTreeMap<String, FileStamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobClaim {
    pub workflow_id: WorkflowId,
    pub run_id: Uuid,
    pub spec: ScheduleSpec,
    pub claimed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileStamp {
    bytes: u64,
    modified_nanos: Option<u128>,
    fingerprint: u64,
}
#[derive(Default, Serialize, Deserialize)]
struct Records {
    workflows: BTreeMap<WorkflowId, WorkflowDetails>,
}

pub struct FileWorkflowScheduler {
    records: ProjectRecords,
}
impl FileWorkflowScheduler {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            records: ProjectRecords::new(root, "jobs"),
        }
    }
    pub fn root(&self) -> &Path {
        self.records.root()
    }
    pub fn details(&self, id: WorkflowId) -> anyhow::Result<WorkflowDetails> {
        self.records
            .read::<Records>("schedules")?
            .workflows
            .remove(&id)
            .ok_or_else(|| anyhow::anyhow!("workflow not found: {id}"))
    }
    pub fn all(&self) -> anyhow::Result<Vec<WorkflowDetails>> {
        Ok(self
            .records
            .read::<Records>("schedules")?
            .workflows
            .into_values()
            .collect())
    }
    /// Atomically claim at most one due run. No catch-up burst for missed intervals.
    pub fn claim_next(&self, now: DateTime<Utc>) -> anyhow::Result<Option<JobClaim>> {
        let records = self.records.read::<Records>("schedules")?;
        for (id, workflow) in records.workflows {
            if workflow.handle.status != WorkflowStatus::Active || workflow.in_flight.is_some() {
                continue;
            }
            let snapshot = match &workflow.handle.spec.frequency {
                ScheduleFrequency::Watch { pattern } => {
                    match watch_snapshot(self.root(), pattern) {
                        Ok(snapshot) => Some(snapshot),
                        Err(error) => {
                            self.records.update::<Records, _>("schedules", |records| {
                                let current = records
                                    .workflows
                                    .get_mut(&id)
                                    .context("workflow disappeared")?;
                                // Do not overwrite a concurrent pause, cancellation, claim or resumption.
                                if current.handle.status != WorkflowStatus::Active
                                    || current.in_flight.is_some()
                                    || current.watch_snapshot != workflow.watch_snapshot
                                {
                                    return Ok(());
                                }
                                current.scan_failures.push(WatchScanFailure {
                                    at: now,
                                    error: format!("{error:#}").chars().take(4096).collect(),
                                });
                                current.handle.status = WorkflowStatus::Failed;
                                current.handle.spec.enabled = false;
                                Ok(())
                            })?;
                            continue;
                        }
                    }
                }
                _ => None,
            };
            let due = snapshot
                .as_ref()
                .map(|s| s != &workflow.watch_snapshot)
                .unwrap_or_else(|| workflow.next_run.is_some_and(|at| at <= now));
            if !due {
                continue;
            }
            let claim = self.records.update::<Records, _>("schedules", |records| {
                let current = records
                    .workflows
                    .get_mut(&id)
                    .context("workflow disappeared")?;
                if current.handle.status != WorkflowStatus::Active || current.in_flight.is_some() {
                    return Ok(None);
                }
                if let Some(snapshot) = snapshot {
                    if current.watch_snapshot != workflow.watch_snapshot
                        || snapshot == current.watch_snapshot
                    {
                        return Ok(None);
                    }
                    current.watch_snapshot = snapshot;
                } else if current.next_run.is_none_or(|at| at > now) {
                    return Ok(None);
                }
                let run_id = Uuid::new_v4();
                current.in_flight = Some(run_id);
                current.handle.run_count += 1;
                current.handle.last_run = Some(now);
                current.next_run = next_run(&current.handle.spec.frequency, now, false)?;
                current.runs.push(JobRun {
                    id: run_id,
                    claimed_at: now,
                    finished_at: None,
                    outcome: JobOutcome {
                        status: JobRunStatus::Claimed,
                        exit_code: None,
                        stdout: String::new(),
                        stderr: String::new(),
                        output_truncated: false,
                    },
                });
                Ok(Some(JobClaim {
                    workflow_id: id,
                    run_id,
                    spec: current.handle.spec.clone(),
                    claimed_at: now,
                }))
            })?;
            if claim.is_some() {
                return Ok(claim);
            }
        }
        Ok(None)
    }
    pub fn finish(&self, claim: &JobClaim, outcome: JobOutcome) -> anyhow::Result<()> {
        ensure!(
            outcome.status != JobRunStatus::Completed || outcome.exit_code == Some(0),
            "completed child must have exit code zero"
        );
        ensure!(
            outcome.status != JobRunStatus::Claimed,
            "a finished run cannot remain claimed"
        );
        ensure!(
            outcome.stdout.len() <= 66_000 && outcome.stderr.len() <= 66_000,
            "job output exceeds storage limit"
        );
        self.records.update::<Records, _>("schedules", |records| {
            let job = records
                .workflows
                .get_mut(&claim.workflow_id)
                .context("workflow not found")?;
            ensure!(
                job.in_flight == Some(claim.run_id),
                "run claim is no longer current"
            );
            let run = job
                .runs
                .iter_mut()
                .find(|r| r.id == claim.run_id)
                .context("missing run history")?;
            run.finished_at = Some(Utc::now());
            run.outcome = outcome.clone();
            job.in_flight = None;
            if job.handle.status == WorkflowStatus::Active {
                job.handle.status = match outcome.status {
                    JobRunStatus::Completed
                        if matches!(job.handle.spec.frequency, ScheduleFrequency::Once { .. }) =>
                    {
                        WorkflowStatus::Completed
                    }
                    JobRunStatus::Completed => WorkflowStatus::Active,
                    JobRunStatus::Interrupted | JobRunStatus::Cancelled => WorkflowStatus::Paused,
                    _ => WorkflowStatus::Failed,
                };
            }
            job.handle.spec.enabled = job.handle.status == WorkflowStatus::Active;
            Ok(())
        })
    }
    /// Call only while holding the singleton service lease. Never replay a stale claim.
    pub fn recover_interrupted(&self) -> anyhow::Result<usize> {
        if !self.all()?.iter().any(|w| w.in_flight.is_some()) {
            return Ok(0);
        }
        self.records.update::<Records, _>("schedules", |records| {
            let mut count = 0;
            for job in records.workflows.values_mut() {
                if let Some(id) = job.in_flight.take() {
                    let run = job.runs.iter_mut().find(|r| r.id == id).context("missing interrupted run history")?;
                    run.finished_at = Some(Utc::now());
                    run.outcome.status = JobRunStatus::Interrupted;
                    run.outcome.stderr = "Service stopped before a final result was saved. Effects may have happened; inspect the saved session and project before explicitly resuming. This run was not retried.".into();
                    if job.handle.status != WorkflowStatus::Cancelled { job.handle.status = WorkflowStatus::Paused; }
                    job.handle.spec.enabled = false;
                    count += 1;
                }
            }
            Ok(count)
        })
    }
    fn change_status(&self, id: WorkflowId, status: WorkflowStatus) -> anyhow::Result<()> {
        self.records.update::<Records, _>("schedules", |records| {
            let job = records
                .workflows
                .get_mut(&id)
                .context("workflow not found")?;
            if status != WorkflowStatus::Cancelled {
                ensure!(
                    job.handle.status != WorkflowStatus::Cancelled,
                    "cancelled workflows cannot be resumed or paused"
                );
                ensure!(
                    job.handle.status != WorkflowStatus::Completed,
                    "completed one-time workflows cannot be resumed; create a new job"
                );
            }
            if status == WorkflowStatus::Active {
                ensure!(
                    job.in_flight.is_none(),
                    "wait for the current child to stop before resuming"
                );
                // Explicit resumption starts a new attempt; never silently reuses an old claim.
                job.next_run = next_run(&job.handle.spec.frequency, Utc::now(), true)?;
                if let ScheduleFrequency::Watch { pattern } = &job.handle.spec.frequency {
                    job.watch_snapshot = watch_snapshot(self.root(), pattern)?;
                }
            }
            job.handle.status = status;
            job.handle.spec.enabled = status == WorkflowStatus::Active;
            Ok(())
        })
    }
}

#[async_trait::async_trait]
impl WorkflowScheduler for FileWorkflowScheduler {
    async fn create(&self, spec: ScheduleSpec) -> anyhow::Result<WorkflowId> {
        ensure!(
            !spec.label.trim().is_empty() && spec.label.len() <= 256,
            "job label must have 1 to 256 bytes"
        );
        ensure!(
            !spec.objective.trim().is_empty() && spec.objective.len() <= 16_384,
            "job objective must have 1 to 16384 bytes"
        );
        if let Some(pack) = &spec.pack {
            ensure!(
                !pack.is_empty()
                    && pack.len() <= 128
                    && pack
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                "invalid pack name"
            );
        }
        let now = Utc::now();
        let next_run = next_run(&spec.frequency, now, true)?;
        let watch_snapshot = match &spec.frequency {
            ScheduleFrequency::Watch { pattern } => watch_snapshot(self.root(), pattern)?,
            _ => BTreeMap::new(),
        };
        let id = Uuid::new_v4();
        let status = if spec.enabled {
            WorkflowStatus::Active
        } else {
            WorkflowStatus::Paused
        };
        let workflow = WorkflowDetails {
            handle: WorkflowHandle {
                id,
                spec,
                status,
                created_at: now,
                last_run: None,
                run_count: 0,
            },
            next_run,
            in_flight: None,
            runs: Vec::new(),
            scan_failures: Vec::new(),
            watch_snapshot,
        };
        self.records.update::<Records, _>("schedules", |records| {
            ensure!(records.workflows.len() < 1_000, "project job limit reached");
            records.workflows.insert(id, workflow);
            Ok(())
        })?;
        Ok(id)
    }
    async fn get(&self, id: WorkflowId) -> anyhow::Result<WorkflowHandle> {
        Ok(self.details(id)?.handle)
    }
    async fn list(&self) -> anyhow::Result<Vec<WorkflowId>> {
        Ok(self.all()?.iter().map(|j| j.handle.id).collect())
    }
    async fn pause(&self, id: WorkflowId) -> anyhow::Result<()> {
        self.change_status(id, WorkflowStatus::Paused)
    }
    async fn resume(&self, id: WorkflowId) -> anyhow::Result<()> {
        self.change_status(id, WorkflowStatus::Active)
    }
    async fn cancel(&self, id: WorkflowId) -> anyhow::Result<()> {
        self.change_status(id, WorkflowStatus::Cancelled)
    }
}

fn next_run(
    frequency: &ScheduleFrequency,
    now: DateTime<Utc>,
    initial: bool,
) -> anyhow::Result<Option<DateTime<Utc>>> {
    Ok(match frequency {
        ScheduleFrequency::Once { at } => {
            if initial {
                Some(*at)
            } else {
                None
            }
        }
        ScheduleFrequency::Interval { every_secs } => {
            ensure!(
                (1..=315_360_000).contains(every_secs),
                "interval must be between 1 second and 10 years"
            );
            Some(
                now.checked_add_signed(Duration::seconds(*every_secs as i64))
                    .context("interval date overflow")?,
            )
        }
        ScheduleFrequency::Cron { expression } => Some(Cron::parse(expression)?.next_after(now)?),
        ScheduleFrequency::Watch { pattern } => {
            validate_pattern(pattern)?;
            None
        }
    })
}

struct Cron {
    fields: Vec<BTreeSet<u32>>,
    day_any: bool,
    weekday_any: bool,
}
impl Cron {
    fn parse(expression: &str) -> anyhow::Result<Self> {
        ensure!(expression.len() <= 256, "cron expression is too long");
        let parts: Vec<_> = expression.split_whitespace().collect();
        ensure!(parts.len() == 5, "cron requires five numeric fields in UTC");
        let ranges = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)];
        let mut fields = Vec::new();
        for (part, (low, high)) in parts.iter().zip(ranges) {
            let mut values = BTreeSet::new();
            for term in part.split(',') {
                let (base, step) = if let Some((base, step)) = term.split_once('/') {
                    (base, step.parse::<u32>().context("invalid cron step")?)
                } else {
                    (term, 1)
                };
                ensure!(step > 0 && step <= high + 1, "cron step is out of range");
                let (start, end) = if base == "*" {
                    (low, high)
                } else if let Some((start, end)) = base.split_once('-') {
                    (start.parse::<u32>()?, end.parse::<u32>()?)
                } else {
                    let value = base
                        .parse::<u32>()
                        .context("cron supports numeric fields, ranges, lists and steps")?;
                    (value, if term.contains('/') { high } else { value })
                };
                ensure!(
                    start >= low && end <= high && start <= end,
                    "cron value is out of range"
                );
                values.extend((start..=end).step_by(step as usize));
            }
            ensure!(!values.is_empty(), "empty cron field");
            fields.push(values);
        }
        if fields[4].remove(&7) {
            fields[4].insert(0);
        }
        Ok(Self {
            fields,
            day_any: parts[2].starts_with('*'),
            weekday_any: parts[4].starts_with('*'),
        })
    }
    fn matches(&self, t: DateTime<Utc>) -> bool {
        let day = self.fields[2].contains(&t.day());
        let weekday = self.fields[4].contains(&t.weekday().num_days_from_sunday());
        let day_ok = match (self.day_any, self.weekday_any) {
            (true, _) => weekday && day,
            (_, true) => day && weekday,
            _ => day || weekday,
        };
        self.fields[0].contains(&t.minute())
            && self.fields[1].contains(&t.hour())
            && self.fields[3].contains(&t.month())
            && day_ok
    }
    fn next_after(&self, now: DateTime<Utc>) -> anyhow::Result<DateTime<Utc>> {
        let mut candidate = now
            .with_second(0)
            .and_then(|t| t.with_nanosecond(0))
            .context("invalid cron date")?
            .checked_add_signed(Duration::minutes(1))
            .context("cron date overflow")?;
        for _ in 0..(366 * 24 * 60 * 5) {
            if self.matches(candidate) {
                return Ok(candidate);
            }
            candidate = candidate
                .checked_add_signed(Duration::minutes(1))
                .context("cron date overflow")?;
        }
        anyhow::bail!("cron expression has no occurrence within the next five years")
    }
}

fn protected(name: &str) -> bool {
    matches!(
        name,
        ".git" | ".chimera" | ".floyd" | ".supercache" | ".agents" | ".codex" | ".aws" | ".ssh"
    ) || name == ".env"
        || name.starts_with(".env.")
}
fn validate_pattern(pattern: &str) -> anyhow::Result<()> {
    ensure!(
        !pattern.is_empty() && pattern.len() <= 512 && !pattern.starts_with('/'),
        "watch pattern must be project-relative and 1 to 512 bytes"
    );
    ensure!(
        !pattern.contains(['\\', '[', ']', '{', '}', '\0']),
        "watch supports only *, ** and ? glob syntax"
    );
    for part in pattern.split('/') {
        ensure!(
            !part.is_empty() && part != "." && part != ".." && !protected(part),
            "unsafe watch path"
        );
    }
    Ok(())
}
fn glob_matches(pattern: &str, path: &str) -> bool {
    fn visit(
        p: &[char],
        s: &[char],
        i: usize,
        j: usize,
        memo: &mut BTreeMap<(usize, usize), bool>,
    ) -> bool {
        if let Some(v) = memo.get(&(i, j)) {
            return *v;
        }
        let result = if i == p.len() {
            j == s.len()
        } else if p[i] == '*' {
            let double = p.get(i + 1) == Some(&'*');
            let skip = i + if double { 2 } else { 1 };
            visit(p, s, skip, j, memo)
                || (double && p.get(skip) == Some(&'/') && visit(p, s, skip + 1, j, memo))
                || (j < s.len() && (double || s[j] != '/') && visit(p, s, i, j + 1, memo))
        } else {
            j < s.len()
                && (p[i] == s[j] || (p[i] == '?' && s[j] != '/'))
                && visit(p, s, i + 1, j + 1, memo)
        };
        memo.insert((i, j), result);
        result
    }
    visit(
        &pattern.chars().collect::<Vec<_>>(),
        &path.chars().collect::<Vec<_>>(),
        0,
        0,
        &mut BTreeMap::new(),
    )
}
fn watch_snapshot(root: &Path, pattern: &str) -> anyhow::Result<BTreeMap<String, FileStamp>> {
    validate_pattern(pattern)?;
    let root = root
        .canonicalize()
        .context("watch project is unavailable")?;
    // Start at the fixed directory prefix so src/**/*.rs does not scan unrelated trees.
    let parts: Vec<_> = pattern.split('/').collect();
    let mut base = root.clone();
    for part in parts.iter().take(parts.len().saturating_sub(1)) {
        if part.contains(['*', '?']) {
            break;
        }
        base.push(part);
        match std::fs::symlink_metadata(&base) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => (),
            Ok(_) => return Ok(BTreeMap::new()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(e) => return Err(e.into()),
        }
    }
    let mut pending = vec![base];
    let mut snapshot = BTreeMap::new();
    let mut entries = 0;
    let mut bytes = 0;
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            entries += 1;
            ensure!(
                entries <= 10_000,
                "watch scan exceeds 10000 entries; use a narrower pattern or project"
            );
            let name = entry.file_name();
            if protected(&name.to_string_lossy()) {
                continue;
            }
            let meta = std::fs::symlink_metadata(entry.path())?;
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                pending.push(entry.path());
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            let path = entry.path();
            let relative = path
                .strip_prefix(&root)?
                .to_str()
                .context("watch path is not valid UTF-8")?;
            if !glob_matches(pattern, relative) {
                continue;
            }
            ensure!(
                bytes + meta.len() <= 32 * 1024 * 1024,
                "watch contents exceed 32 MiB; use a narrower pattern"
            );
            // A stable content fingerprint catches equal-length writes with restored timestamps.
            // It is a change detector, not a security or integrity hash.
            use std::io::Read;
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(target_os = "macos")]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(0x100);
            } // O_NOFOLLOW
            #[cfg(target_os = "linux")]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(0x20000);
            } // O_NOFOLLOW
            let file = options.open(&path)?;
            ensure!(
                file.metadata()?.is_file(),
                "watched target is not a regular file"
            );
            let mut content = Vec::new();
            file.take(32 * 1024 * 1024 + 1).read_to_end(&mut content)?;
            bytes += content.len() as u64;
            ensure!(bytes <= 32 * 1024 * 1024, "watch contents exceed 32 MiB");
            ensure!(
                content.len() <= 32 * 1024 * 1024,
                "watched file grew beyond 32 MiB"
            );
            let fingerprint = content.iter().fold(0xcbf29ce484222325u64, |h, b| {
                (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
            });
            let modified_nanos = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos());
            snapshot.insert(
                relative.into(),
                FileStamp {
                    bytes: content.len() as u64,
                    modified_nanos,
                    fingerprint,
                },
            );
        }
    }
    Ok(snapshot)
}

// Kept for source compatibility in tests; production callers use FileWorkflowScheduler.
#[cfg(test)]
pub struct MockScheduler {
    inner: FileWorkflowScheduler,
}
#[cfg(test)]
impl MockScheduler {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!("chimera-scheduler-mock-{}", Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self {
            inner: FileWorkflowScheduler::new(path),
        }
    }
}
#[cfg(test)]
impl Default for MockScheduler {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
#[async_trait::async_trait]
impl WorkflowScheduler for MockScheduler {
    async fn create(&self, spec: ScheduleSpec) -> anyhow::Result<WorkflowId> {
        self.inner.create(spec).await
    }
    async fn get(&self, id: WorkflowId) -> anyhow::Result<WorkflowHandle> {
        self.inner.get(id).await
    }
    async fn list(&self) -> anyhow::Result<Vec<WorkflowId>> {
        self.inner.list().await
    }
    async fn pause(&self, id: WorkflowId) -> anyhow::Result<()> {
        self.inner.pause(id).await
    }
    async fn resume(&self, id: WorkflowId) -> anyhow::Result<()> {
        self.inner.resume(id).await
    }
    async fn cancel(&self, id: WorkflowId) -> anyhow::Result<()> {
        self.inner.cancel(id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("ch-job-{}", Uuid::new_v4().simple()));
        std::fs::create_dir(&root).unwrap();
        root
    }
    fn spec(frequency: ScheduleFrequency) -> ScheduleSpec {
        ScheduleSpec {
            label: "test".into(),
            objective: "harmless objective".into(),
            frequency,
            pack: None,
            enabled: true,
        }
    }
    fn success() -> JobOutcome {
        JobOutcome {
            status: JobRunStatus::Completed,
            exit_code: Some(0),
            stdout: "actual output".into(),
            stderr: String::new(),
            output_truncated: false,
        }
    }
    #[tokio::test]
    async fn once_claim_survives_reopen_and_is_never_replayed_after_success() {
        let root = root();
        let store = FileWorkflowScheduler::new(&root);
        let id = store
            .create(spec(ScheduleFrequency::Once {
                at: Utc::now() - Duration::seconds(1),
            }))
            .await
            .unwrap();
        let claim = store.claim_next(Utc::now()).unwrap().unwrap();
        assert!(FileWorkflowScheduler::new(&root)
            .claim_next(Utc::now())
            .unwrap()
            .is_none());
        store.finish(&claim, success()).unwrap();
        let reopened = FileWorkflowScheduler::new(&root);
        assert!(reopened.claim_next(Utc::now()).unwrap().is_none());
        let job = reopened.details(id).unwrap();
        assert_eq!(job.handle.status, WorkflowStatus::Completed);
        assert_eq!(job.runs[0].outcome.stdout, "actual output");
        assert!(reopened.resume(id).await.is_err());
        assert!(store.finish(&claim, success()).is_err());
    }
    #[tokio::test]
    async fn crashed_claim_is_ambiguous_paused_and_requires_explicit_resume() {
        let root = root();
        let store = FileWorkflowScheduler::new(&root);
        let id = store
            .create(spec(ScheduleFrequency::Once { at: Utc::now() }))
            .await
            .unwrap();
        let first = store.claim_next(Utc::now()).unwrap().unwrap();
        let reopened = FileWorkflowScheduler::new(&root);
        assert_eq!(reopened.recover_interrupted().unwrap(), 1);
        let job = reopened.details(id).unwrap();
        assert_eq!(job.handle.status, WorkflowStatus::Paused);
        assert_eq!(job.runs[0].outcome.status, JobRunStatus::Interrupted);
        assert!(job.runs[0]
            .outcome
            .stderr
            .contains("Effects may have happened"));
        assert!(reopened.claim_next(Utc::now()).unwrap().is_none());
        reopened.resume(id).await.unwrap();
        let second = reopened.claim_next(Utc::now()).unwrap().unwrap();
        assert_ne!(first.run_id, second.run_id);
        assert!(store.finish(&first, success()).is_err());
    }
    #[tokio::test]
    async fn pause_cancel_disabled_and_interval_restart_are_durable() {
        let root = root();
        let store = FileWorkflowScheduler::new(&root);
        let mut request = spec(ScheduleFrequency::Interval { every_secs: 1 });
        request.enabled = false;
        let id = store.create(request).await.unwrap();
        assert!(store
            .claim_next(Utc::now() + Duration::days(1))
            .unwrap()
            .is_none());
        store.resume(id).await.unwrap();
        store.pause(id).await.unwrap();
        let reopened = FileWorkflowScheduler::new(&root);
        assert_eq!(
            reopened.get(id).await.unwrap().status,
            WorkflowStatus::Paused
        );
        reopened.resume(id).await.unwrap();
        let now = Utc::now() + Duration::seconds(2);
        let claim = reopened.claim_next(now).unwrap().unwrap();
        reopened.cancel(id).await.unwrap();
        reopened.finish(&claim, success()).unwrap();
        assert_eq!(
            reopened.get(id).await.unwrap().status,
            WorkflowStatus::Cancelled
        );
        assert!(reopened.resume(id).await.is_err());
        assert!(reopened
            .claim_next(now + Duration::days(1))
            .unwrap()
            .is_none());
    }
    #[tokio::test]
    async fn independent_writers_cannot_claim_the_same_run() {
        let root = root();
        let store = FileWorkflowScheduler::new(&root);
        store
            .create(spec(ScheduleFrequency::Once { at: Utc::now() }))
            .await
            .unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let tasks: Vec<_> = (0..4)
            .map(|_| {
                let root = root.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    FileWorkflowScheduler::new(root)
                        .claim_next(Utc::now())
                        .unwrap()
                })
            })
            .collect();
        assert_eq!(
            tasks
                .into_iter()
                .filter_map(|task| task.join().unwrap())
                .count(),
            1
        );
    }
    #[tokio::test]
    async fn watch_detects_real_content_changes_and_excludes_private_paths() {
        let root = root();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.txt"), "one").unwrap();
        let store = FileWorkflowScheduler::new(&root);
        let id = store
            .create(spec(ScheduleFrequency::Watch {
                pattern: "**/*.txt".into(),
            }))
            .await
            .unwrap();
        assert!(store.claim_next(Utc::now()).unwrap().is_none());
        std::fs::write(root.join(".chimera/private.txt"), "secret").unwrap();
        assert!(store.claim_next(Utc::now()).unwrap().is_none());
        std::fs::write(root.join("src/a.txt"), "two").unwrap();
        let claim = store.claim_next(Utc::now()).unwrap().unwrap();
        assert_eq!(claim.workflow_id, id);
        store.finish(&claim, success()).unwrap();
        assert!(FileWorkflowScheduler::new(&root)
            .claim_next(Utc::now())
            .unwrap()
            .is_none());
        std::fs::write(root.join("new.txt"), "new root file").unwrap();
        assert!(store.claim_next(Utc::now()).unwrap().is_some());
    }
    #[tokio::test]
    async fn watcher_scan_failure_is_saved_without_stopping_other_jobs() {
        let root = root();
        let watched = root.join("watched.txt");
        std::fs::write(&watched, "small").unwrap();
        let store = FileWorkflowScheduler::new(&root);
        let watcher = store
            .create(spec(ScheduleFrequency::Watch {
                pattern: "watched.txt".into(),
            }))
            .await
            .unwrap();
        // A sparse temporary file reaches the real scan limit without a large write.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&watched)
            .unwrap()
            .set_len(32 * 1024 * 1024 + 1)
            .unwrap();
        assert!(store.claim_next(Utc::now()).unwrap().is_none());
        let reopened = FileWorkflowScheduler::new(&root);
        let failure = reopened.details(watcher).unwrap();
        assert_eq!(failure.handle.status, WorkflowStatus::Failed);
        assert!(!failure.handle.spec.enabled);
        assert_eq!(failure.handle.run_count, 0);
        assert!(failure.runs.is_empty());
        assert!(failure.in_flight.is_none());
        let saved = serde_json::to_value(&failure).unwrap();
        assert!(saved["scan_failures"][0]["error"]
            .as_str()
            .unwrap()
            .contains("32 MiB"));
        assert!(saved["scan_failures"][0]["at"].is_string());

        let once = reopened
            .create(spec(ScheduleFrequency::Once { at: Utc::now() }))
            .await
            .unwrap();
        let claim = reopened.claim_next(Utc::now()).unwrap().unwrap();
        assert_eq!(claim.workflow_id, once);
        reopened.finish(&claim, success()).unwrap();
        assert!(reopened.claim_next(Utc::now()).unwrap().is_none());
        assert!(reopened.resume(watcher).await.is_err());
        assert_eq!(
            reopened.details(watcher).unwrap().handle.status,
            WorkflowStatus::Failed
        );
        std::fs::write(&watched, "repaired").unwrap();
        reopened.resume(watcher).await.unwrap();
        std::fs::write(&watched, "changed after resumption").unwrap();
        assert_eq!(
            reopened
                .claim_next(Utc::now())
                .unwrap()
                .unwrap()
                .workflow_id,
            watcher
        );
        let saved = serde_json::to_value(reopened.details(watcher).unwrap()).unwrap();
        assert_eq!(saved["scan_failures"].as_array().unwrap().len(), 1);
    }
    #[test]
    fn cron_matches_calendar_and_checks_numeric_fields() {
        let t = DateTime::parse_from_rfc3339("2026-10-11T22:14:59Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            Cron::parse("*/15 * * * *")
                .unwrap()
                .next_after(t)
                .unwrap()
                .minute(),
            15
        );
        let t = DateTime::parse_from_rfc3339("2026-10-11T22:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            Cron::parse("0 9 * * 1-5")
                .unwrap()
                .next_after(t)
                .unwrap()
                .to_rfc3339(),
            "2026-10-12T09:00:00+00:00"
        );
        assert!(Cron::parse("60 * * * *").is_err());
        assert!(Cron::parse("* * * * * *").is_err());
        assert!(Cron::parse("*/0 * * * *").is_err());
        assert!(Cron::parse("0 0 30 2 *").unwrap().next_after(t).is_err());
    }
    #[test]
    fn glob_is_scoped_and_does_not_treat_single_star_as_recursive() {
        assert!(glob_matches("**/*.rs", "main.rs"));
        assert!(glob_matches("src/**/*.rs", "src/a/b.rs"));
        assert!(!glob_matches("*.rs", "src/main.rs"));
        assert!(glob_matches("src/?.rs", "src/a.rs"));
        for pattern in ["../*", "/tmp/*", ".env", ".git/**", "a/[bc]"] {
            assert!(validate_pattern(pattern).is_err());
        }
    }
    #[tokio::test]
    async fn corruption_and_invalid_specs_are_explicit_errors() {
        let root = root();
        let store = FileWorkflowScheduler::new(&root);
        assert!(store
            .create(spec(ScheduleFrequency::Interval { every_secs: 0 }))
            .await
            .is_err());
        store
            .create(spec(ScheduleFrequency::Interval { every_secs: 1 }))
            .await
            .unwrap();
        std::fs::write(root.join(".chimera/jobs/schedules.json"), b"bad json").unwrap();
        assert!(store
            .claim_next(Utc::now())
            .unwrap_err()
            .to_string()
            .contains("corrupt"));
        assert!(store.all().is_err());
    }
}
