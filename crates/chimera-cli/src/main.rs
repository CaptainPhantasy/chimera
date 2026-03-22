use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

use chimera_core::{
    BuiltinCreatureRegistry, EvidenceLevel, ExecutionMode, Harness, ObjectiveRequest,
    StubBrowserController, StubCommsBridge, StubEvaluator, StubMcpClient, StubMemoryStore,
    StubMobileController, StubSessionStore, StubShellManager, StubToolRouter, StubTraceSink,
    StubWorldManager,
};

// ===========================================================================
// Top-level CLI
// ===========================================================================

/// Chimera: One shell. Many creatures. Endless work.
#[derive(Debug, Parser)]
#[command(name = "chimera", version, about, long_about = None)]
pub struct Cli {
    /// Output format.
    #[arg(long, global = true, default_value = "human")]
    pub format: OutputFormat,

    /// Policy profile to use.
    #[arg(long, global = true)]
    pub profile: Option<String>,

    /// Enable verbose tracing output.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

/// Output format for CLI responses.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    Human,
    Json,
}

// ===========================================================================
// Top-level command enum
// ===========================================================================

#[derive(Debug, Subcommand)]
pub enum Command {
    // ----- Bootstrap & lifecycle (5.2.1) -----
    /// Initialize a new Chimera workspace.
    Init,
    /// Start the local control plane.
    Up,
    /// Gracefully stop the control plane.
    Down,
    /// Run diagnostics on dependencies and connectivity.
    Doctor,
    /// Show current harness status.
    Status,
    /// Manage the background daemon.
    Daemon {
        #[command(subcommand)]
        cmd: DaemonCmd,
    },

    // ----- Objective execution (5.2.2) -----
    /// Execute a single coordinated objective.
    Run {
        /// The objective description.
        objective: String,
    },
    /// Plan an objective without write side effects.
    Plan {
        /// The objective description.
        objective: String,
    },
    /// Execute an objective with parallel creatures and worlds.
    Swarm {
        /// The objective description.
        objective: String,
    },
    /// Launch a named creature pack.
    Pack {
        /// Pack name (e.g. "ship-hotfix", "incident-response").
        name: String,
    },

    // ----- Session control (5.2.3) -----
    /// Manage sessions.
    Session {
        #[command(subcommand)]
        cmd: SessionCmd,
    },
    /// Manage checkpoints.
    Checkpoint {
        #[command(subcommand)]
        cmd: CheckpointCmd,
    },
    /// Rollback to a checkpoint.
    Rollback {
        /// Checkpoint ID to rollback to.
        checkpoint_id: String,
    },
    /// Replay a session (read-only).
    Replay {
        /// Session ID to replay.
        session_id: String,
    },

    // ----- Creature control (5.2.4) -----
    /// Manage creatures.
    Creature {
        #[command(subcommand)]
        cmd: CreatureCmd,
    },

    // ----- Tool control (5.2.5) -----
    /// Manage tools.
    Tool {
        #[command(subcommand)]
        cmd: ToolCmd,
    },

    // ----- World control (5.2.6) -----
    /// Manage execution worlds.
    World {
        #[command(subcommand)]
        cmd: WorldCmd,
    },
    /// Open a browser session.
    Browser {
        #[command(subcommand)]
        cmd: BrowserCmd,
    },
    /// Control mobile emulators.
    Phone {
        #[command(subcommand)]
        cmd: PhoneCmd,
    },

    // ----- Policy & approvals (5.2.7) -----
    /// Approve a pending action.
    Approve {
        #[command(subcommand)]
        cmd: ApproveCmd,
    },
    /// Deny a pending action.
    Deny {
        /// The call ID to deny.
        call_id: String,
    },
    /// Manage policies.
    Policy {
        #[command(subcommand)]
        cmd: PolicyCmd,
    },
    /// Set the global autonomy level.
    Autonomy {
        #[command(subcommand)]
        cmd: AutonomyCmd,
    },
    /// Leash creatures to restrict permissions.
    Leash {
        /// Target: creature name/id or "all".
        target: String,
    },

