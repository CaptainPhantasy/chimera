//! chimera-tui::jobs — In-chat background work manager.
//!
//! Lists this project's scheduled jobs from the real scheduler, shows a
//! chosen job's details, and creates, starts, pauses or stops it. Every
//! change goes through `FileWorkflowScheduler`, and the panel says plainly
//! when the background service is not running.

use crate::dialog::{Choice, Dialog, Field};
use anyhow::{Context, Result};
use chimera_core::daemon::{self, DaemonAction};
use chimera_core::scheduler::{
    parse_frequency, FileWorkflowScheduler, JobRunStatus, ScheduleFrequency, ScheduleSpec,
    WorkflowDetails, WorkflowScheduler, WorkflowStatus,
};
use std::path::{Path, PathBuf};

const TITLE: &str = "Background work";
const OBJECTIVE_CHARS: usize = 200;
const RECENT_RUNS: usize = 3;

/// A scheduler listing or change that runs while the chat screen keeps drawing.
#[derive(Default)]
pub struct Jobs {
    root: Option<PathBuf>,
    jobs: Vec<WorkflowDetails>,
    service: String,
    job: Option<tokio::task::JoinHandle<Result<Refreshed>>>,
}

/// What a finished scheduler request leaves for the overlay.
struct Refreshed {
    message: Option<String>,
    jobs: Vec<WorkflowDetails>,
    service: String,
}

impl Jobs {
    /// Start listing this project's jobs and show a waiting panel.
    pub fn open(&mut self, root: PathBuf) -> Dialog {
        self.begin(root, "Loading background jobs for this project.")
    }

    fn begin(&mut self, root: PathBuf, note: &str) -> Dialog {
        self.cancel();
        self.root = Some(root.clone());
        self.job = Some(tokio::spawn(async move { Self::fetch(&root, None).await }));
        Dialog::busy(TITLE, note)
    }

    /// Read every job from the real scheduler plus how the service stands.
    async fn fetch(root: &Path, message: Option<String>) -> Result<Refreshed> {
        let mut jobs = FileWorkflowScheduler::new(root).all()?;
        jobs.sort_by_key(|job| std::cmp::Reverse((job.handle.created_at, job.handle.id)));
        Ok(Refreshed {
            message,
            jobs,
            service: service_note(root).await,
        })
    }

    fn refresh(&mut self, dialog: &mut Option<Dialog>, message: Option<String>) -> Result<()> {
        let root = self
            .root
            .clone()
            .context("The job list is not open. Use /jobs.")?;
        self.job = Some(tokio::spawn(
            async move { Self::fetch(&root, message).await },
        ));
        *dialog = Some(Dialog::busy(TITLE, "Reading the scheduler."));
        Ok(())
    }

    fn show_list(&mut self, dialog: &mut Option<Dialog>, message: Option<&str>) {
        *dialog = Some(Self::list_dialog(&self.jobs, &self.service, message));
    }

    fn chosen(&self, id: &str) -> Result<&WorkflowDetails> {
        let id = uuid::Uuid::parse_str(id)
            .context("That job could not be identified. Choose it from the list again.")?;
        self.jobs
            .iter()
            .find(|job| job.handle.id == id)
            .context("That job is no longer in the list. Choose Refresh to reload it.")
    }

    /// Abandon a running scheduler request. Safe when nothing is running.
    pub fn cancel(&mut self) {
        if let Some(job) = self.job.take() {
            job.abort();
        }
        self.root = None;
        self.jobs.clear();
        self.service.clear();
    }

