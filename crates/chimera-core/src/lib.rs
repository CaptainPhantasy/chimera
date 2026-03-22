pub mod pack;
pub mod scheduler;
pub mod skill;

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{info, info_span, Instrument};
use uuid::Uuid;

use chimera_browser::BrowserController;
use chimera_comms::CommsBridge;
use chimera_creatures::{AutonomyLevel, CreatureRegistry, CreatureSpec, Temperament, WorldKind};
use chimera_eval::Evaluator;
use chimera_mcp::McpClient;
use chimera_memory::MemoryStore;
use chimera_mobile::MobileController;
use chimera_sandbox::WorldManager;
use chimera_session::SessionStore;
use chimera_shell::ShellManager;
use chimera_tools::ToolRouter;
use chimera_trace::TraceSink;

// ---------------------------------------------------------------------------
// ID types
// ---------------------------------------------------------------------------

/// Unique session identifier.
pub type SessionId = Uuid;

/// Unique checkpoint identifier.
pub type CheckpointId = Uuid;

/// Unique creature instance identifier.
pub type CreatureId = Uuid;

/// Unique world instance identifier.
pub type WorldId = Uuid;

// ---------------------------------------------------------------------------
// Execution mode
// ---------------------------------------------------------------------------

/// How the harness should execute an objective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    /// Single coordinated objective with default creature routing.
    Run,
    /// Planning-only mode — no write side effects.
    Plan,
    /// Parallel execution with multiple creatures and worlds.
    Swarm,
    /// Launch a named pack of creatures with predefined skills.
    Pack,
}

// ---------------------------------------------------------------------------
// Evidence
// ---------------------------------------------------------------------------

/// The kind of evidence artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    CommandTranscript,
    FileDiff,
    TraceSpan,
    Screenshot,
    TestResult,
    BrowserReplay,
    ApprovalDecision,
}

/// How much evidence the operator wants captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceLevel {
    /// Minimal — only approvals and errors.
    Minimal,
    /// Standard — diffs, test results, approval records.
    Standard,
    /// Full — all traces, transcripts, screenshots.
    Full,
}

/// A reference to a captured evidence artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRef {
    /// What kind of evidence this is.
    pub kind: EvidenceKind,
    /// URI or path to the artifact.
    pub uri: String,
    /// Human-readable label.
    pub label: String,
    /// When this evidence was captured.
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Run status
// ---------------------------------------------------------------------------

/// The outcome status of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Run is currently executing.
    Running,
    /// Run completed successfully.
    Completed,
    /// Run failed with errors.
    Failed,
    /// Run was cancelled by the operator.
    Cancelled,
    /// Run is paused awaiting approval.
    AwaitingApproval,
    /// Run was rolled back.
    RolledBack,
}

// ---------------------------------------------------------------------------
// Approval
// ---------------------------------------------------------------------------

/// The classification of a proposed action for approval purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalClass {
    /// Read/search/analyze — auto-approve.
    Green,
    /// Patch/install/browser actions — approve by plan or per step.
    Yellow,
    /// Commit/external messaging/infra edits — explicit approval required.
    Orange,
    /// Deploy/delete/prod/secrets/payments — blocked or dual approval.
    Red,
}

/// Record of an approval decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRecord {
    /// Which creature requested approval.
    pub creature_id: CreatureId,
    /// Classification of the proposed action.
    pub class: ApprovalClass,
    /// Description of the proposed action.
    pub action: String,
    /// Whether the action was approved.
    pub approved: bool,
    /// When the decision was made.
    pub decided_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Task report
// ---------------------------------------------------------------------------

/// Report for an individual task within a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskReport {
    /// Human-readable task description.
    pub description: String,
    /// Which creature executed this task.
    pub creature_id: CreatureId,
    /// Whether the task completed successfully.
    pub success: bool,
    /// Evidence collected during this task.
    pub evidence: Vec<EvidenceRef>,
}