    // ----- Trace & eval (5.2.8) -----
    /// Inspect traces.
    Trace {
        #[command(subcommand)]
        cmd: TraceCmd,
    },
    /// Run evaluations and comparisons.
    Eval {
        #[command(subcommand)]
        cmd: EvalCmd,
    },

    // ----- Memory & skills (5.2.9) -----
    /// Inspect memory store.
    Memory {
        #[command(subcommand)]
        cmd: MemoryCmd,
    },
    /// Manage skills.
    Skill {
        #[command(subcommand)]
        cmd: SkillCmd,
    },

    // ----- Durable jobs -----
    /// Schedule recurring jobs.
    Schedule {
        /// Frequency (e.g. "daily", "hourly").
        frequency: String,
        /// The objective description.
        objective: String,
    },
    /// Watch and react to events.
    Watch {
        /// The watch objective description.
        objective: String,
    },
}

// ===========================================================================
// Subcommand enums
// ===========================================================================

// ----- Daemon -----

#[derive(Debug, Subcommand)]
pub enum DaemonCmd {
    /// Start the background daemon.
    Start,
    /// Stop the background daemon.
    Stop,
}

// ----- Session -----

#[derive(Debug, Subcommand)]
pub enum SessionCmd {
    /// List all sessions.
    List,
    /// Resume a paused session.
    Resume {
        /// Session ID.
        session_id: String,
    },
    /// Fork a session into a new lineage.
    Fork {
        /// Session ID to fork.
        session_id: String,
    },
}

// ----- Checkpoint -----

#[derive(Debug, Subcommand)]
pub enum CheckpointCmd {
    /// Create a new checkpoint.
    Create,
    /// List all checkpoints.
    List,
}

// ----- Creature -----

#[derive(Debug, Subcommand)]
pub enum CreatureCmd {
    /// List all active creatures.
    List,
    /// Spawn a new creature.
    Spawn {
        /// Creature type name (e.g. "raven", "mantis").
        name: String,
        /// Number of instances to spawn.
        #[arg(long, default_value = "1")]
        count: u32,
    },
    /// Inspect a creature instance.
    Inspect {
        /// Creature instance ID (e.g. "mantis-1").
        instance: String,
    },
    /// Stop a creature instance.
    Stop {
        /// Creature instance ID.
        instance: String,
    },
    /// Promote a creature's autonomy level.
    Promote {
        /// Creature instance ID.
        instance: String,
    },
    /// Leash a creature to require per-step approval.
    Leash {
        /// Creature instance ID.
        instance: String,
    },
}

// ----- Tool -----

#[derive(Debug, Subcommand)]
pub enum ToolCmd {
    /// List all available tools.
    List,
    /// Inspect a specific tool.
    Inspect {
        /// Tool name.
        name: String,
    },
    /// Run a health check on a tool.
    Healthcheck {
        /// Tool name.
        name: String,
    },
    /// Test a tool with a dry run.
    Test {
        /// Tool name.
        name: String,
    },
    /// Disable a tool.
    Disable {
        /// Tool name.
        name: String,
    },
}

// ----- World -----

#[derive(Debug, Subcommand)]
pub enum WorldCmd {
    /// List all active worlds.
    List,
    /// Spawn a new world.
    Spawn {
        /// World type (e.g. "worktree", "vm", "container").
        kind: String,
    },
}

// ----- Browser -----

#[derive(Debug, Subcommand)]
pub enum BrowserCmd {
    /// Open a browser session targeting a URL or environment.
    Open {
        /// Target (URL or environment name).
        target: String,
    },
}

// ----- Phone -----

#[derive(Debug, Subcommand)]
pub enum PhoneCmd {
    /// Boot a mobile emulator.
    Boot {
        /// Device spec (e.g. "pixel8").
        device: String,
    },
}

// ----- Approve -----

#[derive(Debug, Subcommand)]
pub enum ApproveCmd {
    /// Approve the next pending action.
    Next,
    /// Approve the current plan.
    Plan,
}

// ----- Policy -----

#[derive(Debug, Subcommand)]
pub enum PolicyCmd {
    /// Show current policy.
    Show,
    /// Edit the policy configuration.
    Edit,
}

// ----- Autonomy -----