    /// Handle one confirmed overlay choice.
    pub async fn action(
        &mut self,
        action: &str,
        values: &[String],
        dialog: &mut Option<Dialog>,
    ) -> Result<Option<String>> {
        if action == "jobs:refresh" {
            self.refresh(dialog, None)?;
        } else if action == "jobs:new" {
            *dialog = Some(Self::new_dialog());
        } else if action == "jobs:create" {
            self.create(values, dialog)?;
        } else if action == "jobs:back" {
            self.show_list(dialog, None);
        } else if let Some(id) = action.strip_prefix("jobs:details:") {
            let detail = self.chosen(id)?;
            *dialog = Some(Self::details_dialog(detail, &self.service));
        } else if let Some(id) = action.strip_prefix("jobs:pause:") {
            self.change(dialog, id, "pause")?;
        } else if let Some(id) = action.strip_prefix("jobs:resume:") {
            self.change(dialog, id, "resume")?;
        } else if let Some(id) = action.strip_prefix("jobs:stop:") {
            let detail = self.chosen(id)?;
            *dialog = Some(Self::stop_dialog(detail));
        } else if let Some(id) = action.strip_prefix("jobs:stop-confirm:") {
            self.change(dialog, id, "cancel")?;
        }
        Ok(None)
    }

    /// Run one pause, resume or stop against the real scheduler, then reload
    /// the list so the panel reports the scheduler's actual state.
    fn change(&mut self, dialog: &mut Option<Dialog>, id: &str, verb: &'static str) -> Result<()> {
        let chosen = {
            let detail = self.chosen(id)?;
            (detail.handle.id, detail.handle.spec.label.clone())
        };
        let root = self
            .root
            .clone()
            .context("The job list is not open. Use /jobs.")?;
        self.job = Some(tokio::spawn(async move {
            let (id, label) = chosen;
            let scheduler = FileWorkflowScheduler::new(&root);
            let result = match verb {
                "pause" => scheduler.pause(id).await,
                "resume" => scheduler.resume(id).await,
                _ => scheduler.cancel(id).await,
            };
            result.map_err(|error| {
                anyhow::anyhow!("The scheduler refused to {verb} \"{label}\": {error:#}")
            })?;
            let message = match verb {
                "pause" => format!("\"{label}\" is paused; the service will not claim it until it is started again."),
                "resume" => format!("\"{label}\" is active again; the service claims it when it is due."),
                _ => format!("\"{label}\" is stopped permanently. Its history stays saved; a stopped job cannot resume."),
            };
            Self::fetch(&root, Some(message)).await
        }));
        *dialog = Some(Dialog::busy(
            TITLE,
            match verb {
                "pause" => "Pausing the job.",
                "resume" => "Starting the job again.",
                _ => "Stopping the job.",
            },
        ));
        Ok(())
    }

    /// Save a new job with the real scheduler from the submitted form.
    fn create(&mut self, values: &[String], dialog: &mut Option<Dialog>) -> Result<()> {
        let [label, objective, schedule, pack] = values else {
            anyhow::bail!("Fill the new-job form and submit it from the form.");
        };
        let label = label.trim();
        anyhow::ensure!(
            !label.is_empty(),
            "Give the job a name so it is recognizable in the list."
        );
        let objective = objective.trim();
        anyhow::ensure!(
            !objective.is_empty(),
            "Describe the work the job should do."
        );
        let frequency = parse_frequency(schedule)
            .map_err(|error| anyhow::anyhow!("That schedule was not understood. {error}"))?;
        let pack = pack.trim();
        let pack = (!pack.is_empty()).then(|| pack.to_string());
        let label = label.to_string();
        let objective = objective.to_string();
        let root = self
            .root
            .clone()
            .context("The job list is not open. Use /jobs.")?;
        self.job = Some(tokio::spawn(async move {
            let scheduler = FileWorkflowScheduler::new(&root);
            let id = scheduler
                .create(ScheduleSpec {
                    label: label.clone(),
                    objective: objective.clone(),
                    frequency,
                    pack,
                    enabled: true,
                })
                .await
                .map_err(|error| anyhow::anyhow!("The scheduler refused this job: {error:#}"))?;
            let message =
                format!("Saved job {id}. It runs only while the background service is up.");
            Self::fetch(&root, Some(message)).await
        }));
        *dialog = Some(Dialog::busy(TITLE, "Saving the job."));
        Ok(())
    }