// ---------------------------------------------------------------------------
// Objective request
// ---------------------------------------------------------------------------

/// A request from the operator to execute an objective.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectiveRequest {
    /// The objective description in natural language.
    pub objective: String,
    /// How to execute: run, plan, swarm, or pack.
    pub mode: ExecutionMode,
    /// Optional repo root for filesystem-bound work.
    pub repo_root: Option<PathBuf>,
    /// Optional policy profile name to apply.
    pub policy_profile: Option<String>,
    /// How much evidence to capture.
    pub evidence_level: EvidenceLevel,
}

// ---------------------------------------------------------------------------
// Run report
// ---------------------------------------------------------------------------

/// The complete report for a finished (or in-progress) run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    /// Session that owns this run.
    pub session_id: SessionId,
    /// Current status.
    pub status: RunStatus,
    /// Per-task reports.
    pub tasks: Vec<TaskReport>,
    /// Approval decisions made during the run.
    pub approvals: Vec<ApprovalRecord>,
    /// All evidence collected.
    pub evidence: Vec<EvidenceRef>,
    /// Checkpoints created during the run.
    pub checkpoints: Vec<CheckpointId>,
}

// ---------------------------------------------------------------------------
// Harness — the orchestration control plane
// ---------------------------------------------------------------------------

/// The core orchestration harness. Holds trait-object handles to all subsystems.
pub struct Harness {
    pub sessions: Arc<dyn SessionStore>,
    pub creatures: Arc<dyn CreatureRegistry>,
    pub tools: Arc<dyn ToolRouter>,
    pub trace: Arc<dyn TraceSink>,
    pub shells: Arc<dyn ShellManager>,
    pub worlds: Arc<dyn WorldManager>,
    pub browser: Arc<dyn BrowserController>,
    pub mobile: Arc<dyn MobileController>,
    pub comms: Arc<dyn CommsBridge>,
    pub memory: Arc<dyn MemoryStore>,
    pub eval: Arc<dyn Evaluator>,
    pub mcp: Arc<dyn McpClient>,
}