#[derive(Debug, Subcommand)]
pub enum AutonomyCmd {
    /// Set the global autonomy level.
    Set {
        /// Level: none, low, medium, high, full.
        level: String,
    },
}

// ----- Trace -----

#[derive(Debug, Subcommand)]
pub enum TraceCmd {
    /// Stream live trace events.
    Live,
    /// Show traces for a specific task.
    Show {
        /// Task ID.
        task_id: String,
    },
    /// Export traces for a session.
    Export {
        /// Session ID.
        session_id: String,
    },
}

// ----- Eval -----

#[derive(Debug, Subcommand)]
pub enum EvalCmd {
    /// Run an evaluation.
    Run {
        /// Evaluation target name.
        name: String,
    },
    /// Compare two checkpoints.
    Compare {
        /// First checkpoint ID.
        checkpoint_a: String,
        /// Second checkpoint ID.
        checkpoint_b: String,
    },
}

// ----- Memory -----

#[derive(Debug, Subcommand)]
pub enum MemoryCmd {
    /// List stored memory items.
    List,
    /// Show a specific memory episode.
    Show {
        /// Episode ID.
        episode_id: String,
    },
}

// ----- Skill -----

#[derive(Debug, Subcommand)]
pub enum SkillCmd {
    /// List available skills.
    List,
    /// Inspect a skill.
    Inspect {
        /// Skill name.
        name: String,
    },
    /// Run a skill.
    Run {
        /// Skill name.
        name: String,
    },
}

// ===========================================================================
// Harness construction
// ===========================================================================

