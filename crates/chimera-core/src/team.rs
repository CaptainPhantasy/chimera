//! Persisted specialist teams using the same real, saved model loop as one-person runs.
//! File capabilities are enforced here; a role description never grants permission.
use crate::{
    agent::{AgentConfig, DoneReason},
    pack::{ExecutionScope, PackSpec},
    runtime::{event_channel, RunEvent, SavedSession},
};
use anyhow::{Context, Result};
use chimera_llm::LlmProvider;
use chimera_tools::{
    DefaultToolRouter, ToolCall, ToolDescriptor, ToolPolicy, ToolResult, ToolRouter,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{mpsc, watch, Semaphore};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamRequest {
    pub objective: String,
    pub creatures: Vec<String>,
    pub scope: ExecutionScope,
    pub max_parallel: usize,
    pub read_only: bool,
}
impl TeamRequest {
    pub fn swarm(objective: String) -> Self {
        Self {
            objective,
            creatures: vec![
                "raven".into(),
                "mantis".into(),
                "hound".into(),
                "owl".into(),
            ],
            scope: ExecutionScope::default(),
            max_parallel: 4,
            read_only: false,
        }
    }
    pub fn from_pack(pack: &PackSpec, objective: String) -> Result<Self> {
        anyhow::ensure!(
            pack.skills.is_empty(),
            "pack skill loading is unavailable; no team started"
        );
        Ok(Self {
            objective: pack
                .objective_template
                .as_ref()
                .map(|t| t.replace("{objective}", &objective))
                .unwrap_or(objective),
            creatures: pack.creatures.clone(),
            scope: pack.scope.clone(),
            max_parallel: 4,
            read_only: false,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamStatus {
    Queued,
    Running,
    Completed,
    Unfinished,
    Failed,
    Cancelled,
    Interrupted,
}
impl TeamStatus {
    fn active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    EveryAction,
    Writes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecialistSpec {
    pub name: String,
    pub role: String,
    pub tools: Vec<String>,
    pub timeout_secs: u64,
    pub max_tool_calls: u32,
}
/// These roles expose only implemented file tools. Tests and commits require a
/// separate approved shell/world workflow; these specialists cannot claim those ran.
pub fn specialists() -> Vec<SpecialistSpec> {
    [("raven", "Inspect files and gather cited evidence. Report findings and uncertainties.", false),
     ("mantis", "Make the smallest requested file changes. Read before writing and explain each change.", true),
     ("hound", "Inspect files for defects and regression risks. Distinguish inspection from tests actually run.", false),
     ("owl", "Independently review evidence and report gaps. Do not claim unperformed checks.", false)]
        .into_iter().map(|(name, role, writes)| SpecialistSpec { name: name.into(), role: role.into(), tools: if writes {vec!["read_file", "write_file", "edit_file", "glob", "grep"]} else {vec!["read_file", "glob", "grep"]}.into_iter().map(str::to_owned).collect(), timeout_secs: 600, max_tool_calls: 100 }).collect()
}
pub fn builtin_packs() -> Vec<PackSpec> {
    vec![PackSpec { name: "ship-hotfix".into(), description: "Inspect, patch with approval, and review files; shell tests and publishing remain separate actions.".into(), creatures: vec!["raven".into(), "mantis".into(), "hound".into(), "owl".into()], scope: ExecutionScope::default(), objective_template: Some("Fix: {objective}".into()), skills: vec![] },
         PackSpec { name: "incident-response".into(), description: "Inspect local evidence and propose a response; no network or external messages.".into(), creatures: vec!["raven".into(), "hound".into(), "owl".into()], scope: ExecutionScope::default(), objective_template: None, skills: vec![] }]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberRecord {
    pub id: Uuid,
    pub specialist: SpecialistSpec,
    pub session_id: Option<Uuid>,
    pub status: TeamStatus,
    pub approval_mode: ApprovalMode,
    pub stop_requested: bool,
    pub tool_calls: u32,
    pub final_answer: Option<String>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamRecord {
    pub id: Uuid,
    pub request: TeamRequest,
    pub status: TeamStatus,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub stop_requested: bool,
    pub members: Vec<MemberRecord>,
    pub write_claims: BTreeMap<String, Uuid>,
}
pub struct TeamEvent {
    pub team_id: Uuid,
    pub member_id: Uuid,
    pub event: RunEvent,
}

#[derive(Clone)]
pub struct TeamRegistry {
    root: PathBuf,
    storage: PathBuf,
}
struct Lock(File);
impl Lock {
    fn take(path: &Path, nonblocking: bool) -> Result<Option<Self>> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
            #[cfg(target_os = "macos")]
            options.custom_flags(0x100);
            #[cfg(not(target_os = "macos"))]
            options.custom_flags(0x20000);
        }
        let file = options.open(path)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe extern "C" {
                fn flock(fd: i32, op: i32) -> i32;
            }
            if unsafe { flock(file.as_raw_fd(), 2 | if nonblocking { 4 } else { 0 }) } != 0 {
                let error = std::io::Error::last_os_error();
                if nonblocking && error.kind() == std::io::ErrorKind::WouldBlock {
                    return Ok(None);
                }
                return Err(error.into());
            }
            Ok(Some(Self(file)))
        }
        #[cfg(not(unix))]
        {
            let _ = (file, nonblocking);
            anyhow::bail!("team ownership locking is unavailable on this platform")
        }
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe extern "C" {
                fn flock(fd: i32, op: i32) -> i32;
            }
            unsafe {
                flock(self.0.as_raw_fd(), 8);
            }
        }
    }
}
fn directory(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) => anyhow::ensure!(
            m.is_dir() && !m.file_type().is_symlink(),
            "unsafe team directory: {}",
            path.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(path)?,
        Err(e) => return Err(e.into()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
impl TeamRegistry {
    pub fn new(root: PathBuf) -> Result<Self> {
        let root = root.canonicalize()?;
        anyhow::ensure!(root.is_dir(), "project must be a directory");
        directory(&root.join(".chimera"))?;
        let storage = root.join(".chimera/teams");
        directory(&storage)?;
        directory(&storage.join("history"))?;
        Ok(Self { root, storage })
    }
    fn lock(&self) -> Result<Lock> {
        Ok(Lock::take(&self.storage.join("state.lock"), false)?.unwrap())
    }
    fn active_lock(&self, id: Uuid) -> Result<Option<Lock>> {
        Lock::take(&self.storage.join(format!("{id}.active")), true)
    }
    fn load(&self, id: Uuid) -> Result<TeamRecord> {
        let path = self.storage.join(format!("{id}.json"));
        let m = std::fs::symlink_metadata(&path).context("team not found")?;
        anyhow::ensure!(
            m.is_file() && !m.file_type().is_symlink() && m.len() <= 4_194_304,
            "unsafe or oversized team record"
        );
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            #[cfg(target_os = "macos")]
            options.custom_flags(0x100);
            #[cfg(not(target_os = "macos"))]
            options.custom_flags(0x20000);
        }
        let mut data = Vec::new();
        options.open(path)?.take(4_194_305).read_to_end(&mut data)?;
        anyhow::ensure!(data.len() <= 4_194_304, "team record exceeds limit");
        let record: TeamRecord = serde_json::from_slice(&data).context("team record is damaged")?;
        anyhow::ensure!(record.id == id, "team record identity mismatch");
        validate_request(&record.request)?;
        Ok(record)
    }
    fn save(&self, record: &TeamRecord) -> Result<()> {
        let path = self.storage.join(format!("{}.json", record.id));
        if path.exists() {
            let m = std::fs::symlink_metadata(&path)?;
            anyhow::ensure!(
                m.is_file() && !m.file_type().is_symlink(),
                "unsafe team record"
            );
            std::fs::hard_link(
                &path,
                self.storage
                    .join("history")
                    .join(format!("{}-{}.json", record.id, Uuid::new_v4())),
            )?;
        }
        let temp = self
            .storage
            .join("history")
            .join(format!("{}-pending.json", Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        let bytes = serde_json::to_vec_pretty(record)?;
        anyhow::ensure!(bytes.len() <= 4_194_304, "team record exceeds limit");
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(temp, path)?;
        File::open(&self.storage)?.sync_all()?;
        Ok(())
    }
    fn ids(&self) -> Result<Vec<Uuid>> {
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(&self.storage)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json") {
                ids.push(Uuid::parse_str(
                    path.file_stem()
                        .and_then(|x| x.to_str())
                        .context("invalid team record name")?,
                )?);
            }
        }
        ids.sort();
        Ok(ids)
    }
    pub fn create(&self, request: TeamRequest) -> Result<TeamRecord> {
        validate_request(&request)?;
        let members = request
            .creatures
            .iter()
            .map(|name| MemberRecord {
                id: Uuid::new_v4(),
                specialist: specialists().into_iter().find(|s| &s.name == name).unwrap(),
                session_id: None,
                status: TeamStatus::Queued,
                approval_mode: ApprovalMode::Writes,
                stop_requested: false,
                tool_calls: 0,
                final_answer: None,
                error: None,
            })
            .collect();
        let record = TeamRecord {
            id: Uuid::new_v4(),
            request,
            status: TeamStatus::Queued,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
            stop_requested: false,
            members,
            write_claims: BTreeMap::new(),
        };
        let _lock = self.lock()?;
        self.save(&record)?;
        Ok(record)
    }
    pub fn inspect(&self, id: Uuid) -> Result<TeamRecord> {
        let _lock = self.lock()?;
        let mut record = self.load(id)?;
        if record.status == TeamStatus::Running && self.active_lock(id)?.is_some() {
            record.status = TeamStatus::Interrupted;
            record.finished_at = Some(Utc::now());
            for member in &mut record.members {
                if member.status.active() {
                    member.status = TeamStatus::Interrupted;
                    member.error=Some("runner stopped before saving an outcome; inspect saved session before retrying".into());
                }
            }
            self.save(&record)?;
        }
        Ok(record)
    }
    pub fn list(&self) -> Result<Vec<TeamRecord>> {
        self.ids()?.into_iter().map(|id| self.inspect(id)).collect()
    }
    pub fn find_member(&self, member: Uuid) -> Result<(TeamRecord, MemberRecord)> {
        for team in self.list()? {
            if let Some(found) = team.members.iter().find(|m| m.id == member).cloned() {
                return Ok((team, found));
            }
        }
        anyhow::bail!("specialist instance not found: {member}")
    }
    fn update(
        &self,
        id: Uuid,
        f: impl FnOnce(&mut TeamRecord) -> Result<()>,
    ) -> Result<TeamRecord> {
        let _lock = self.lock()?;
        let mut record = self.load(id)?;
        f(&mut record)?;
        self.save(&record)?;
        Ok(record)
    }
    pub fn stop(&self, id: Uuid) -> Result<TeamRecord> {
        self.update(id, |r| {
            anyhow::ensure!(r.status.active(), "team is already finished");
            r.stop_requested = true;
            if r.status == TeamStatus::Queued {
                r.status = TeamStatus::Cancelled;
                r.finished_at = Some(Utc::now());
                for m in &mut r.members {
                    m.status = TeamStatus::Cancelled;
                    m.stop_requested = true;
                }
            }
            Ok(())
        })
    }
    pub fn stop_member(&self, id: Uuid, member: Uuid) -> Result<TeamRecord> {
        self.update(id, |r| {
            let m = r
                .members
                .iter_mut()
                .find(|m| m.id == member)
                .context("specialist not found")?;
            anyhow::ensure!(m.status.active(), "specialist is already finished");
            m.stop_requested = true;
            if m.status == TeamStatus::Queued {
                m.status = TeamStatus::Cancelled;
                m.error = Some("stopped before starting".into());
            }
            Ok(())
        })
    }
    pub fn leash(&self, id: Uuid, member: Uuid) -> Result<TeamRecord> {
        self.set_mode(id, member, ApprovalMode::EveryAction)
    }
    /// Promotion permits reads without asking; writes still need exact approval.
    pub fn promote(&self, id: Uuid, member: Uuid) -> Result<TeamRecord> {
        self.set_mode(id, member, ApprovalMode::Writes)
    }
    fn set_mode(&self, id: Uuid, member: Uuid, mode: ApprovalMode) -> Result<TeamRecord> {
        self.update(id, |r| {
            let m = r
                .members
                .iter_mut()
                .find(|m| m.id == member)
                .context("specialist not found")?;
            anyhow::ensure!(m.status.active(), "specialist is already finished");
            m.approval_mode = mode;
            Ok(())
        })
    }

    /// Executes in the caller's lifetime. The daemon may own this caller; this
    /// registry never starts a second detached process or repeats an old job.
    pub async fn execute(
        &self,
        id: Uuid,
        provider: Arc<dyn LlmProvider>,
        model: String,
        config: AgentConfig,
        events: mpsc::Sender<TeamEvent>,
        mut cancellation: watch::Receiver<bool>,
    ) -> Result<TeamRecord> {
        let _lease = self
            .active_lock(id)?
            .context("team already has a live runner")?;
        let record = self.update(id, |r| {
            anyhow::ensure!(
                r.status == TeamStatus::Queued && !r.stop_requested,
                "only a queued team can start; old work is never replayed"
            );
            r.status = TeamStatus::Running;
            r.started_at = Some(Utc::now());
            Ok(())
        })?;
        let _interrupted = RunGuard {
            registry: self.clone(),
            id,
        };
        let tools = Arc::new(DefaultToolRouter::with_policy(
            Some(self.root.clone()),
            if record.request.read_only {
                ToolPolicy::read_only()
            } else {
                ToolPolicy::supervised()
            },
        ));
        let permits = Arc::new(Semaphore::new(record.request.max_parallel));
        let (stop, stopped) = watch::channel(*cancellation.borrow());
        let mut cancellation_open = true;
        let mut jobs = tokio::task::JoinSet::new();
        for member in &record.members {
            let registry = self.clone();
            let member = member.clone();
            let provider = provider.clone();
            let model = model.clone();
            let mut config = config.clone();
            let tools = tools.clone();
            let permits = permits.clone();
            let events = events.clone();
            let cancelled = stopped.clone();
            let objective = record.request.objective.clone();
            config.system_prompt=format!("{}\n\nYour specialist role: {}\nOther specialists are working concurrently. Only report actions you actually performed. File ownership conflicts must stop the action.",config.system_prompt,member.specialist.role);
            jobs.spawn(async move {
                registry
                    .run_member(
                        id, member, provider, model, config, tools, permits, events, cancelled,
                        objective,
                    )
                    .await
            });
        }
        let mut fatal = None;
        let mut controls = tokio::time::interval(Duration::from_millis(30));
        while !jobs.is_empty() {
            tokio::select! {
                result=jobs.join_next()=>{match result {Some(Ok(Ok(())))=>{},Some(Ok(Err(e)))=>{fatal=Some(format!("{e:#}"));stop.send_replace(true);},Some(Err(e))=>{fatal=Some(format!("specialist task stopped: {e}"));stop.send_replace(true);},None=>{}}},
                _=controls.tick()=>{
                    match self.load(id){Ok(r)=>{let timed_out=r.request.scope.timeout_secs.is_some_and(|n|r.started_at.is_some_and(|s|(Utc::now()-s).num_milliseconds()>=n as i64*1000));if r.stop_requested||timed_out||*cancellation.borrow(){stop.send_replace(true);}},Err(e)=>{fatal=Some(format!("could not read team controls: {e:#}"));stop.send_replace(true);}}
                },
                changed=cancellation.changed(),if cancellation_open=>{if changed.is_err(){cancellation_open=false;stop.send_replace(true);}else if *cancellation.borrow(){stop.send_replace(true);}}
            }
        }
        self.update(id, |r| {
            for m in &mut r.members {
                if m.status.active() {
                    m.status = TeamStatus::Failed;
                    m.error = fatal
                        .clone()
                        .or_else(|| Some("specialist ended without a saved result".into()));
                }
            }
            r.status =
                if fatal.is_some() || r.members.iter().any(|m| m.status == TeamStatus::Failed) {
                    TeamStatus::Failed
                } else if r.members.iter().any(|m| m.status == TeamStatus::Cancelled) {
                    TeamStatus::Cancelled
                } else if r.members.iter().all(|m| m.status == TeamStatus::Completed) {
                    TeamStatus::Completed
                } else {
                    TeamStatus::Unfinished
                };
            r.finished_at = Some(Utc::now());
            Ok(())
        })
    }
    #[allow(clippy::too_many_arguments)]
    async fn run_member(
        &self,
        team: Uuid,
        member: MemberRecord,
        provider: Arc<dyn LlmProvider>,
        model: String,
        config: AgentConfig,
        tools: Arc<DefaultToolRouter>,
        permits: Arc<Semaphore>,
        events: mpsc::Sender<TeamEvent>,
        mut cancellation: watch::Receiver<bool>,
        objective: String,
    ) -> Result<()> {
        let permit = tokio::select! {p=permits.acquire_owned()=>Some(p?),_=async{if !*cancellation.borrow(){let _=cancellation.changed().await;}}=>None};
        if permit.is_none()
            || *cancellation.borrow()
            || self
                .load(team)?
                .members
                .iter()
                .find(|m| m.id == member.id)
                .context("specialist missing")?
                .stop_requested
        {
            return self.finish_member(
                team,
                member.id,
                TeamStatus::Cancelled,
                None,
                Some("stopped before starting".into()),
            );
        }
        let mut session = SavedSession::create(self.root.clone(), &objective).await?;
        let session_id = session.id;
        self.update(team, |r| {
            let m = r
                .members
                .iter_mut()
                .find(|m| m.id == member.id)
                .context("specialist missing")?;
            m.status = TeamStatus::Running;
            m.session_id = Some(session_id);
            Ok(())
        })?;
        let router = Arc::new(ScopedRouter {
            registry: self.clone(),
            team,
            member: member.id,
            tools,
            approvals: std::sync::Mutex::new(BTreeMap::new()),
        });
        let (out, mut receive) = event_channel();
        let (stop, stopped) = watch::channel(false);
        let started = tokio::time::Instant::now();
        let mut stop_reason = None;
        let mut failed = false;
        let mut controls = tokio::time::interval(Duration::from_millis(20));
        let run = session.run(
            provider.as_ref(),
            router,
            &model,
            &objective,
            &config,
            out,
            stopped,
        );
        tokio::pin!(run);
        let result = loop {
            tokio::select! {
                result = &mut run => break result,
                event = receive.recv() => {
                    if let Some(event) = event {
                        if events.try_send(TeamEvent { team_id: team, member_id: member.id, event }).is_err() {
                            failed = true;
                            stop_reason = Some("team output reader stopped keeping up".to_string());
                            stop.send_replace(true);
                        }
                    }
                },
                _ = controls.tick() => {
                    match self.load(team) {
                        Ok(control) => {
                            let member_stopped = control.members.iter().find(|m|m.id == member.id)
                                .is_none_or(|m|m.stop_requested);
                            if *cancellation.borrow() || member_stopped || control.stop_requested {
                                stop_reason.get_or_insert_with(|| "stopped by operator or team deadline".into());
                                stop.send_replace(true);
                            }
                        }
                        Err(error) => {
                            failed = true;
                            stop_reason = Some(format!("could not read specialist controls: {error:#}"));
                            stop.send_replace(true);
                        }
                    }
                    if started.elapsed() >= Duration::from_secs(member.specialist.timeout_secs) {
                        stop_reason.get_or_insert_with(|| "specialist deadline reached".into());
                        stop.send_replace(true);
                    }
                }
            }
        };
        while let Ok(event) = receive.try_recv() {
            let _ = events.try_send(TeamEvent {
                team_id: team,
                member_id: member.id,
                event,
            });
        }
        let (status, answer, error) = match result {
            Ok(r) => match r.done_reason {
                DoneReason::FinalAnswer => (TeamStatus::Completed, r.final_answer, None),
                DoneReason::MaxIterations => (
                    TeamStatus::Unfinished,
                    r.final_answer,
                    Some("iteration limit reached".into()),
                ),
                DoneReason::Error => (TeamStatus::Failed, r.final_answer, r.error),
            },
            Err(e) => (
                if stop_reason.is_some() && !failed {
                    TeamStatus::Cancelled
                } else {
                    TeamStatus::Failed
                },
                None,
                Some(stop_reason.unwrap_or_else(|| format!("{e:#}"))),
            ),
        };
        self.finish_member(team, member.id, status, answer, error)
    }
    fn finish_member(
        &self,
        team: Uuid,
        member: Uuid,
        status: TeamStatus,
        answer: Option<String>,
        error: Option<String>,
    ) -> Result<()> {
        self.update(team, |r| {
            let m = r
                .members
                .iter_mut()
                .find(|m| m.id == member)
                .context("specialist missing")?;
            m.status = status;
            m.final_answer = answer.map(|text| {
                let mut preview: String = text.chars().take(16_384).collect();
                if preview.len() < text.len() {
                    preview.push_str(
                        "\n[Answer preview shortened; full answer is in the saved session.]",
                    );
                }
                preview
            });
            m.error = error;
            Ok(())
        })?;
        Ok(())
    }
}
struct RunGuard {
    registry: TeamRegistry,
    id: Uuid,
}
impl Drop for RunGuard {
    fn drop(&mut self) {
        let _ = self.registry.update(self.id, |r| {
            if r.status == TeamStatus::Running {
                r.status = TeamStatus::Interrupted;
                r.finished_at = Some(Utc::now());
                for m in &mut r.members {
                    if m.status.active() {
                        m.status = TeamStatus::Interrupted;
                        m.error = Some("execution ended before a saved outcome".into());
                    }
                }
            }
            Ok(())
        });
    }
}

fn validate_request(r: &TeamRequest) -> Result<()> {
    anyhow::ensure!(
        !r.objective.trim().is_empty() && r.objective.len() <= 65_536,
        "team objective must contain 1 to 65536 bytes"
    );
    anyhow::ensure!(
        (1..=16).contains(&r.creatures.len()) && (1..=8).contains(&r.max_parallel),
        "team needs 1 to 16 specialists and 1 to 8 parallel slots"
    );
    for name in &r.creatures {
        anyhow::ensure!(
            specialists().iter().any(|s| &s.name == name),
            "unsupported specialist: {name}"
        );
    }
    anyhow::ensure!(
        !r.scope.network && !r.scope.can_commit && !r.scope.can_message_external,
        "team network, commits and external messages are unavailable; no team started"
    );
    anyhow::ensure!(r.scope.total_token_budget.is_none(),"hard total-token budgets require provider token reservation support, which is unavailable; no team started");
    anyhow::ensure!(
        r.scope
            .timeout_secs
            .is_none_or(|v| (1..=86_400).contains(&v)),
        "team deadline must be 1 to 86400 seconds"
    );
    anyhow::ensure!(
        !r.scope.allowed_paths.is_empty()
            && r.scope.allowed_paths.len() <= 128
            && r.scope.denied_paths.len() <= 128,
        "team scope needs a bounded nonempty allowed path list"
    );
    for pattern in r.scope.allowed_paths.iter().chain(&r.scope.denied_paths) {
        if pattern == "**" {
            continue;
        }
        let bare = pattern.strip_suffix("/**").unwrap_or(pattern);
        normalized(bare)?;
        anyhow::ensure!(
            !bare.contains(['*', '?', '[', ']']),
            "scope supports literal paths, directory/**, or ** only: {pattern}"
        );
    }
    Ok(())
}
fn normalized(path: &str) -> Result<String> {
    anyhow::ensure!(
        !path.is_empty() && path.len() <= 4096 && !path.contains('\\'),
        "invalid scoped path"
    );
    let p = Path::new(path);
    anyhow::ensure!(
        !p.is_absolute(),
        "team tools require project-relative paths"
    );
    let mut pieces = Vec::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => pieces.push(s.to_str().context("path must be valid UTF-8")?),
            Component::CurDir => {}
            _ => anyhow::bail!("path escapes team scope"),
        }
    }
    Ok(pieces.join("/"))
}
fn scope_matches(pattern: &str, path: &str) -> bool {
    pattern == "**"
        || pattern == path
        || pattern
            .strip_suffix("/**")
            .is_some_and(|p| path == p || path.starts_with(&format!("{p}/")))
}
fn allowed(scope: &ExecutionScope, path: &str) -> bool {
    scope.allowed_paths.iter().any(|p| scope_matches(p, path))
        && !scope.denied_paths.iter().any(|p| scope_matches(p, path))
}

struct ScopedRouter {
    registry: TeamRegistry,
    team: Uuid,
    member: Uuid,
    tools: Arc<DefaultToolRouter>,
    approvals: std::sync::Mutex<BTreeMap<String, std::time::Instant>>,
}
impl ScopedRouter {
    fn check(&self, call: &ToolCall) -> Result<(TeamRecord, MemberRecord)> {
        let record = self.registry.load(self.team)?;
        let member = record
            .members
            .iter()
            .find(|m| m.id == self.member)
            .context("specialist not found")?
            .clone();
        anyhow::ensure!(
            record.status == TeamStatus::Running
                && !record.stop_requested
                && !member.stop_requested,
            "team or specialist stopped"
        );
        anyhow::ensure!(
            matches!(
                call.tool.as_str(),
                "read_file" | "write_file" | "edit_file" | "glob" | "grep"
            ) && member.specialist.tools.contains(&call.tool),
            "tool is outside specialist capability: {}",
            call.tool
        );
        anyhow::ensure!(
            call.world.is_none(),
            "team uses explicit file scope; requested world is unsupported"
        );
        let writes = matches!(call.tool.as_str(), "write_file" | "edit_file");
        anyhow::ensure!(!writes || !record.request.read_only, "team is read-only");
        if matches!(call.tool.as_str(), "read_file" | "write_file" | "edit_file") {
            let p = normalized(
                call.args["path"]
                    .as_str()
                    .context("tool path is required")?,
            )?;
            anyhow::ensure!(
                allowed(&record.request.scope, &p),
                "path is outside team scope: {p}"
            );
        }
        if let Some(path) = call.args.get("path") {
            normalized(path.as_str().context("path must be text")?)?;
        }
        Ok((record, member))
    }
    fn before_call(&self, call: &ToolCall) -> Result<()> {
        let _lock = self.registry.lock()?;
        let (mut record, member) = self.check(call)?;
        anyhow::ensure!(
            member.tool_calls < member.specialist.max_tool_calls.min(100),
            "specialist tool-call limit reached"
        );
        if matches!(call.tool.as_str(), "write_file" | "edit_file") {
            let path = normalized(call.args["path"].as_str().context("path required")?)?;
            for id in self.registry.ids()? {
                let mut other = self.registry.load(id)?;
                if other.status == TeamStatus::Running && self.registry.active_lock(id)?.is_some() {
                    other.status = TeamStatus::Interrupted;
                    other.finished_at = Some(Utc::now());
                    self.registry.save(&other)?;
                }
                if other.status.active() {
                    if let Some(owner) = other.write_claims.get(&path) {
                        anyhow::ensure!(
                            *owner == self.member,
                            "file conflict: {path} is owned by specialist {owner} in team {id}"
                        );
                    }
                }
            }
            record.write_claims.insert(path, self.member);
        }
        record
            .members
            .iter_mut()
            .find(|m| m.id == self.member)
            .unwrap()
            .tool_calls += 1;
        self.registry.save(&record)
    }
}
#[async_trait::async_trait]
impl ToolRouter for ScopedRouter {
    async fn list(&self) -> Result<Vec<ToolDescriptor>> {
        let r = self.registry.load(self.team)?;
        let m = r
            .members
            .iter()
            .find(|m| m.id == self.member)
            .context("specialist not found")?;
        Ok(self
            .tools
            .list()
            .await?
            .into_iter()
            .filter(|d| {
                m.specialist.tools.contains(&d.name) && (!r.request.read_only || d.read_only)
            })
            .collect())
    }
    async fn approval_required(&self, call: &ToolCall) -> Result<bool> {
        let (_, m) = self.check(call)?;
        Ok(m.approval_mode == ApprovalMode::EveryAction
            || self.tools.approval_required(call).await?)
    }
    async fn approve_once(&self, call: &ToolCall) -> Result<()> {
        self.check(call)?;
        self.approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("approval state unavailable"))?
            .insert(
                serde_json::to_string(call)?,
                std::time::Instant::now() + Duration::from_secs(120),
            );
        if self.tools.approval_required(call).await? {
            self.tools.approve_once(call).await?;
        }
        Ok(())
    }
    async fn deny(&self, call: &ToolCall) -> Result<()> {
        self.approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("approval state unavailable"))?
            .remove(&serde_json::to_string(call)?);
        self.tools.deny(call).await
    }
    async fn healthcheck(&self, tool: &str) -> Result<bool> {
        anyhow::ensure!(
            self.list().await?.iter().any(|d| d.name == tool),
            "tool is outside specialist capability"
        );
        self.tools.healthcheck(tool).await
    }
    async fn call(&self, call: ToolCall) -> Result<ToolResult> {
        let (record, member) = self.check(&call)?;
        let grant = self
            .approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("approval state unavailable"))?
            .remove(&serde_json::to_string(&call)?);
        anyhow::ensure!(
            member.approval_mode != ApprovalMode::EveryAction
                || grant.is_some_and(|v| v > std::time::Instant::now()),
            "leashed specialist requires approval for this exact action"
        );
        self.before_call(&call)?;
        if call.tool == "glob" || call.tool == "grep" {
            // Discover names with the built-in safe walker, then read only files
            // inside the effective scope. Denied content never reaches grep.
            let base = normalized(
                call.args
                    .get("path")
                    .and_then(|p| p.as_str())
                    .unwrap_or("."),
            )?;
            let pattern = if call.tool == "glob" {
                call.args["pattern"].as_str().context("pattern required")?
            } else {
                "**/*"
            };
            let result = self
                .tools
                .call(ToolCall {
                    tool: "glob".into(),
                    args: serde_json::json!({"pattern":pattern,"path":base}),
                    world: None,
                    requested_by: call.requested_by,
                })
                .await?;
            let names = result.output["matches"]
                .as_array()
                .context("file listing is invalid")?;
            let paths: Vec<String> = names
                .iter()
                .filter_map(|v| v.as_str())
                .map(|p| {
                    normalized(&if base.is_empty() {
                        p.to_string()
                    } else {
                        format!("{base}/{p}")
                    })
                })
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .filter(|p| allowed(&record.request.scope, p))
                .collect();
            if call.tool == "glob" {
                return Ok(ToolResult {
                    ok: true,
                    output: serde_json::json!({"matches":paths,"truncated":names.len()>=200}),
                    evidence: vec![],
                });
            }
            let limit = call
                .args
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(100)
                .clamp(1, 200) as usize;
            let mut matches = Vec::new();
            for path in paths {
                if matches.len() >= limit {
                    break;
                }
                let mut args = call.args.clone();
                args["path"] = serde_json::json!(path);
                args["max_results"] = serde_json::json!(limit - matches.len());
                let result = self
                    .tools
                    .call(ToolCall {
                        tool: "grep".into(),
                        args,
                        world: None,
                        requested_by: call.requested_by,
                    })
                    .await?;
                for mut value in result.output["matches"]
                    .as_array()
                    .context("search output invalid")?
                    .clone()
                {
                    value["path"] = serde_json::json!(path);
                    matches.push(value);
                }
            }
            return Ok(ToolResult {
                ok: true,
                output: serde_json::json!({"count":matches.len(),"truncated":names.len()>=200||matches.len()>=limit,"matches":matches}),
                evidence: vec![],
            });
        }
        self.tools.call(call).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chimera_llm::{CompletionRequest, CompletionResponse, LlmError};
    use std::sync::atomic::{AtomicUsize, Ordering};
    fn fixture() -> TeamRegistry {
        let root = std::env::temp_dir().join(format!("chimera-team-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        TeamRegistry::new(root).unwrap()
    }
    fn request(names: &[&str]) -> TeamRequest {
        TeamRequest {
            objective: "inspect this project".into(),
            creatures: names.iter().map(|s| s.to_string()).collect(),
            scope: ExecutionScope::default(),
            max_parallel: 4,
            read_only: false,
        }
    }
    fn call(member: Uuid, tool: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            tool: tool.into(),
            args,
            world: None,
            requested_by: member,
        }
    }
    struct Concurrent {
        arrived: AtomicUsize,
        barrier: tokio::sync::Barrier,
    }
    #[async_trait::async_trait]
    impl LlmProvider for Concurrent {
        fn name(&self) -> &str {
            "controlled-concurrency"
        }
        async fn complete(
            &self,
            req: &CompletionRequest,
        ) -> std::result::Result<CompletionResponse, LlmError> {
            assert!(req
                .messages
                .first()
                .unwrap()
                .content
                .as_ref()
                .unwrap()
                .contains("specialist role"));
            self.arrived.fetch_add(1, Ordering::SeqCst);
            self.barrier.wait().await;
            Ok(CompletionResponse {
                content: Some("inspected controlled evidence".into()),
                tool_calls: vec![],
                finish_reason: Some("stop".into()),
                usage: None,
            })
        }
    }
    #[tokio::test]
    async fn two_real_saved_loops_run_concurrently_and_survive_registry_restart() {
        let registry = fixture();
        let team = registry.create(request(&["raven", "owl"])).unwrap();
        let provider = Arc::new(Concurrent {
            arrived: AtomicUsize::new(0),
            barrier: tokio::sync::Barrier::new(2),
        });
        let (events, mut receive) = mpsc::channel::<TeamEvent>(128);
        let (_cancel, cancel) = watch::channel(false);
        let drain = tokio::spawn(async move {
            while let Some(event) = receive.recv().await {
                if let RunEvent::Approval { answer, .. } = event.event {
                    let _ = answer.send(false);
                }
            }
        });
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            registry.execute(
                team.id,
                provider.clone(),
                "controlled".into(),
                AgentConfig::default(),
                events,
                cancel,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        drain.await.unwrap();
        assert_eq!(provider.arrived.load(Ordering::SeqCst), 2);
        assert_eq!(result.status, TeamStatus::Completed);
        assert_ne!(result.members[0].session_id, result.members[1].session_id);
        let reopened = TeamRegistry::new(registry.root.clone()).unwrap();
        assert_eq!(
            reopened.inspect(team.id).unwrap().status,
            TeamStatus::Completed
        );
        for member in result.members {
            let saved = SavedSession::resume(registry.root.clone(), member.session_id.unwrap())
                .await
                .unwrap();
            assert_eq!(
                saved
                    .messages
                    .iter()
                    .filter(|m| m.role == chimera_llm::Role::User)
                    .count(),
                1
            );
            assert!(member.final_answer.unwrap().contains("controlled evidence"));
        }
    }
    struct Stalled;
    #[async_trait::async_trait]
    impl LlmProvider for Stalled {
        fn name(&self) -> &str {
            "controlled-stall"
        }
        async fn complete(
            &self,
            _: &CompletionRequest,
        ) -> std::result::Result<CompletionResponse, LlmError> {
            std::future::pending().await
        }
    }
    #[tokio::test]
    async fn saved_stop_from_another_registry_cancels_live_work() {
        let registry = fixture();
        let team = registry.create(request(&["raven"])).unwrap();
        let runner = registry.clone();
        let (events, _receive) = mpsc::channel::<TeamEvent>(128);
        let (_cancel, cancel) = watch::channel(false);
        let job = tokio::spawn(async move {
            runner
                .execute(
                    team.id,
                    Arc::new(Stalled),
                    "test".into(),
                    AgentConfig::default(),
                    events,
                    cancel,
                )
                .await
        });
        for _ in 0..100 {
            if registry.load(team.id).unwrap().members[0]
                .session_id
                .is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        TeamRegistry::new(registry.root.clone())
            .unwrap()
            .stop(team.id)
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), job)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(result.status, TeamStatus::Cancelled);
        assert_eq!(result.members[0].status, TeamStatus::Cancelled);
        let id = result.members[0].session_id.unwrap();
        let saved = SavedSession::resume(registry.root, id).await.unwrap();
        assert!(saved
            .messages
            .iter()
            .any(|m| m.content.as_deref() == Some("inspect this project")));
    }
    #[tokio::test]
    async fn limits_and_unsupported_scope_fail_before_any_model_call() {
        let registry = fixture();
        let mut r = request(&["raven"]);
        r.scope.total_token_budget = Some(100);
        assert!(registry
            .create(r)
            .unwrap_err()
            .to_string()
            .contains("token"));
        let mut r = request(&["raven"]);
        r.scope.allowed_paths = vec!["../secret".into()];
        assert!(registry.create(r).is_err());
        let mut r = request(&["raven"]);
        r.scope.network = true;
        assert!(registry.create(r).is_err());
        assert!(registry.list().unwrap().is_empty());
        let mut r = request(&["raven"]);
        r.scope.timeout_secs = Some(1);
        let team = registry.create(r).unwrap();
        let (events, _receive) = mpsc::channel::<TeamEvent>(128);
        let (_cancel, cancel) = watch::channel(false);
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            registry.execute(
                team.id,
                Arc::new(Stalled),
                "test".into(),
                AgentConfig::default(),
                events,
                cancel,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result.status, TeamStatus::Cancelled);
        assert!(result.members[0]
            .error
            .as_ref()
            .unwrap()
            .contains("deadline"));
    }
    fn scoped(
        registry: &TeamRegistry,
        team: Uuid,
        member: Uuid,
        tools: Arc<DefaultToolRouter>,
    ) -> ScopedRouter {
        ScopedRouter {
            registry: registry.clone(),
            team,
            member,
            tools,
            approvals: std::sync::Mutex::new(BTreeMap::new()),
        }
    }
    #[tokio::test]
    async fn scopes_leash_and_file_conflicts_are_enforced_at_dispatch() {
        let registry = fixture();
        std::fs::create_dir(registry.root.join("src")).unwrap();
        std::fs::create_dir(registry.root.join("secrets")).unwrap();
        std::fs::write(registry.root.join("src/a.txt"), "needle public").unwrap();
        std::fs::write(registry.root.join("secrets/key"), "needle SECRET").unwrap();
        let mut req = request(&["mantis", "mantis"]);
        req.scope.allowed_paths = vec!["src/**".into()];
        let team = registry.create(req).unwrap();
        let _lease = registry.active_lock(team.id).unwrap().unwrap();
        registry
            .update(team.id, |r| {
                r.status = TeamStatus::Running;
                Ok(())
            })
            .unwrap();
        let tools = Arc::new(DefaultToolRouter::new(Some(registry.root.clone())));
        let first = scoped(&registry, team.id, team.members[0].id, tools.clone());
        let second = scoped(&registry, team.id, team.members[1].id, tools);
        assert!(first
            .call(call(
                first.member,
                "read_file",
                serde_json::json!({"path":"secrets/key"})
            ))
            .await
            .is_err());
        assert!(first
            .call(call(
                first.member,
                "bash",
                serde_json::json!({"command":"pwd"})
            ))
            .await
            .is_err());
        let listed = first
            .call(call(
                first.member,
                "glob",
                serde_json::json!({"path":"src","pattern":"**/*"}),
            ))
            .await
            .unwrap();
        assert_eq!(listed.output["matches"], serde_json::json!(["src/a.txt"]));
        let searched = first
            .call(call(
                first.member,
                "grep",
                serde_json::json!({"pattern":"needle"}),
            ))
            .await
            .unwrap();
        assert!(!searched.output.to_string().contains("SECRET"));
        assert_eq!(searched.output["matches"][0]["path"], "src/a.txt");
        let read = call(
            first.member,
            "read_file",
            serde_json::json!({"path":"src/a.txt"}),
        );
        registry.leash(team.id, first.member).unwrap();
        assert!(first.approval_required(&read).await.unwrap());
        assert!(first.call(read.clone()).await.is_err());
        first.approve_once(&read).await.unwrap();
        first.call(read.clone()).await.unwrap();
        registry.promote(team.id, first.member).unwrap();
        assert!(!first.approval_required(&read).await.unwrap());
        let read_second = call(
            second.member,
            "read_file",
            serde_json::json!({"path":"src/a.txt"}),
        );
        second.call(read_second).await.unwrap();
        let write = call(
            first.member,
            "write_file",
            serde_json::json!({"path":"src/a.txt","content":"first writer"}),
        );
        assert!(first.approval_required(&write).await.unwrap());
        first.approve_once(&write).await.unwrap();
        first.call(write).await.unwrap();
        let write = call(
            second.member,
            "write_file",
            serde_json::json!({"path":"src/a.txt","content":"second writer"}),
        );
        second.approve_once(&write).await.unwrap();
        assert!(second
            .call(write)
            .await
            .unwrap_err()
            .to_string()
            .contains("file conflict"));
        assert_eq!(
            std::fs::read_to_string(registry.root.join("src/a.txt")).unwrap(),
            "first writer"
        );
        registry
            .update(team.id, |r| {
                r.members[0].tool_calls = 100;
                Ok(())
            })
            .unwrap();
        assert!(first
            .call(read)
            .await
            .unwrap_err()
            .to_string()
            .contains("limit"));
    }
    #[test]
    fn abandoned_run_is_interrupted_and_cannot_be_reported_successful() {
        let registry = fixture();
        let team = registry.create(request(&["raven"])).unwrap();
        registry
            .update(team.id, |r| {
                r.status = TeamStatus::Running;
                r.members[0].status = TeamStatus::Running;
                Ok(())
            })
            .unwrap();
        let record = registry.inspect(team.id).unwrap();
        assert_eq!(record.status, TeamStatus::Interrupted);
        assert_eq!(record.members[0].status, TeamStatus::Interrupted);
        assert!(record.members[0].error.is_some());
    }
    struct Limited {
        active: AtomicUsize,
        peak: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl LlmProvider for Limited {
        fn name(&self) -> &str {
            "controlled-limit"
        }
        async fn complete(
            &self,
            _: &CompletionRequest,
        ) -> std::result::Result<CompletionResponse, LlmError> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(40)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(CompletionResponse {
                content: Some("finished actual controlled request".into()),
                tool_calls: vec![],
                finish_reason: Some("stop".into()),
                usage: None,
            })
        }
    }
    #[tokio::test]
    async fn parallel_limit_is_enforced_for_real_requests() {
        let registry = fixture();
        let mut request = request(&["raven", "raven", "raven", "raven", "raven"]);
        request.max_parallel = 2;
        let team = registry.create(request).unwrap();
        let provider = Arc::new(Limited {
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        });
        let (events, _receive) = mpsc::channel::<TeamEvent>(128);
        let (_stop, stopped) = watch::channel(false);
        let outcome = registry
            .execute(
                team.id,
                provider.clone(),
                "test".into(),
                AgentConfig::default(),
                events,
                stopped,
            )
            .await
            .unwrap();
        assert_eq!(outcome.status, TeamStatus::Completed);
        assert_eq!(provider.peak.load(Ordering::SeqCst), 2);
        assert_eq!(
            outcome
                .members
                .iter()
                .filter(|m| m.session_id.is_some())
                .count(),
            5
        );
    }
    struct Denied {
        calls: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl LlmProvider for Denied {
        fn name(&self) -> &str {
            "controlled-denial"
        }
        async fn complete(
            &self,
            _: &CompletionRequest,
        ) -> std::result::Result<CompletionResponse, LlmError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(CompletionResponse {
                content: None,
                tool_calls: vec![chimera_llm::ToolCall {
                    id: "denied-write".into(),
                    kind: "function".into(),
                    function: chimera_llm::FunctionCall {
                        name: "write_file".into(),
                        arguments: r#"{"path":"denied.txt","content":"must not exist"}"#.into(),
                    },
                }],
                finish_reason: Some("tool_calls".into()),
                usage: None,
            })
        }
    }
    #[tokio::test]
    async fn denied_team_action_never_becomes_a_successful_member() {
        let registry = fixture();
        let team = registry.create(request(&["mantis"])).unwrap();
        let provider = Arc::new(Denied {
            calls: AtomicUsize::new(0),
        });
        let (events, mut receive) = mpsc::channel::<TeamEvent>(128);
        let (_stop, stopped) = watch::channel(false);
        let decisions = tokio::spawn(async move {
            while let Some(event) = receive.recv().await {
                if let RunEvent::Approval { answer, .. } = event.event {
                    let _ = answer.send(false);
                }
            }
        });
        let outcome = tokio::time::timeout(
            Duration::from_secs(3),
            registry.execute(
                team.id,
                provider.clone(),
                "test".into(),
                AgentConfig::default(),
                events,
                stopped,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        decisions.await.unwrap();
        assert_eq!(outcome.status, TeamStatus::Failed);
        assert_eq!(outcome.members[0].status, TeamStatus::Failed);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert!(!registry.root.join("denied.txt").exists());
    }
    #[tokio::test]
    async fn dropping_the_team_future_preserves_an_interrupted_outcome() {
        let registry = fixture();
        let team = registry.create(request(&["raven"])).unwrap();
        let runner = registry.clone();
        let (events, _receive) = mpsc::channel::<TeamEvent>(128);
        let (_stop, stopped) = watch::channel(false);
        let job = tokio::spawn(async move {
            runner
                .execute(
                    team.id,
                    Arc::new(Stalled),
                    "test".into(),
                    AgentConfig::default(),
                    events,
                    stopped,
                )
                .await
        });
        for _ in 0..100 {
            if registry.load(team.id).unwrap().members[0]
                .session_id
                .is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        job.abort();
        assert!(job.await.unwrap_err().is_cancelled());
        let saved = registry.inspect(team.id).unwrap();
        assert_eq!(saved.status, TeamStatus::Interrupted);
        assert_eq!(saved.members[0].status, TeamStatus::Interrupted);
    }
}