impl Harness {
    /// Execute an objective through the full orchestration loop.
    ///
    /// Phases: intake → decompose → assign creatures → assign worlds →
    /// execute → verify → approve → commit → memory update.
    pub async fn run_objective(&self, req: ObjectiveRequest) -> anyhow::Result<RunReport> {
        let session_id = Uuid::new_v4();

        let root_span = info_span!(
            "harness.run_objective",
            session_id = %session_id,
            objective = %req.objective,
            mode = ?req.mode,
        );

        async {
            // ── Phase 1: Intake ──────────────────────────────────────
            let _intake = info_span!("phase.intake").entered();
            info!(objective = %req.objective, "objective received");
            drop(_intake);

            // ── Phase 2: Decompose into tasks ────────────────────────
            let tasks = {
                let _span = info_span!("phase.decompose").entered();
                let tasks = vec![
                    "analyze codebase structure".to_string(),
                    "identify affected modules".to_string(),
                    "apply changes".to_string(),
                    "run verification checks".to_string(),
                ];
                info!(task_count = tasks.len(), "decomposed objective into tasks");
                tasks
            };

            // ── Phase 3: Assign creatures ────────────────────────────
            let assigned_creatures = {
                let _span = info_span!("phase.assign_creatures").entered();
                let available = self.creatures.builtin();
                let mut assigned = Vec::new();
                for (i, task) in tasks.iter().enumerate() {
                    let creature = if i < available.len() {
                        available[i].clone()
                    } else {
                        available[0].clone()
                    };
                    info!(
                        task = %task,
                        creature = %creature.name,
                        role = %creature.role,
                        "assigned creature to task"
                    );
                    assigned.push((task.clone(), creature));
                }
                assigned
            };

            // ── Phase 4: Assign worlds ───────────────────────────────
            {
                let _span = info_span!("phase.assign_worlds").entered();
                for (task, creature) in &assigned_creatures {
                    let world = creature
                        .preferred_worlds
                        .first()
                        .copied()
                        .unwrap_or(WorldKind::LocalShell);
                    info!(
                        task = %task,
                        creature = %creature.name,
                        world = ?world,
                        "assigned world to creature"
                    );
                }
            }

            // ── Phase 5: Execute ─────────────────────────────────────
            let task_reports = {
                let _span = info_span!("phase.execute").entered();
                let mut reports = Vec::new();
                for (task, creature) in &assigned_creatures {
                    let creature_id = Uuid::new_v4();
                    let exec_span = info_span!(
                        "creature.execute",
                        creature = %creature.name,
                        creature_id = %creature_id,
                        task = %task,
                    );
                    let _enter = exec_span.enter();

                    info!("executing task");

                    // Emit trace event
                    let trace_event = chimera_trace::TraceEvent {
                        timestamp: Utc::now(),
                        level: chimera_trace::TraceLevel::Info,
                        session_id,
                        creature_id: Some(creature_id),
                        span: format!("execute:{}", creature.name),
                        message: format!("executing: {}", task),
                        fields: serde_json::json!({
                            "autonomy": format!("{:?}", creature.autonomy),
                            "temperament": format!("{:?}", creature.temperament),
                        }),
                    };
                    self.trace.emit(trace_event).await?;

                    reports.push(TaskReport {
                        description: task.clone(),
                        creature_id,
                        success: true,
                        evidence: vec![],
                    });
                    info!("task complete");
                }
                reports
            };

            // ── Phase 6: Verify ──────────────────────────────────────
            {
                let _span = info_span!("phase.verify").entered();
                info!(
                    tasks_passed = task_reports.iter().filter(|t| t.success).count(),
                    tasks_total = task_reports.len(),
                    "verification complete"
                );
            }

            // ── Phase 7: Approval ────────────────────────────────────
            let approvals = {
                let _span = info_span!("phase.approval").entered();
                info!("no approval gates triggered (stub mode)");
                vec![]
            };

            // ── Phase 8: Commit / checkpoint ─────────────────────────
            let checkpoints = {
                let _span = info_span!("phase.commit").entered();
                let checkpoint_id = Uuid::new_v4();
                info!(checkpoint_id = %checkpoint_id, "checkpoint created");
                vec![checkpoint_id]
            };

            // ── Phase 9: Memory update ───────────────────────────────
            {
                let _span = info_span!("phase.memory_update").entered();
                info!("memory update complete (stub mode)");
            }

            // ── Build report ─────────────────────────────────────────
            let report = RunReport {
                session_id,
                status: RunStatus::Completed,
                tasks: task_reports,
                approvals,
                evidence: vec![],
                checkpoints,
            };

            info!(
                status = ?report.status,
                task_count = report.tasks.len(),
                "objective run complete"
            );

            Ok(report)
        }
        .instrument(root_span)
        .await
    }
}

// ---------------------------------------------------------------------------
// Default / mock implementations for one-shot execution
// ---------------------------------------------------------------------------

/// A no-op TraceSink that logs events but does not persist them.
pub struct StubTraceSink;

#[async_trait::async_trait]
impl TraceSink for StubTraceSink {
    async fn emit(&self, event: chimera_trace::TraceEvent) -> anyhow::Result<()> {
        info!(
            span = %event.span,
            message = %event.message,
            "trace event"
        );
        Ok(())
    }

    async fn query(
        &self,
        _session_id: chimera_trace::SessionId,
    ) -> anyhow::Result<Vec<chimera_trace::TraceEvent>> {
        Ok(vec![])
    }

    async fn query_creature(
        &self,
        _session_id: chimera_trace::SessionId,
        _creature_id: chimera_trace::CreatureId,
    ) -> anyhow::Result<Vec<chimera_trace::TraceEvent>> {
        Ok(vec![])
    }
}

