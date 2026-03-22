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

// ---------------------------------------------------------------------------
// Helper: build a fully-wired harness from stubs
// ---------------------------------------------------------------------------

/// Build a Harness with all stub/mock backends for testing.
pub fn build_stub_harness() -> Harness {
    Harness {
        sessions: Arc::new(StubSessionStore),
        creatures: Arc::new(BuiltinCreatureRegistry),
        tools: Arc::new(StubToolRouter),
        trace: Arc::new(StubTraceSink),
        shells: Arc::new(StubShellManager),
        worlds: Arc::new(StubWorldManager),
        browser: Arc::new(StubBrowserController),
        mobile: Arc::new(StubMobileController),
        comms: Arc::new(StubCommsBridge),
        memory: Arc::new(StubMemoryStore),
        eval: Arc::new(StubEvaluator),
        mcp: Arc::new(StubMcpClient),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Harness tests ────────────────────────────────────────────

    #[tokio::test]
    async fn harness_run_objective_completes() {
        let harness = build_stub_harness();
        let req = ObjectiveRequest {
            objective: "test objective".into(),
            mode: ExecutionMode::Run,
            repo_root: None,
            policy_profile: None,
            evidence_level: EvidenceLevel::Standard,
        };
        let report = harness.run_objective(req).await.unwrap();
        assert_eq!(report.status, RunStatus::Completed);
        assert_eq!(report.tasks.len(), 4);
        assert!(report.tasks.iter().all(|t| t.success));
        assert_eq!(report.checkpoints.len(), 1);
    }

    #[tokio::test]
    async fn harness_run_plan_mode() {
        let harness = build_stub_harness();
        let req = ObjectiveRequest {
            objective: "plan something".into(),
            mode: ExecutionMode::Plan,
            repo_root: None,
            policy_profile: None,
            evidence_level: EvidenceLevel::Minimal,
        };
        let report = harness.run_objective(req).await.unwrap();
        assert_eq!(report.status, RunStatus::Completed);
    }

    #[tokio::test]
    async fn harness_run_swarm_mode() {
        let harness = build_stub_harness();
        let req = ObjectiveRequest {
            objective: "swarm something".into(),
            mode: ExecutionMode::Swarm,
            repo_root: Some(PathBuf::from("/tmp")),
            policy_profile: Some("strict".into()),
            evidence_level: EvidenceLevel::Full,
        };
        let report = harness.run_objective(req).await.unwrap();
        assert_eq!(report.status, RunStatus::Completed);
    }

    // ── StubTraceSink ────────────────────────────────────────────

    #[tokio::test]
    async fn stub_trace_sink_emit_and_query() {
        let sink = StubTraceSink;
        let event = chimera_trace::TraceEvent {
            timestamp: Utc::now(),
            level: chimera_trace::TraceLevel::Info,
            session_id: Uuid::new_v4(),
            creature_id: None,
            span: "test".into(),
            message: "test msg".into(),
            fields: serde_json::json!({}),
        };
        sink.emit(event).await.unwrap();

        let results = sink.query(Uuid::new_v4()).await.unwrap();
        assert!(results.is_empty());

        let results = sink.query_creature(Uuid::new_v4(), Uuid::new_v4()).await.unwrap();
        assert!(results.is_empty());
    }

    // ── StubSessionStore ─────────────────────────────────────────

    #[tokio::test]
    async fn stub_session_store_full_lifecycle() {
        let store = StubSessionStore;
        let meta = chimera_session::SessionMeta {
            label: "test".into(),
            objective: "test obj".into(),
            created_at: Utc::now(),
        };
        let id = store.create(meta).await.unwrap();

        store
            .append_event(id, chimera_session::SessionEvent::Created {
                meta: chimera_session::SessionMeta {
                    label: "test".into(),
                    objective: "test".into(),
                    created_at: Utc::now(),
                },
            })
            .await
            .unwrap();

        let _cp = store.snapshot(id).await.unwrap();
        let _fork = store.fork(id).await.unwrap();

        let state = store.load(id).await.unwrap();
        assert_eq!(state.meta.label, "stub");

        let list = store.list().await.unwrap();
        assert!(list.is_empty());
    }

    // ── BuiltinCreatureRegistry ──────────────────────────────────

    #[test]
    fn builtin_creature_registry_has_four() {
        let reg = BuiltinCreatureRegistry;
        let builtins = reg.builtin();
        assert_eq!(builtins.len(), 4);
        let names: Vec<&str> = builtins.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"raven"));
        assert!(names.contains(&"mantis"));
        assert!(names.contains(&"hound"));
        assert!(names.contains(&"owl"));
    }

    #[test]
    fn builtin_creature_registry_resolve() {
        let reg = BuiltinCreatureRegistry;
        let raven = reg.resolve("raven").unwrap();
        assert_eq!(raven.role, "reconnaissance and evidence gathering");
        assert!(reg.resolve("nonexistent").is_err());
    }

    // ── StubToolRouter ───────────────────────────────────────────

    #[tokio::test]
    async fn stub_tool_router_call_and_list() {
        let router = StubToolRouter;
        let call = chimera_tools::ToolCall {
            tool: "bash".into(),
            args: serde_json::json!({"cmd": "ls"}),
            world: None,
            requested_by: Uuid::new_v4(),
        };
        let result = router.call(call).await.unwrap();
        assert!(result.ok);

        let tools = router.list().await.unwrap();
        assert!(tools.is_empty());

        assert!(router.healthcheck("bash").await.unwrap());
    }

    // ── StubShellManager ─────────────────────────────────────────

    #[tokio::test]
    async fn stub_shell_manager_lifecycle() {
        let mgr = StubShellManager;
        let spec = chimera_shell::ShellSpec {
            cwd: PathBuf::from("/tmp"),
            env: Default::default(),
            read_only: false,
            label: Some("test".into()),
        };
        let id = mgr.spawn_shell(spec).await.unwrap();

        let output = mgr
            .exec(id, chimera_shell::ShellCommand {
                command: "echo hello".into(),
                timeout_secs: None,
            })
            .await
            .unwrap();
        assert_eq!(output.exit_code, 0);

        let transcript = mgr.transcript(id).await.unwrap();
        assert!(transcript.is_empty());

        mgr.close(id).await.unwrap();

        let shells = mgr.list_shells().await.unwrap();
        assert!(shells.is_empty());
    }

    #[tokio::test]
    async fn stub_shell_manager_worktree() {
        let mgr = StubShellManager;
        let spec = chimera_shell::WorktreeSpec {
            repo_root: PathBuf::from("/tmp/repo"),
            branch: Some("main".into()),
            read_only: true,
            label: None,
        };
        let handle = mgr.create_worktree(spec).await.unwrap();
        assert!(handle.read_only);

        mgr.destroy_worktree(handle.path).await.unwrap();
    }

    // ── StubWorldManager ─────────────────────────────────────────

    #[tokio::test]
    async fn stub_world_manager_lifecycle() {
        let mgr = StubWorldManager;
        let spec = chimera_sandbox::WorldSpec {
            kind: chimera_sandbox::WorldKind::Container,
            image: Some("alpine".into()),
            root: None,
            env: Default::default(),
            network: false,
            writable: true,
            label: Some("test".into()),
            limits: Default::default(),
        };
        let id = mgr.allocate(spec).await.unwrap();

        let state = mgr.inspect(id).await.unwrap();
        assert_eq!(state.status, chimera_sandbox::WorldStatus::Running);

        mgr.pause(id).await.unwrap();
        mgr.resume(id).await.unwrap();
        mgr.destroy(id).await.unwrap();

        let worlds = mgr.list().await.unwrap();
        assert!(worlds.is_empty());
    }

    // ── StubBrowserController ────────────────────────────────────

    #[tokio::test]
    async fn stub_browser_controller_lifecycle() {
        let ctrl = StubBrowserController;
        let id = ctrl
            .open(chimera_browser::BrowserTarget::Url("http://test.com".into()))
            .await
            .unwrap();

        let artifact = ctrl
            .act(id, chimera_browser::BrowserAction::Screenshot)
            .await
            .unwrap();
        assert!(matches!(artifact, chimera_browser::BrowserArtifact::Ack));

        let evidence = ctrl.screenshot(id).await.unwrap();
        assert_eq!(evidence.kind, "screenshot");

        let state = ctrl.state(id).await.unwrap();
        assert!(state.active);
        assert_eq!(state.current_url, "about:blank");

        ctrl.close(id).await.unwrap();

        let sessions = ctrl.list_sessions().await.unwrap();
        assert!(sessions.is_empty());
    }

    // ── StubMobileController ─────────────────────────────────────

    #[tokio::test]
    async fn stub_mobile_controller_lifecycle() {
        let ctrl = StubMobileController;
        let spec = chimera_mobile::DeviceSpec {
            profile: "pixel8".into(),
            os_version: None,
            network: false,
            label: None,
        };
        let id = ctrl.boot(spec).await.unwrap();

        let artifact = ctrl
            .perform(id, chimera_mobile::DeviceAction::Screenshot)
            .await
            .unwrap();
        assert!(matches!(artifact, chimera_mobile::DeviceArtifact::Ack));

        let state = ctrl.state(id).await.unwrap();
        assert_eq!(state.status, chimera_mobile::DeviceStatus::Running);

        ctrl.shutdown(id).await.unwrap();

        let devices = ctrl.list_devices().await.unwrap();
        assert!(devices.is_empty());
    }

    // ── StubCommsBridge ──────────────────────────────────────────

    #[tokio::test]
    async fn stub_comms_bridge_lifecycle() {
        let bridge = StubCommsBridge;
        let msg = chimera_comms::Notification {
            channel: chimera_comms::Channel::Terminal,
            priority: chimera_comms::Priority::Normal,
            subject: "test".into(),
            body: "body".into(),
            metadata: None,
        };
        let _mid = bridge.notify(msg).await.unwrap();

        let prompt = chimera_comms::CommsApprovalPrompt {
            channel: chimera_comms::Channel::Terminal,
            actor: "test".into(),
            class: "green".into(),
            action: "read".into(),
            reason: "testing".into(),
            touched_resources: vec![],
        };
        let aid = bridge.request_approval(prompt).await.unwrap();
        let result = bridge.check_approval(aid).await.unwrap();
        assert_eq!(result, Some(true));

        let receipt = bridge.receipt(Uuid::new_v4()).await.unwrap();
        assert!(receipt.is_none());
    }

    // ── StubMemoryStore ──────────────────────────────────────────

    #[tokio::test]
    async fn stub_memory_store_operations() {
        let store = StubMemoryStore;
        let item = chimera_memory::MemoryItem {
            id: Uuid::new_v4(),
            layer: chimera_memory::MemoryLayer::Episodic,
            key: "test".into(),
            content: "content".into(),
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            last_accessed: Utc::now(),
            session_id: None,
            creature_id: None,
            relevance: 0.9,
        };
        let id = store.remember(item).await.unwrap();
        assert_ne!(id, Uuid::nil());

        let results = store.retrieve(chimera_memory::MemoryQuery::default()).await.unwrap();
        assert!(results.is_empty());

        store.forget(Uuid::new_v4()).await.unwrap();

        let removed = store.compact(chimera_memory::MemoryLayer::Scratch).await.unwrap();
        assert_eq!(removed, 0);
    }

    // ── StubEvaluator ────────────────────────────────────────────

    #[tokio::test]
    async fn stub_evaluator_operations() {
        let eval = StubEvaluator;
        let req = chimera_eval::EvalRequest {
            name: "test".into(),
            target: chimera_eval::EvalTarget::Artifact { path: "x".into() },
            graders: vec![],
        };
        let report = eval.grade(req).await.unwrap();
        assert!(report.passed);
        assert_eq!(report.score, 1.0);

        let diff = eval.compare(Uuid::new_v4(), Uuid::new_v4()).await.unwrap();
        assert!(!diff.regression_detected);
    }

    // ── StubMcpClient ────────────────────────────────────────────

    #[tokio::test]
    async fn stub_mcp_client_operations() {
        let client = StubMcpClient;
        let tools = client.list_tools("test").await.unwrap();
        assert!(tools.is_empty());

        let result = client.invoke("test", "tool", serde_json::json!({})).await.unwrap();
        assert!(!result.is_error);

        let resources = client.list_resources("test").await.unwrap();
        assert!(resources.is_empty());

        let content = client.read_resource("test", "uri").await.unwrap();
        assert!(matches!(content, chimera_mcp::McpContent::Text { .. }));

        assert!(client.ping("test").await.unwrap());
    }

    // ── Domain type coverage ─────────────────────────────────────

    #[test]
    fn evidence_level_variants() {
        let levels = [EvidenceLevel::Minimal, EvidenceLevel::Standard, EvidenceLevel::Full];
        for level in &levels {
            let json = serde_json::to_string(level).unwrap();
            let _: EvidenceLevel = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn execution_mode_variants() {
        let modes = [ExecutionMode::Run, ExecutionMode::Plan, ExecutionMode::Swarm, ExecutionMode::Pack];
        for mode in &modes {
            let json = serde_json::to_string(mode).unwrap();
            let _: ExecutionMode = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn run_status_variants() {
        let statuses = [
            RunStatus::Running, RunStatus::Completed, RunStatus::Failed,
            RunStatus::Cancelled, RunStatus::AwaitingApproval, RunStatus::RolledBack,
        ];
        for s in &statuses {
            let json = serde_json::to_string(s).unwrap();
            let _: RunStatus = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn approval_class_variants() {
        let classes = [ApprovalClass::Green, ApprovalClass::Yellow, ApprovalClass::Orange, ApprovalClass::Red];
        for c in &classes {
            let json = serde_json::to_string(c).unwrap();
            let _: ApprovalClass = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn evidence_kind_variants() {
        let kinds = [
            EvidenceKind::CommandTranscript, EvidenceKind::FileDiff, EvidenceKind::TraceSpan,
            EvidenceKind::Screenshot, EvidenceKind::TestResult, EvidenceKind::BrowserReplay,
            EvidenceKind::ApprovalDecision,
        ];
        for k in &kinds {
            let json = serde_json::to_string(k).unwrap();
            let _: EvidenceKind = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn evidence_ref_serialization() {
        let er = EvidenceRef {
            kind: EvidenceKind::FileDiff,
            uri: "file:///test.diff".into(),
            label: "test diff".into(),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&er).unwrap();
        let _: EvidenceRef = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn approval_record_serialization() {
        let ar = ApprovalRecord {
            creature_id: Uuid::new_v4(),
            class: ApprovalClass::Yellow,
            action: "patch files".into(),
            approved: true,
            decided_at: Utc::now(),
        };
        let json = serde_json::to_string(&ar).unwrap();
        let _: ApprovalRecord = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn task_report_serialization() {
        let tr = TaskReport {
            description: "analyze code".into(),
            creature_id: Uuid::new_v4(),
            success: true,
            evidence: vec![],
        };
        let json = serde_json::to_string(&tr).unwrap();
        let _: TaskReport = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn objective_request_serialization() {
        let req = ObjectiveRequest {
            objective: "fix auth".into(),
            mode: ExecutionMode::Run,
            repo_root: Some(PathBuf::from("/tmp")),
            policy_profile: Some("strict".into()),
            evidence_level: EvidenceLevel::Full,
        };
        let json = serde_json::to_string(&req).unwrap();
        let _: ObjectiveRequest = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn run_report_serialization() {
        let report = RunReport {
            session_id: Uuid::new_v4(),
            status: RunStatus::Completed,
            tasks: vec![],
            approvals: vec![],
            evidence: vec![],
            checkpoints: vec![],
        };
        let json = serde_json::to_string(&report).unwrap();
        let _: RunReport = serde_json::from_str(&json).unwrap();
    }

    // ── Creature wrapping fallback ───────────────────────────────

    /// A single-creature registry to force the wrapping fallback
    /// in Harness::run_objective when tasks outnumber creatures.
    struct SingleCreatureRegistry;

    impl CreatureRegistry for SingleCreatureRegistry {
        fn builtin(&self) -> Vec<CreatureSpec> {
            vec![CreatureSpec {
                name: "solo".into(),
                role: "does everything".into(),
                temperament: Temperament::Balanced,
                toolbelt: vec![],
                preferred_worlds: vec![WorldKind::LocalShell],
                autonomy: AutonomyLevel::Medium,
                stop_conditions: vec![],
                escalation_rules: vec![],
                token_budget: None,
                deadline_secs: None,
            }]
        }

        fn resolve(&self, name: &str) -> anyhow::Result<CreatureSpec> {
            self.builtin()
                .into_iter()
                .find(|c| c.name == name)
                .ok_or_else(|| anyhow::anyhow!("unknown: {}", name))
        }
    }

    #[tokio::test]
    async fn harness_wraps_creatures_when_tasks_exceed_count() {
        // With only 1 creature but 4 tasks, the harness must wrap
        // around to available[0] for tasks 1..3 (the else branch).
        let harness = Harness {
            sessions: Arc::new(StubSessionStore),
            creatures: Arc::new(SingleCreatureRegistry),
            tools: Arc::new(StubToolRouter),
            trace: Arc::new(StubTraceSink),
            shells: Arc::new(StubShellManager),
            worlds: Arc::new(StubWorldManager),
            browser: Arc::new(StubBrowserController),
            mobile: Arc::new(StubMobileController),
            comms: Arc::new(StubCommsBridge),
            memory: Arc::new(StubMemoryStore),
            eval: Arc::new(StubEvaluator),
            mcp: Arc::new(StubMcpClient),
        };

        let req = ObjectiveRequest {
            objective: "test wrapping".into(),
            mode: ExecutionMode::Run,
            repo_root: None,
            policy_profile: None,
            evidence_level: EvidenceLevel::Standard,
        };

        let report = harness.run_objective(req).await.unwrap();
        assert_eq!(report.status, RunStatus::Completed);
        // All 4 tasks should complete even with only 1 creature
        assert_eq!(report.tasks.len(), 4);
        assert!(report.tasks.iter().all(|t| t.success));
    }
}