    /// Move a finished scheduler request into the overlay.
    pub async fn poll(&mut self, dialog: &mut Option<Dialog>) {
        if !self.job.as_ref().is_some_and(|job| job.is_finished()) {
            return;
        }
        let Some(job) = self.job.take() else { return };
        let result = job
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("The scheduler request stopped. Try again.")));
        match result {
            Ok(refreshed) => {
                self.jobs = refreshed.jobs;
                self.service = refreshed.service;
                *dialog = Some(Self::list_dialog(
                    &self.jobs,
                    &self.service,
                    refreshed.message.as_deref(),
                ));
            }
            Err(error) => {
                *dialog = Some(Dialog::choices(
                    TITLE,
                    &error.to_string(),
                    vec![Choice::new(
                        "Try again",
                        "Reload the job list from the scheduler.",
                        "jobs:refresh",
                    )],
                ));
            }
        }
    }

    fn choice(job: &WorkflowDetails) -> Choice {
        Choice::new(
            format!(
                "{} — {}",
                job.handle.spec.label,
                status_text(job.handle.status)
            ),
            format!(
                "{} · {} run(s) · created {}",
                frequency_text(&job.handle.spec.frequency),
                job.handle.run_count,
                when_text(Some(job.handle.created_at)),
            ),
            format!("jobs:details:{}", job.handle.id),
        )
    }

    fn list_dialog(jobs: &[WorkflowDetails], service: &str, message: Option<&str>) -> Dialog {
        let mut rows: Vec<Choice> = jobs.iter().map(Self::choice).collect();
        rows.push(Choice::new(
            "Describe a new job",
            "Save work the scheduler starts on a schedule you choose.",
            "jobs:new",
        ));
        rows.push(Choice::new(
            "Refresh",
            "Reload every job from the scheduler.",
            "jobs:refresh",
        ));
        let lead = message
            .map(|message| format!("{message}\n"))
            .unwrap_or_default();
        let note = if jobs.is_empty() {
            format!("{lead}{service}\nNo background jobs exist yet. Choose \"Describe a new job\" to create one.")
        } else {
            format!("{lead}{service}\nSearch by label or status. Enter shows a job's details and controls.")
        };
        Dialog::choices(TITLE, &note, rows)
    }

    fn details_dialog(detail: &WorkflowDetails, service: &str) -> Dialog {
        let id = detail.handle.id;
        let mut rows = Vec::new();
        match detail.handle.status {
            WorkflowStatus::Active => rows.push(Choice::new(
                "Pause this job",
                "The service stops claiming it until it is started again.",
                format!("jobs:pause:{id}"),
            )),
            WorkflowStatus::Paused | WorkflowStatus::Failed => rows.push(Choice::new(
                "Start this job again",
                "It becomes active and is claimed when it is due.",
                format!("jobs:resume:{id}"),
            )),
            WorkflowStatus::Completed | WorkflowStatus::Cancelled => {}
        }
        if !matches!(
            detail.handle.status,
            WorkflowStatus::Cancelled | WorkflowStatus::Completed
        ) {
            rows.push(Choice::new(
                "Stop this job",
                "Permanent: a stopped job never runs again. You confirm first.",
                format!("jobs:stop:{id}"),
            ));
        }
        rows.push(Choice::new(
            "Describe a new job",
            "Save work the scheduler starts on a schedule you choose.",
            "jobs:new",
        ));
        rows.push(Choice::new(
            "Back to the job list",
            "Return to every saved job.",
            "jobs:back",
        ));
        Dialog::choices(TITLE, &detail_note(detail, service), rows)
    }

    fn stop_dialog(detail: &WorkflowDetails) -> Dialog {
        let id = detail.handle.id;
        Dialog::choices(
            "Stop this job?",
            &format!(
                "\"{}\" will be stopped permanently. It never runs again and cannot be resumed; its history stays saved. This cannot be undone.",
                brief(&detail.handle.spec.label, 80)
            ),
            vec![
                Choice::new(
                    "Stop it permanently",
                    "Cancel this job with the scheduler now.",
                    format!("jobs:stop-confirm:{id}"),
                ),
                Choice::new(
                    "Keep the job",
                    "Return to its details without changing anything.",
                    format!("jobs:details:{id}"),
                ),
            ],
        )
    }

    fn new_dialog() -> Dialog {
        Dialog::form(
            "New background job",
            "The scheduler saves this work and the background service runs it when due. Schedule uses the same words as the command line: hourly, daily, every:<seconds>, once:<RFC 3339 date and time>, or cron:<five numeric UTC fields>.",
            vec![
                Field::new("Name", "", false),
                Field::new("Objective", "", false),
                Field::new("Schedule", "daily", false),
                Field::new("Pack (optional)", "", false),
            ],
            "jobs:create",
        )
    }
}