/// A stub SessionStore that creates in-memory sessions.
pub struct StubSessionStore;

#[async_trait::async_trait]
impl SessionStore for StubSessionStore {
    async fn create(
        &self,
        meta: chimera_session::SessionMeta,
    ) -> anyhow::Result<chimera_session::SessionId> {
        info!(label = %meta.label, "session created (stub)");
        Ok(Uuid::new_v4())
    }

    async fn append_event(
        &self,
        _id: chimera_session::SessionId,
        _event: chimera_session::SessionEvent,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn snapshot(
        &self,
        _id: chimera_session::SessionId,
    ) -> anyhow::Result<chimera_session::CheckpointId> {
        Ok(Uuid::new_v4())
    }

    async fn fork(
        &self,
        _id: chimera_session::SessionId,
    ) -> anyhow::Result<chimera_session::SessionId> {
        Ok(Uuid::new_v4())
    }

    async fn load(
        &self,
        id: chimera_session::SessionId,
    ) -> anyhow::Result<chimera_session::SessionState> {
        Ok(chimera_session::SessionState {
            id,
            meta: chimera_session::SessionMeta {
                label: "stub".into(),
                objective: "stub".into(),
                created_at: Utc::now(),
            },
            events: vec![],
            checkpoints: vec![],
            active: false,
        })
    }

    async fn list(&self) -> anyhow::Result<Vec<chimera_session::SessionId>> {
        Ok(vec![])
    }
}

/// A stub CreatureRegistry providing builtin creature specs.
pub struct BuiltinCreatureRegistry;

impl CreatureRegistry for BuiltinCreatureRegistry {
    fn builtin(&self) -> Vec<CreatureSpec> {
        use chimera_creatures::ToolCapability;

        vec![
            CreatureSpec {
                name: "raven".into(),
                role: "reconnaissance and evidence gathering".into(),
                temperament: Temperament::Cautious,
                toolbelt: vec![
                    ToolCapability { name: "grep".into(), read_only: true },
                    ToolCapability { name: "rg".into(), read_only: true },
                    ToolCapability { name: "browser".into(), read_only: true },
                ],
                preferred_worlds: vec![WorldKind::WorktreeReadOnly],
                autonomy: AutonomyLevel::Medium,
                stop_conditions: vec![],
                escalation_rules: vec![],
                token_budget: Some(50_000),
                deadline_secs: Some(300),
            },
            CreatureSpec {
                name: "mantis".into(),
                role: "code surgery and patch application".into(),
                temperament: Temperament::Balanced,
                toolbelt: vec![
                    ToolCapability { name: "bash".into(), read_only: false },
                    ToolCapability { name: "git".into(), read_only: false },
                ],
                preferred_worlds: vec![WorldKind::WorktreeReadWrite],
                autonomy: AutonomyLevel::Low,
                stop_conditions: vec![],
                escalation_rules: vec![],
                token_budget: Some(80_000),
                deadline_secs: Some(600),
            },
            CreatureSpec {
                name: "hound".into(),
                role: "monitoring and regression hunting".into(),
                temperament: Temperament::Patient,
                toolbelt: vec![
                    ToolCapability { name: "bash".into(), read_only: true },
                    ToolCapability { name: "test-runner".into(), read_only: true },
                ],
                preferred_worlds: vec![WorldKind::Container],
                autonomy: AutonomyLevel::Medium,
                stop_conditions: vec![],
                escalation_rules: vec![],
                token_budget: Some(40_000),
                deadline_secs: Some(900),
            },
            CreatureSpec {
                name: "owl".into(),
                role: "evaluator, critic, and verifier".into(),
                temperament: Temperament::Cautious,
                toolbelt: vec![
                    ToolCapability { name: "analyze".into(), read_only: true },
                ],
                preferred_worlds: vec![WorldKind::LocalShell],
                autonomy: AutonomyLevel::High,
                stop_conditions: vec![],
                escalation_rules: vec![],
                token_budget: Some(30_000),
                deadline_secs: Some(120),
            },
        ]
    }