fn build_harness() -> Harness {
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

/// Run an objective through the harness and print the report.
async fn run_objective(objective: String, mode: ExecutionMode) -> Result<()> {
    let harness = build_harness();
    let req = ObjectiveRequest {
        objective,
        mode,
        repo_root: std::env::current_dir().ok(),
        policy_profile: None,
        evidence_level: EvidenceLevel::Standard,
    };

    let report = harness.run_objective(req).await?;

    println!("\n=== Run Report ===");
    println!("Session:    {}", report.session_id);
    println!("Status:     {:?}", report.status);
    println!("Tasks:      {}", report.tasks.len());
    for (i, task) in report.tasks.iter().enumerate() {
        let icon = if task.success { "+" } else { "x" };
        println!("  [{}] {}: {}", icon, i + 1, task.description);
    }
    println!("Approvals:  {}", report.approvals.len());
    println!("Checkpoints:{}", report.checkpoints.len());

    Ok(())
}

// ===========================================================================
// Stub handler for non-harness commands
// ===========================================================================

fn handle_stub(label: &str) {
    println!("[stub] {label}");
}

// ===========================================================================
// Async command dispatch
// ===========================================================================

async fn dispatch(cmd: Command) -> Result<()> {
    match cmd {
        // Bootstrap & lifecycle
        Command::Init => handle_stub("chimera init"),
        Command::Up => handle_stub("chimera up"),
        Command::Down => handle_stub("chimera down"),
        Command::Doctor => handle_stub("chimera doctor"),
        Command::Status => handle_stub("chimera status"),
        Command::Daemon { cmd } => match cmd {
            DaemonCmd::Start => handle_stub("daemon start"),
            DaemonCmd::Stop => handle_stub("daemon stop"),
        },

        // Objective execution — wired to Harness
        Command::Run { objective } => return run_objective(objective, ExecutionMode::Run).await,
        Command::Plan { objective } => return run_objective(objective, ExecutionMode::Plan).await,
        Command::Swarm { objective } => return run_objective(objective, ExecutionMode::Swarm).await,
        Command::Pack { name } => handle_stub(&format!("pack: {name}")),

        // Session control
        Command::Session { cmd } => match cmd {
            SessionCmd::List => handle_stub("session list"),
            SessionCmd::Resume { session_id } => handle_stub(&format!("session resume {session_id}")),
            SessionCmd::Fork { session_id } => handle_stub(&format!("session fork {session_id}")),
        },
        Command::Checkpoint { cmd } => match cmd {
            CheckpointCmd::Create => handle_stub("checkpoint create"),
            CheckpointCmd::List => handle_stub("checkpoint list"),
        },
        Command::Rollback { checkpoint_id } => handle_stub(&format!("rollback {checkpoint_id}")),
        Command::Replay { session_id } => handle_stub(&format!("replay {session_id}")),

        // Creature control
        Command::Creature { cmd } => match cmd {
            CreatureCmd::List => handle_stub("creature list"),
            CreatureCmd::Spawn { name, count } => handle_stub(&format!("creature spawn {name} x{count}")),
            CreatureCmd::Inspect { instance } => handle_stub(&format!("creature inspect {instance}")),
            CreatureCmd::Stop { instance } => handle_stub(&format!("creature stop {instance}")),
            CreatureCmd::Promote { instance } => handle_stub(&format!("creature promote {instance}")),
            CreatureCmd::Leash { instance } => handle_stub(&format!("creature leash {instance}")),
        },

        // Tool control
        Command::Tool { cmd } => match cmd {
            ToolCmd::List => handle_stub("tool list"),
            ToolCmd::Inspect { name } => handle_stub(&format!("tool inspect {name}")),
            ToolCmd::Healthcheck { name } => handle_stub(&format!("tool healthcheck {name}")),
            ToolCmd::Test { name } => handle_stub(&format!("tool test {name}")),
            ToolCmd::Disable { name } => handle_stub(&format!("tool disable {name}")),
        },

        // World control
        Command::World { cmd } => match cmd {
            WorldCmd::List => handle_stub("world list"),
            WorldCmd::Spawn { kind } => handle_stub(&format!("world spawn {kind}")),
        },
        Command::Browser { cmd } => match cmd {
            BrowserCmd::Open { target } => handle_stub(&format!("browser open {target}")),
        },
        Command::Phone { cmd } => match cmd {
            PhoneCmd::Boot { device } => handle_stub(&format!("phone boot {device}")),
        },

        // Policy & approvals
        Command::Approve { cmd } => match cmd {
            ApproveCmd::Next => handle_stub("approve next"),
            ApproveCmd::Plan => handle_stub("approve plan"),
        },
        Command::Deny { call_id } => handle_stub(&format!("deny {call_id}")),
        Command::Policy { cmd } => match cmd {
            PolicyCmd::Show => handle_stub("policy show"),
            PolicyCmd::Edit => handle_stub("policy edit"),
        },
        Command::Autonomy { cmd } => match cmd {
            AutonomyCmd::Set { level } => handle_stub(&format!("autonomy set {level}")),
        },
        Command::Leash { target } => handle_stub(&format!("leash {target}")),

        // Trace & eval
        Command::Trace { cmd } => match cmd {
            TraceCmd::Live => handle_stub("trace live"),
            TraceCmd::Show { task_id } => handle_stub(&format!("trace show {task_id}")),
            TraceCmd::Export { session_id } => handle_stub(&format!("trace export {session_id}")),
        },
        Command::Eval { cmd } => match cmd {
            EvalCmd::Run { name } => handle_stub(&format!("eval run {name}")),
            EvalCmd::Compare { checkpoint_a, checkpoint_b } => {
                handle_stub(&format!("eval compare {checkpoint_a} {checkpoint_b}"))
            }
        },

        // Memory & skills
        Command::Memory { cmd } => match cmd {
            MemoryCmd::List => handle_stub("memory list"),
            MemoryCmd::Show { episode_id } => handle_stub(&format!("memory show {episode_id}")),
        },
        Command::Skill { cmd } => match cmd {
            SkillCmd::List => handle_stub("skill list"),
            SkillCmd::Inspect { name } => handle_stub(&format!("skill inspect {name}")),
            SkillCmd::Run { name } => handle_stub(&format!("skill run {name}")),
        },

        // Durable jobs
        Command::Schedule { frequency, objective } => {
            handle_stub(&format!("schedule {frequency}: {objective}"))
        }
        Command::Watch { objective } => handle_stub(&format!("watch: {objective}")),
    }

    Ok(())
}

// ===========================================================================
// Main
// ===========================================================================

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing subscriber
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    if cli.verbose {
        tracing::info!("verbose mode enabled");
    }

    dispatch(cli.command).await
}