impl Drop for Jobs {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// The honest state of the background service, which is what executes jobs.
async fn service_note(root: &Path) -> String {
    match daemon::request(root, DaemonAction::Status).await {
        Ok(status) => format!(
            "Service: running as process {} and claims due jobs.",
            status.pid
        ),
        Err(_) => {
            "Service: not running, so no job will execute now. Start it with `chimera up`.".into()
        }
    }
}

fn status_text(status: WorkflowStatus) -> &'static str {
    match status {
        WorkflowStatus::Active => "active",
        WorkflowStatus::Paused => "paused",
        WorkflowStatus::Completed => "completed",
        WorkflowStatus::Failed => "failed",
        WorkflowStatus::Cancelled => "stopped",
    }
}

fn run_status_text(status: JobRunStatus) -> &'static str {
    match status {
        JobRunStatus::Claimed => "running",
        JobRunStatus::Completed => "completed",
        JobRunStatus::Failed => "failed",
        JobRunStatus::Cancelled => "cancelled",
        JobRunStatus::TimedOut => "timed out",
        JobRunStatus::Interrupted => "interrupted",
    }
}

fn frequency_text(frequency: &ScheduleFrequency) -> String {
    match frequency {
        ScheduleFrequency::Once { at } => {
            format!("once at {}", when_text(Some(*at)))
        }
        ScheduleFrequency::Interval { every_secs } => format!("every {every_secs}s"),
        ScheduleFrequency::Cron { expression } => format!("cron {expression}"),
        ScheduleFrequency::Watch { pattern } => format!("watch {pattern}"),
    }
}

fn when_text(at: Option<chrono::DateTime<chrono::Utc>>) -> String {
    at.map(|at| {
        at.with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    })
    .unwrap_or_else(|| "never".into())
}

fn brief(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let mut shown: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        shown.push('…');
    }
    shown.replace('\n', " ")
}