    fn resolve(&self, name: &str) -> anyhow::Result<CreatureSpec> {
        self.builtin()
            .into_iter()
            .find(|c| c.name == name)
            .ok_or_else(|| anyhow::anyhow!("unknown creature: {}", name))
    }
}

/// A stub ToolRouter that acknowledges calls but does nothing.
pub struct StubToolRouter;

#[async_trait::async_trait]
impl ToolRouter for StubToolRouter {
    async fn call(&self, req: chimera_tools::ToolCall) -> anyhow::Result<chimera_tools::ToolResult> {
        info!(tool = %req.tool, "tool call (stub)");
        Ok(chimera_tools::ToolResult {
            ok: true,
            output: serde_json::json!({"stub": true}),
            evidence: vec![],
        })
    }

    async fn list(&self) -> anyhow::Result<Vec<chimera_tools::ToolDescriptor>> {
        Ok(vec![])
    }

    async fn healthcheck(&self, tool: &str) -> anyhow::Result<bool> {
        info!(tool = %tool, "healthcheck (stub)");
        Ok(true)
    }
}

/// A stub ShellManager for one-shot execution.
pub struct StubShellManager;

#[async_trait::async_trait]
impl ShellManager for StubShellManager {
    async fn spawn_shell(&self, spec: chimera_shell::ShellSpec) -> anyhow::Result<chimera_shell::ShellId> {
        info!(label = ?spec.label, "shell spawned (stub)");
        Ok(Uuid::new_v4())
    }

    async fn exec(&self, _shell: chimera_shell::ShellId, cmd: chimera_shell::ShellCommand) -> anyhow::Result<chimera_shell::ShellOutput> {
        info!(command = %cmd.command, "shell exec (stub)");
        Ok(chimera_shell::ShellOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 0,
        })
    }

    async fn transcript(&self, _shell: chimera_shell::ShellId) -> anyhow::Result<Vec<chimera_shell::ShellEvent>> {
        Ok(vec![])
    }

    async fn close(&self, _shell: chimera_shell::ShellId) -> anyhow::Result<()> {
        Ok(())
    }

    async fn create_worktree(&self, spec: chimera_shell::WorktreeSpec) -> anyhow::Result<chimera_shell::WorktreeHandle> {
        info!(repo = %spec.repo_root.display(), "worktree created (stub)");
        Ok(chimera_shell::WorktreeHandle {
            path: spec.repo_root.join(".chimera-wt-stub"),
            shell_id: Uuid::new_v4(),
            read_only: spec.read_only,
        })
    }

    async fn destroy_worktree(&self, _path: PathBuf) -> anyhow::Result<()> {
        Ok(())
    }

    async fn list_shells(&self) -> anyhow::Result<Vec<chimera_shell::ShellId>> {
        Ok(vec![])
    }
}

/// A stub WorldManager for one-shot execution.
pub struct StubWorldManager;

#[async_trait::async_trait]
impl WorldManager for StubWorldManager {
    async fn allocate(&self, spec: chimera_sandbox::WorldSpec) -> anyhow::Result<chimera_sandbox::WorldId> {
        info!(kind = ?spec.kind, "world allocated (stub)");
        Ok(Uuid::new_v4())
    }

    async fn inspect(&self, id: chimera_sandbox::WorldId) -> anyhow::Result<chimera_sandbox::WorldState> {
        Ok(chimera_sandbox::WorldState {
            id,
            spec: chimera_sandbox::WorldSpec {
                kind: chimera_sandbox::WorldKind::LocalShell,
                image: None,
                root: None,
                env: Default::default(),
                network: false,
                writable: false,
                label: None,
                limits: Default::default(),
            },
            status: chimera_sandbox::WorldStatus::Running,
            created_at: Utc::now(),
            root_path: None,
        })
    }

    async fn pause(&self, _id: chimera_sandbox::WorldId) -> anyhow::Result<()> {
        Ok(())
    }

    async fn resume(&self, _id: chimera_sandbox::WorldId) -> anyhow::Result<()> {
        Ok(())
    }

    async fn destroy(&self, _id: chimera_sandbox::WorldId) -> anyhow::Result<()> {
        Ok(())
    }

    async fn list(&self) -> anyhow::Result<Vec<chimera_sandbox::WorldId>> {
        Ok(vec![])
    }
}

/// A stub BrowserController for one-shot execution.
pub struct StubBrowserController;

#[async_trait::async_trait]
impl BrowserController for StubBrowserController {
    async fn open(&self, target: chimera_browser::BrowserTarget) -> anyhow::Result<chimera_browser::BrowserSessionId> {
        info!(target = ?target, "browser opened (stub)");
        Ok(Uuid::new_v4())
    }

    async fn act(&self, _session: chimera_browser::BrowserSessionId, action: chimera_browser::BrowserAction) -> anyhow::Result<chimera_browser::BrowserArtifact> {
        info!(action = ?action, "browser action (stub)");
        Ok(chimera_browser::BrowserArtifact::Ack)
    }

    async fn screenshot(&self, _session: chimera_browser::BrowserSessionId) -> anyhow::Result<chimera_browser::BrowserEvidence> {
        Ok(chimera_browser::BrowserEvidence {
            kind: "screenshot".into(),
            uri: "stub://screenshot.png".into(),
            label: "stub screenshot".into(),
            captured_at: Utc::now(),
        })
    }

    async fn state(&self, session: chimera_browser::BrowserSessionId) -> anyhow::Result<chimera_browser::BrowserState> {
        Ok(chimera_browser::BrowserState {
            id: session,
            current_url: "about:blank".into(),
            title: "Stub".into(),
            active: true,
            opened_at: Utc::now(),
        })
    }

    async fn close(&self, _session: chimera_browser::BrowserSessionId) -> anyhow::Result<()> {
        Ok(())
    }

    async fn list_sessions(&self) -> anyhow::Result<Vec<chimera_browser::BrowserSessionId>> {
        Ok(vec![])
    }
}

/// A stub MobileController.
pub struct StubMobileController;

#[async_trait::async_trait]
impl MobileController for StubMobileController {
    async fn boot(&self, spec: chimera_mobile::DeviceSpec) -> anyhow::Result<chimera_mobile::DeviceId> {
        info!(profile = %spec.profile, "device booted (stub)");
        Ok(Uuid::new_v4())
    }

    async fn perform(&self, _id: chimera_mobile::DeviceId, action: chimera_mobile::DeviceAction) -> anyhow::Result<chimera_mobile::DeviceArtifact> {
        info!(action = ?action, "device action (stub)");
        Ok(chimera_mobile::DeviceArtifact::Ack)
    }

    async fn state(&self, id: chimera_mobile::DeviceId) -> anyhow::Result<chimera_mobile::DeviceState> {
        Ok(chimera_mobile::DeviceState {
            id,
            spec: chimera_mobile::DeviceSpec {
                profile: "stub".into(),
                os_version: None,
                network: false,
                label: None,
            },
            status: chimera_mobile::DeviceStatus::Running,
            booted_at: Some(Utc::now()),
        })
    }

    async fn shutdown(&self, _id: chimera_mobile::DeviceId) -> anyhow::Result<()> {
        Ok(())
    }

    async fn list_devices(&self) -> anyhow::Result<Vec<chimera_mobile::DeviceId>> {
        Ok(vec![])
    }
}

/// A stub CommsBridge.
pub struct StubCommsBridge;