fn detail_note(detail: &WorkflowDetails, service: &str) -> String {
    let handle = &detail.handle;
    let mut lines = vec![
        format!(
            "{} · {} · {} run(s)",
            status_text(handle.status),
            frequency_text(&handle.spec.frequency),
            handle.run_count
        ),
        format!(
            "Objective: {}",
            brief(&handle.spec.objective, OBJECTIVE_CHARS)
        ),
        format!(
            "Created {} · last run {} · next run {}",
            when_text(Some(handle.created_at)),
            when_text(handle.last_run),
            when_text(detail.next_run)
        ),
    ];
    if let Some(pack) = &handle.spec.pack {
        lines.push(format!("Pack: {pack}"));
    }
    if detail.in_flight.is_some() {
        lines.push("A run is in progress right now.".into());
    }
    let total = detail.runs.len();
    let recent: Vec<String> = detail
        .runs
        .iter()
        .enumerate()
        .skip(total.saturating_sub(RECENT_RUNS))
        .map(|(index, run)| {
            format!(
                "Run {} · {} · exit {} · {}",
                index + 1,
                run_status_text(run.outcome.status),
                run.outcome
                    .exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "none".into()),
                when_text(run.finished_at)
            )
        })
        .collect();
    if !recent.is_empty() {
        lines.push("Recent runs:".into());
        lines.extend(recent);
    }
    if let Some(failure) = detail.scan_failures.last() {
        lines.push(format!(
            "Last watcher scan failure: {}",
            brief(&failure.error, 160)
        ));
    }
    lines.push(service.into());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialog::{Body, Outcome};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_root() -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "chimera-jobs-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn success() -> chimera_core::scheduler::JobOutcome {
        chimera_core::scheduler::JobOutcome {
            status: chimera_core::scheduler::JobRunStatus::Completed,
            exit_code: Some(0),
            stdout: "done".into(),
            stderr: String::new(),
            output_truncated: false,
        }
    }

    async fn seed(
        root: &Path,
        label: &str,
        frequency: ScheduleFrequency,
        enabled: bool,
    ) -> uuid::Uuid {
        FileWorkflowScheduler::new(root)
            .create(ScheduleSpec {
                label: label.into(),
                objective: format!("{label} objective"),
                frequency,
                pack: None,
                enabled,
            })
            .await
            .unwrap()
    }

    fn rows(dialog: &Dialog) -> Vec<(String, String, String)> {
        match &dialog.body {
            Body::Choices { rows, .. } => rows
                .iter()
                .map(|row| (row.label.clone(), row.detail.clone(), row.action.clone()))
                .collect(),
            _ => Vec::new(),
        }
    }

    async fn wait_for_choices(jobs: &mut Jobs, dialog: &mut Option<Dialog>) {
        for _ in 0..1000 {
            jobs.poll(dialog).await;
            if matches!(dialog.as_ref().map(|d| &d.body), Some(Body::Choices { .. })) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the scheduler request never finished");
    }

    #[tokio::test]
    async fn listing_reflects_scheduler_state_newest_first_with_status() {
        let root = temp_root();
        let older = seed(
            &root,
            "Older job",
            ScheduleFrequency::Interval { every_secs: 3600 },
            true,
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        let newer = seed(
            &root,
            "Newer job",
            ScheduleFrequency::Interval { every_secs: 86400 },
            false,
        )
        .await;
        let mut jobs = Jobs::default();
        let mut dialog = Some(jobs.open(root.clone()));
        assert!(matches!(dialog.as_ref().map(|d| &d.body), Some(Body::Busy)));
        wait_for_choices(&mut jobs, &mut dialog).await;
        let dialog = dialog.unwrap();
        let listed = rows(&dialog);
        assert_eq!(listed.len(), 4);
        assert_eq!(listed[0].0, "Newer job — paused");
        assert_eq!(listed[1].0, "Older job — active");
        assert_eq!(listed[0].2, format!("jobs:details:{newer}"));
        assert_eq!(listed[1].2, format!("jobs:details:{older}"));
        assert!(listed[1].1.contains("every 3600s"));
        assert!(listed[1].1.contains("0 run(s)"));
        // The panel states the service truth instead of pretending it runs.
        assert!(dialog.note.contains("Service: not running"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_empty_store_shows_a_helpful_empty_state() {
        let root = temp_root();
        let mut jobs = Jobs::default();
        let mut dialog = Some(jobs.open(root.clone()));
        wait_for_choices(&mut jobs, &mut dialog).await;
        let dialog = dialog.unwrap();
        assert!(dialog.note.contains("No background jobs exist yet"));
        assert!(dialog.note.contains("Service: not running"));
        let listed = rows(&dialog);
        assert!(listed.iter().any(|(_, _, action)| action == "jobs:new"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn selecting_a_job_shows_its_details_and_controls() {
        let root = temp_root();
        let id = seed(
            &root,
            "Nightly report",
            ScheduleFrequency::Interval { every_secs: 60 },
            true,
        )
        .await;
        let mut jobs = Jobs::default();
        let mut dialog = Some(jobs.open(root.clone()));
        wait_for_choices(&mut jobs, &mut dialog).await;
        jobs.action(&format!("jobs:details:{id}"), &[], &mut dialog)
            .await
            .unwrap();
        let dialog = dialog.unwrap();
        assert_eq!(dialog.title, TITLE);
        assert!(dialog.note.contains("Objective: Nightly report objective"));
        assert!(dialog.note.contains("every 60s"));
        let listed = rows(&dialog);
        assert!(listed
            .iter()
            .any(|(_, _, action)| action == &format!("jobs:pause:{id}")));
        assert!(listed
            .iter()
            .any(|(_, _, action)| action == &format!("jobs:stop:{id}")));
        assert!(listed.iter().any(|(_, _, action)| action == "jobs:back"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn pause_resume_and_stop_change_the_real_scheduler() {
        let root = temp_root();
        let id = seed(
            &root,
            "Watcher job",
            ScheduleFrequency::Interval { every_secs: 60 },
            true,
        )
        .await;
        let store = FileWorkflowScheduler::new(&root);
        let mut jobs = Jobs::default();
        let mut dialog = Some(jobs.open(root.clone()));
        wait_for_choices(&mut jobs, &mut dialog).await;
        jobs.action(&format!("jobs:pause:{id}"), &[], &mut dialog)
            .await
            .unwrap();
        wait_for_choices(&mut jobs, &mut dialog).await;
        assert_eq!(
            store.details(id).unwrap().handle.status,
            WorkflowStatus::Paused
        );
        assert!(dialog.as_ref().unwrap().note.contains("is paused"));
        jobs.action(&format!("jobs:resume:{id}"), &[], &mut dialog)
            .await
            .unwrap();
        wait_for_choices(&mut jobs, &mut dialog).await;
        assert_eq!(
            store.details(id).unwrap().handle.status,
            WorkflowStatus::Active
        );
        assert!(dialog.as_ref().unwrap().note.contains("is active again"));
        jobs.action(&format!("jobs:stop-confirm:{id}"), &[], &mut dialog)
            .await
            .unwrap();
        wait_for_choices(&mut jobs, &mut dialog).await;
        assert_eq!(
            store.details(id).unwrap().handle.status,
            WorkflowStatus::Cancelled
        );
        assert!(dialog
            .as_ref()
            .unwrap()
            .note
            .contains("stopped permanently"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn stop_requires_an_explicit_confirmation_before_anything_changes() {
        let root = temp_root();
        let id = seed(
            &root,
            "Precious job",
            ScheduleFrequency::Interval { every_secs: 60 },
            true,
        )
        .await;
        let store = FileWorkflowScheduler::new(&root);
        let mut jobs = Jobs::default();
        let mut dialog = Some(jobs.open(root.clone()));
        wait_for_choices(&mut jobs, &mut dialog).await;
        jobs.action(&format!("jobs:stop:{id}"), &[], &mut dialog)
            .await
            .unwrap();
        {
            let dialog = dialog.as_ref().unwrap();
            assert_eq!(dialog.title, "Stop this job?");
            assert!(dialog.note.contains("stopped permanently"));
            assert!(dialog.note.contains("cannot be undone"));
            assert!(rows(dialog)
                .iter()
                .any(|(_, _, action)| action == &format!("jobs:stop-confirm:{id}")));
        }
        assert_eq!(
            store.details(id).unwrap().handle.status,
            WorkflowStatus::Active,
            "nothing may change before the confirmation"
        );
        // Keeping the job returns to its details with the scheduler untouched.
        jobs.action(&format!("jobs:details:{id}"), &[], &mut dialog)
            .await
            .unwrap();
        assert_eq!(
            store.details(id).unwrap().handle.status,
            WorkflowStatus::Active
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn unknown_or_garbage_job_ids_are_refused_and_change_nothing() {
        let root = temp_root();
        let mut jobs = Jobs::default();
        jobs.root = Some(root.clone());
        let mut dialog = Some(Dialog::choices(TITLE, "", Vec::new()));
        let error = jobs
            .action("jobs:details:not-a-uuid", &[], &mut dialog)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("could not be identified"));
        assert!(dialog.is_some());
        let unknown = uuid::Uuid::new_v4();
        let error = jobs
            .action(&format!("jobs:details:{unknown}"), &[], &mut dialog)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no longer in the list"));
        assert!(dialog.is_some());
        let error = jobs
            .action(&format!("jobs:pause:{unknown}"), &[], &mut dialog)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no longer in the list"));
        assert!(jobs.job.is_none());
        assert!(jobs.jobs.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn the_create_form_validates_and_saves_a_real_job() {
        let root = temp_root();
        let store = FileWorkflowScheduler::new(&root);
        let mut jobs = Jobs::default();
        jobs.root = Some(root.clone());
        let mut dialog = Some(Dialog::choices(TITLE, "", Vec::new()));
        jobs.action("jobs:new", &[], &mut dialog).await.unwrap();
        assert!(matches!(dialog.as_ref().unwrap().body, Body::Form { .. }));
        let error = jobs
            .action("jobs:create", &[], &mut dialog)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("new-job form"));
        let error = jobs
            .action(
                "jobs:create",
                &["".into(), "work".into(), "daily".into(), "".into()],
                &mut dialog,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Give the job a name"));
        let error = jobs
            .action(
                "jobs:create",
                &["Named".into(), "work".into(), "sometimes".into(), "".into()],
                &mut dialog,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("schedule was not understood"));
        assert!(store.all().unwrap().is_empty());
        jobs.action(
            "jobs:create",
            &[
                "Named".into(),
                "  do the work  ".into(),
                "every:120".into(),
                " ship-hotfix ".into(),
            ],
            &mut dialog,
        )
        .await
        .unwrap();
        wait_for_choices(&mut jobs, &mut dialog).await;
        let saved = &store.all().unwrap()[0];
        assert_eq!(saved.handle.spec.label, "Named");
        assert_eq!(saved.handle.spec.objective, "do the work");
        assert_eq!(saved.handle.spec.pack.as_deref(), Some("ship-hotfix"));
        assert!(matches!(
            saved.handle.spec.frequency,
            ScheduleFrequency::Interval { every_secs: 120 }
        ));
        assert_eq!(saved.handle.status, WorkflowStatus::Active);
        assert!(dialog.as_ref().unwrap().note.contains("Saved job"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_refused_scheduler_change_reports_the_real_failure() {
        let root = temp_root();
        let id = seed(
            &root,
            "Finished once",
            ScheduleFrequency::Once {
                at: chrono::Utc::now(),
            },
            true,
        )
        .await;
        let store = FileWorkflowScheduler::new(&root);
        let claim = store.claim_next(chrono::Utc::now()).unwrap().unwrap();
        store.finish(&claim, success()).unwrap();
        assert_eq!(
            store.details(id).unwrap().handle.status,
            WorkflowStatus::Completed
        );
        let mut jobs = Jobs::default();
        let mut dialog = Some(jobs.open(root.clone()));
        wait_for_choices(&mut jobs, &mut dialog).await;
        jobs.action(&format!("jobs:resume:{id}"), &[], &mut dialog)
            .await
            .unwrap();
        for _ in 0..1000 {
            jobs.poll(&mut dialog).await;
            if dialog
                .as_ref()
                .is_some_and(|dialog| dialog.note.contains("refused"))
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let dialog = dialog.unwrap();
        assert!(
            dialog.note.contains("refused to resume"),
            "the scheduler's refusal must reach the user: {}",
            dialog.note
        );
        assert_eq!(
            store.details(id).unwrap().handle.status,
            WorkflowStatus::Completed
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn the_search_filters_and_enter_targets_the_chosen_job() {
        let root = temp_root();
        let watcher = seed(
            &root,
            "Payments watcher",
            ScheduleFrequency::Watch {
                pattern: "src/**/*.rs".into(),
            },
            true,
        )
        .await;
        seed(
            &root,
            "Nightly digest",
            ScheduleFrequency::Interval { every_secs: 3600 },
            true,
        )
        .await;
        let mut jobs = Jobs::default();
        let mut dialog = Some(jobs.open(root.clone()));
        wait_for_choices(&mut jobs, &mut dialog).await;
        let mut dialog = dialog.unwrap();
        dialog.paste("watcher");
        assert!(matches!(
            dialog.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Outcome::Action(action, _) if action == format!("jobs:details:{watcher}")
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn cancelling_clears_an_in_flight_scheduler_request() {
        let root = temp_root();
        seed(
            &root,
            "Only job",
            ScheduleFrequency::Interval { every_secs: 60 },
            true,
        )
        .await;
        let mut jobs = Jobs::default();
        let _busy = jobs.open(root.clone());
        assert!(jobs.job.is_some());
        jobs.cancel();
        assert!(jobs.job.is_none());
        assert!(jobs.root.is_none());
        assert!(jobs.jobs.is_empty());
        let mut dialog = None;
        jobs.poll(&mut dialog).await;
        assert!(dialog.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