#[async_trait::async_trait]
impl CommsBridge for StubCommsBridge {
    async fn notify(&self, msg: chimera_comms::Notification) -> anyhow::Result<chimera_comms::MessageId> {
        info!(subject = %msg.subject, "notification sent (stub)");
        Ok(Uuid::new_v4())
    }

    async fn request_approval(&self, prompt: chimera_comms::CommsApprovalPrompt) -> anyhow::Result<chimera_comms::PendingApprovalId> {
        info!(actor = %prompt.actor, action = %prompt.action, "approval requested (stub)");
        Ok(Uuid::new_v4())
    }

    async fn check_approval(&self, _id: chimera_comms::PendingApprovalId) -> anyhow::Result<Option<bool>> {
        Ok(Some(true))
    }

    async fn receipt(&self, _id: chimera_comms::MessageId) -> anyhow::Result<Option<chimera_comms::DeliveryReceipt>> {
        Ok(None)
    }
}

/// A stub MemoryStore.
pub struct StubMemoryStore;

#[async_trait::async_trait]
impl MemoryStore for StubMemoryStore {
    async fn remember(&self, item: chimera_memory::MemoryItem) -> anyhow::Result<chimera_memory::MemoryId> {
        info!(key = %item.key, layer = ?item.layer, "memory stored (stub)");
        Ok(item.id)
    }

    async fn retrieve(&self, _query: chimera_memory::MemoryQuery) -> anyhow::Result<Vec<chimera_memory::MemoryItem>> {
        Ok(vec![])
    }

    async fn forget(&self, _id: chimera_memory::MemoryId) -> anyhow::Result<()> {
        Ok(())
    }

    async fn compact(&self, _layer: chimera_memory::MemoryLayer) -> anyhow::Result<u64> {
        Ok(0)
    }
}

/// A stub Evaluator.
pub struct StubEvaluator;

#[async_trait::async_trait]
impl Evaluator for StubEvaluator {
    async fn grade(&self, req: chimera_eval::EvalRequest) -> anyhow::Result<chimera_eval::EvalReport> {
        info!(name = %req.name, "evaluation run (stub)");
        Ok(chimera_eval::EvalReport {
            name: req.name,
            passed: true,
            score: 1.0,
            grades: vec![],
            completed_at: Utc::now(),
        })
    }

    async fn compare(&self, a: chimera_eval::CheckpointId, b: chimera_eval::CheckpointId) -> anyhow::Result<chimera_eval::DiffReport> {
        Ok(chimera_eval::DiffReport {
            checkpoint_a: a,
            checkpoint_b: b,
            files_changed: vec![],
            summary: "no changes (stub)".into(),
            regression_detected: false,
        })
    }
}

/// A stub McpClient.
pub struct StubMcpClient;

#[async_trait::async_trait]
impl McpClient for StubMcpClient {
    async fn list_tools(&self, server: &str) -> anyhow::Result<Vec<chimera_mcp::McpToolDescriptor>> {
        info!(server = %server, "list MCP tools (stub)");
        Ok(vec![])
    }

    async fn invoke(&self, server: &str, tool: &str, _args: serde_json::Value) -> anyhow::Result<chimera_mcp::McpToolResult> {
        info!(server = %server, tool = %tool, "MCP invoke (stub)");
        Ok(chimera_mcp::McpToolResult {
            is_error: false,
            content: vec![chimera_mcp::McpContent::Text { text: "stub result".into() }],
        })
    }

    async fn list_resources(&self, _server: &str) -> anyhow::Result<Vec<chimera_mcp::McpResource>> {
        Ok(vec![])
    }

    async fn read_resource(&self, _server: &str, _uri: &str) -> anyhow::Result<chimera_mcp::McpContent> {
        Ok(chimera_mcp::McpContent::Text { text: String::new() })
    }

    async fn ping(&self, server: &str) -> anyhow::Result<bool> {
        info!(server = %server, "MCP ping (stub)");
        Ok(true)
    }
}
