use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use chimera_config::Config;
use chimera_core::{
    agent::{self, AgentConfig},
    BuiltinCreatureRegistry, ExecutionMode, Harness,
    StubBrowserController, StubCommsBridge, StubEvaluator, StubMcpClient, StubMemoryStore,
    StubMobileController, StubSessionStore, StubShellManager, StubToolRouter, StubTraceSink,
    StubWorldManager,
};
use chimera_llm::LlmProvider;
use chimera_tools::ToolRouter;
use chimera_creatures::CreatureRegistry;
use chimera_session::SessionStore;
use chimera_trace::TraceSink;

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
        /// Launch in interactive TUI mode.
        #[arg(long, short = 'i')]
        interactive: bool,
    },
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
    /// Launch the interactive terminal UI (TUI).
    Tui {
        /// Optional initial objective.
        objective: Option<String>,
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

#[allow(dead_code)]
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

/// Build the tool router, preferring the concrete `DefaultToolRouter` when
/// available, falling back to the core stub otherwise.
fn build_tool_router(repo_root: Option<std::path::PathBuf>) -> Arc<dyn ToolRouter> {
    chimera_tools::try_build_default_tool_router(repo_root)
        .unwrap_or_else(|| Arc::new(chimera_core::StubToolRouter))
}

/// Run an objective through the REAL LLM-driven agent loop.
///
/// Loads config, builds an OpenAI-compatible provider, a default tool router,
/// and runs the agent loop with live streaming to stdout.
async fn run_objective(objective: String, mode: ExecutionMode) -> Result<()> {
    let cfg = Config::load().context("failed to load CHIMERA config")?;

    if cfg.provider.api_key.is_empty()
        && !cfg.provider.base_url.contains("localhost")
        && !cfg.provider.base_url.contains("127.0.0.1")
    {
        anyhow::bail!(
            "no API key configured. Set CHIMERA_API_KEY or OPENAI_API_KEY, or point \
             CHIMERA_BASE_URL at a local server (e.g. LM Studio on localhost)."
        );
    }

    let provider = cfg.build_provider();
    let repo_root = std::env::current_dir().ok();
    let tools = build_tool_router(repo_root);
    let tool_defs = agent::build_tool_definitions(tools.as_ref())
        .await
        .context("failed to enumerate tools")?;

    let agent_cfg = AgentConfig {
        max_iterations: cfg.agent.max_iterations,
        system_prompt: if matches!(mode, ExecutionMode::Plan) {
            format!(
                "{}\n\nYou are in PLAN mode. Do not make any changes. Read the codebase, \
                 investigate, and produce a concrete plan. Do not call write tools.",
                chimera_core::agent::DEFAULT_SYSTEM_PROMPT
            )
        } else {
            chimera_core::agent::DEFAULT_SYSTEM_PROMPT.to_string()
        },
        temperature: Some(cfg.provider.temperature),
        max_tokens: Some(cfg.provider.max_tokens),
    };

    println!("chimera ▸ {} ▸ {}\n", provider.name(), cfg.provider.model);
    println!("objective: {objective}\n");

    let creature_id = uuid::Uuid::new_v4();
    let mut saw_text = false;
    let mut cb = |kind: String, value: serde_json::Value| {
        if kind == "text" {
            if let Some(t) = value.as_str() {
                print!("{t}");
                use std::io::Write;
                let _ = std::io::stdout().flush();
                saw_text = true;
            }
        } else if kind == "tool_call" {
            if saw_text {
                println!();
                saw_text = false;
            }
            if let Some(name) = value.get("name").and_then(|v| v.as_str()) {
                eprintln!("  ↳ tool: {name}");
            }
        }
    };

    let result = agent::run_agent_loop(
        &provider,
        tools.as_ref(),
        &cfg.provider.model,
        &objective,
        creature_id,
        &agent_cfg,
        &tool_defs,
        &mut cb,
    )
    .await
    .context("agent loop failed")?;

    if saw_text {
        println!();
    }

    println!("\n─── run complete ───");
    println!(
        "iterations: {}  |  done: {:?}  |  tokens: {}+{}",
        result.iterations,
        result.done_reason,
        result.total_prompt_tokens,
        result.total_completion_tokens
    );

    Ok(())
}

// ===========================================================================
// Stub handler for non-harness commands
// ===========================================================================

fn handle_stub(label: &str) {
    println!("[stub] {label}");
}

// ===========================================================================
// Wired command helpers
// ===========================================================================

/// Resolve the workspace root (current directory).
fn workspace_root() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

/// `chimera session list` — list persisted sessions from FileSessionStore.
async fn list_sessions() -> Result<()> {
    let store = chimera_session::FileSessionStore::new(workspace_root());
    let ids = store.list().await?;
    if ids.is_empty() {
        println!("No sessions found.");
        return Ok(());
    }
    println!("Sessions ({}):", ids.len());
    for id in &ids {
        match store.load(*id).await {
            Ok(state) => {
                println!(
                    "  {}  {:<20}  objective: {}",
                    id, state.meta.label, state.meta.objective
                );
            }
            Err(e) => {
                println!("  {}  [unreadable: {e}]", id);
            }
        }
    }
    Ok(())
}

/// `chimera session fork <id>` — fork a session into a new lineage.
async fn fork_session(session_id: String) -> Result<()> {
    let id = uuid::Uuid::parse_str(&session_id)
        .with_context(|| format!("invalid session id: {session_id}"))?;
    let store = chimera_session::FileSessionStore::new(workspace_root());
    let child = store.fork(id).await?;
    println!("Forked {id} → {child}");
    Ok(())
}

/// `chimera tool list` — list registered tools from DefaultToolRouter.
async fn list_tools() -> Result<()> {
    let router = build_tool_router(Some(workspace_root()));
    let descs = router.list().await?;
    println!("Tools ({}):", descs.len());
    for d in &descs {
        let ro = if d.read_only { "ro" } else { "rw" };
        println!("  {:<14} [{:<3}] {}", d.name, ro, d.description);
    }
    Ok(())
}

/// `chimera tool inspect <name>` — show a single tool's descriptor.
async fn inspect_tool(name: String) -> Result<()> {
    let router = build_tool_router(Some(workspace_root()));
    let descs = router.list().await?;
    let d = descs
        .into_iter()
        .find(|d| d.name == name)
        .ok_or_else(|| anyhow::anyhow!("unknown tool: {name}"))?;
    println!("name:        {}", d.name);
    println!("description: {}", d.description);
    println!("read_only:   {}", d.read_only);
    println!("category:    {:?}", d.category);
    Ok(())
}

/// `chimera tool healthcheck <name>` — check a tool is registered.
async fn healthcheck_tool(name: String) -> Result<()> {
    let router = build_tool_router(Some(workspace_root()));
    let ok = router.healthcheck(&name).await?;
    println!("{}: {}", name, if ok { "healthy" } else { "missing" });
    Ok(())
}

/// `chimera creature list` — list builtin creature specs.
async fn list_creatures() -> Result<()> {
    let registry = chimera_core::BuiltinCreatureRegistry;
    let specs = registry.builtin();
    println!("Creatures ({}):", specs.len());
    for c in &specs {
        println!(
            "  {:<12} {:<10} autonomy={:?} temperament={:?}",
            c.name, c.role, c.autonomy, c.temperament
        );
    }
    Ok(())
}

/// `chimera trace export <session_id>` — dump a session's trace JSONL to stdout.
async fn export_trace(session_id: String) -> Result<()> {
    let id = uuid::Uuid::parse_str(&session_id)
        .with_context(|| format!("invalid session id: {session_id}"))?;
    let sink = chimera_trace::FileTraceSink::new(workspace_root());
    let events = sink.query(id).await?;
    if events.is_empty() {
        println!("No trace events for session {id}.");
        return Ok(());
    }
    println!("Trace for {id} ({} events):", events.len());
    for e in &events {
        println!(
            "[{}] {:<5} {:<24} {}",
            e.timestamp.format("%H:%M:%S%.3f"),
            format!("{:?}", e.level).to_lowercase(),
            e.span,
            e.message
        );
    }
    Ok(())
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
        Command::Run { objective, interactive } => {
            if interactive {
                return chimera_tui::runner::run_interactive(Some(objective)).await;
            }
            return run_objective(objective, ExecutionMode::Run).await;
        }
        Command::Plan { objective } => return run_objective(objective, ExecutionMode::Plan).await,
        Command::Swarm { objective } => return run_objective(objective, ExecutionMode::Swarm).await,
        Command::Pack { name } => handle_stub(&format!("pack: {name}")),
        Command::Tui { objective } => {
            return chimera_tui::runner::run_interactive(objective).await;
        }

        // Session control
        // Session control — wired to FileSessionStore
        Command::Session { cmd } => match cmd {
            SessionCmd::List => return list_sessions().await,
            SessionCmd::Resume { session_id } => {
                println!("[info] resume is implicit; sessions are stateless files. {session_id}");
            }
            SessionCmd::Fork { session_id } => return fork_session(session_id).await,
        },
        Command::Checkpoint { cmd } => match cmd {
            CheckpointCmd::Create => handle_stub("checkpoint create"),
            CheckpointCmd::List => handle_stub("checkpoint list"),
        },
        Command::Rollback { checkpoint_id } => handle_stub(&format!("rollback {checkpoint_id}")),
        Command::Replay { session_id } => handle_stub(&format!("replay {session_id}")),

        // Creature control — wired to BuiltinCreatureRegistry
        Command::Creature { cmd } => match cmd {
            CreatureCmd::List => return list_creatures().await,
            CreatureCmd::Spawn { name, count } => {
                println!("[info] creature spawn: {name} x{count} (runtime spawning not yet implemented)");
            }
            CreatureCmd::Inspect { instance } => {
                println!("[info] creature inspect: {instance} (runtime registry not yet implemented)");
            }
            CreatureCmd::Stop { instance } => println!("[info] creature stop: {instance}"),
            CreatureCmd::Promote { instance } => println!("[info] creature promote: {instance}"),
            CreatureCmd::Leash { instance } => println!("[info] creature leash: {instance}"),
        },

        // Tool control — wired to DefaultToolRouter
        Command::Tool { cmd } => match cmd {
            ToolCmd::List => return list_tools().await,
            ToolCmd::Inspect { name } => return inspect_tool(name).await,
            ToolCmd::Healthcheck { name } => return healthcheck_tool(name).await,
            ToolCmd::Test { name } => println!("[info] tool test: {name} (no dry-run backend)"),
            ToolCmd::Disable { name } => println!("[info] tool disable: {name} (not yet persisted)"),
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

        // Trace & eval — trace wired to FileTraceSink
        Command::Trace { cmd } => match cmd {
            TraceCmd::Live => println!("[info] trace live: use the TUI (chimera tui) for live traces"),
            TraceCmd::Show { task_id } => println!("[info] trace show {task_id}: per-task traces not yet indexed"),
            TraceCmd::Export { session_id } => return export_trace(session_id).await,
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    // ── Parsing tests ────────────────────────────────────────────

    fn parse(args: &[&str]) -> Cli {
        Cli::parse_from(std::iter::once("chimera").chain(args.iter().copied()))
    }

    #[test]
    fn parse_init() {
        let cli = parse(&["init"]);
        assert!(matches!(cli.command, Command::Init));
    }

    #[test]
    fn parse_up_down_doctor_status() {
        assert!(matches!(parse(&["up"]).command, Command::Up));
        assert!(matches!(parse(&["down"]).command, Command::Down));
        assert!(matches!(parse(&["doctor"]).command, Command::Doctor));
        assert!(matches!(parse(&["status"]).command, Command::Status));
    }

    #[test]
    fn parse_daemon() {
        let cli = parse(&["daemon", "start"]);
        assert!(matches!(cli.command, Command::Daemon { cmd: DaemonCmd::Start }));
        let cli = parse(&["daemon", "stop"]);
        assert!(matches!(cli.command, Command::Daemon { cmd: DaemonCmd::Stop }));
    }

    #[test]
    fn parse_run_plan_swarm() {
        let cli = parse(&["run", "fix auth"]);
        match cli.command {
            Command::Run { objective, interactive: _ } => assert_eq!(objective, "fix auth"),
            _ => panic!("expected run"),
        }
        let cli = parse(&["plan", "migrate db"]);
        match cli.command {
            Command::Plan { objective } => assert_eq!(objective, "migrate db"),
            _ => panic!("expected plan"),
        }
        let cli = parse(&["swarm", "audit tests"]);
        match cli.command {
            Command::Swarm { objective } => assert_eq!(objective, "audit tests"),
            _ => panic!("expected swarm"),
        }
    }

    #[test]
    fn parse_pack() {
        let cli = parse(&["pack", "ship-hotfix"]);
        match cli.command {
            Command::Pack { name } => assert_eq!(name, "ship-hotfix"),
            _ => panic!("expected pack"),
        }
    }

    #[test]
    fn parse_session_commands() {
        assert!(matches!(parse(&["session", "list"]).command, Command::Session { cmd: SessionCmd::List }));
        match parse(&["session", "resume", "abc"]).command {
            Command::Session { cmd: SessionCmd::Resume { session_id } } => assert_eq!(session_id, "abc"),
            _ => panic!("expected resume"),
        }
        match parse(&["session", "fork", "xyz"]).command {
            Command::Session { cmd: SessionCmd::Fork { session_id } } => assert_eq!(session_id, "xyz"),
            _ => panic!("expected fork"),
        }
    }

    #[test]
    fn parse_checkpoint_rollback_replay() {
        assert!(matches!(parse(&["checkpoint", "create"]).command, Command::Checkpoint { cmd: CheckpointCmd::Create }));
        assert!(matches!(parse(&["checkpoint", "list"]).command, Command::Checkpoint { cmd: CheckpointCmd::List }));
        match parse(&["rollback", "cp-1"]).command {
            Command::Rollback { checkpoint_id } => assert_eq!(checkpoint_id, "cp-1"),
            _ => panic!("expected rollback"),
        }
        match parse(&["replay", "s-1"]).command {
            Command::Replay { session_id } => assert_eq!(session_id, "s-1"),
            _ => panic!("expected replay"),
        }
    }

    #[test]
    fn parse_creature_commands() {
        assert!(matches!(parse(&["creature", "list"]).command, Command::Creature { cmd: CreatureCmd::List }));
        match parse(&["creature", "spawn", "raven", "--count", "3"]).command {
            Command::Creature { cmd: CreatureCmd::Spawn { name, count } } => {
                assert_eq!(name, "raven");
                assert_eq!(count, 3);
            }
            _ => panic!("expected spawn"),
        }
        match parse(&["creature", "inspect", "mantis-1"]).command {
            Command::Creature { cmd: CreatureCmd::Inspect { instance } } => assert_eq!(instance, "mantis-1"),
            _ => panic!("expected inspect"),
        }
        match parse(&["creature", "stop", "hound-2"]).command {
            Command::Creature { cmd: CreatureCmd::Stop { instance } } => assert_eq!(instance, "hound-2"),
            _ => panic!("expected stop"),
        }
        match parse(&["creature", "promote", "owl-1"]).command {
            Command::Creature { cmd: CreatureCmd::Promote { instance } } => assert_eq!(instance, "owl-1"),
            _ => panic!("expected promote"),
        }
        match parse(&["creature", "leash", "hydra-1"]).command {
            Command::Creature { cmd: CreatureCmd::Leash { instance } } => assert_eq!(instance, "hydra-1"),
            _ => panic!("expected leash"),
        }
    }

    #[test]
    fn parse_tool_commands() {
        assert!(matches!(parse(&["tool", "list"]).command, Command::Tool { cmd: ToolCmd::List }));
        match parse(&["tool", "inspect", "bash"]).command {
            Command::Tool { cmd: ToolCmd::Inspect { name } } => assert_eq!(name, "bash"),
            _ => panic!("expected inspect"),
        }
        match parse(&["tool", "healthcheck", "mcp"]).command {
            Command::Tool { cmd: ToolCmd::Healthcheck { name } } => assert_eq!(name, "mcp"),
            _ => panic!("expected healthcheck"),
        }
        match parse(&["tool", "test", "browser"]).command {
            Command::Tool { cmd: ToolCmd::Test { name } } => assert_eq!(name, "browser"),
            _ => panic!("expected test"),
        }
        match parse(&["tool", "disable", "comms"]).command {
            Command::Tool { cmd: ToolCmd::Disable { name } } => assert_eq!(name, "comms"),
            _ => panic!("expected disable"),
        }
    }

    #[test]
    fn parse_world_browser_phone() {
        assert!(matches!(parse(&["world", "list"]).command, Command::World { cmd: WorldCmd::List }));
        match parse(&["world", "spawn", "vm"]).command {
            Command::World { cmd: WorldCmd::Spawn { kind } } => assert_eq!(kind, "vm"),
            _ => panic!("expected spawn"),
        }
        match parse(&["browser", "open", "staging"]).command {
            Command::Browser { cmd: BrowserCmd::Open { target } } => assert_eq!(target, "staging"),
            _ => panic!("expected open"),
        }
        match parse(&["phone", "boot", "pixel8"]).command {
            Command::Phone { cmd: PhoneCmd::Boot { device } } => assert_eq!(device, "pixel8"),
            _ => panic!("expected boot"),
        }
    }

    #[test]
    fn parse_policy_approval_commands() {
        assert!(matches!(parse(&["approve", "next"]).command, Command::Approve { cmd: ApproveCmd::Next }));
        assert!(matches!(parse(&["approve", "plan"]).command, Command::Approve { cmd: ApproveCmd::Plan }));
        match parse(&["deny", "call-4"]).command {
            Command::Deny { call_id } => assert_eq!(call_id, "call-4"),
            _ => panic!("expected deny"),
        }
        assert!(matches!(parse(&["policy", "show"]).command, Command::Policy { cmd: PolicyCmd::Show }));
        assert!(matches!(parse(&["policy", "edit"]).command, Command::Policy { cmd: PolicyCmd::Edit }));
        match parse(&["autonomy", "set", "low"]).command {
            Command::Autonomy { cmd: AutonomyCmd::Set { level } } => assert_eq!(level, "low"),
            _ => panic!("expected set"),
        }
        match parse(&["leash", "all"]).command {
            Command::Leash { target } => assert_eq!(target, "all"),
            _ => panic!("expected leash"),
        }
    }

    #[test]
    fn parse_trace_eval_commands() {
        assert!(matches!(parse(&["trace", "live"]).command, Command::Trace { cmd: TraceCmd::Live }));
        match parse(&["trace", "show", "t-1"]).command {
            Command::Trace { cmd: TraceCmd::Show { task_id } } => assert_eq!(task_id, "t-1"),
            _ => panic!("expected show"),
        }
        match parse(&["trace", "export", "s-1"]).command {
            Command::Trace { cmd: TraceCmd::Export { session_id } } => assert_eq!(session_id, "s-1"),
            _ => panic!("expected export"),
        }
        match parse(&["eval", "run", "auth-refactor"]).command {
            Command::Eval { cmd: EvalCmd::Run { name } } => assert_eq!(name, "auth-refactor"),
            _ => panic!("expected run"),
        }
        match parse(&["eval", "compare", "a", "b"]).command {
            Command::Eval { cmd: EvalCmd::Compare { checkpoint_a, checkpoint_b } } => {
                assert_eq!(checkpoint_a, "a");
                assert_eq!(checkpoint_b, "b");
            }
            _ => panic!("expected compare"),
        }
    }

    #[test]
    fn parse_memory_skill_commands() {
        assert!(matches!(parse(&["memory", "list"]).command, Command::Memory { cmd: MemoryCmd::List }));
        match parse(&["memory", "show", "ep-44"]).command {
            Command::Memory { cmd: MemoryCmd::Show { episode_id } } => assert_eq!(episode_id, "ep-44"),
            _ => panic!("expected show"),
        }
        assert!(matches!(parse(&["skill", "list"]).command, Command::Skill { cmd: SkillCmd::List }));
        match parse(&["skill", "inspect", "hotfix"]).command {
            Command::Skill { cmd: SkillCmd::Inspect { name } } => assert_eq!(name, "hotfix"),
            _ => panic!("expected inspect"),
        }
        match parse(&["skill", "run", "dep-upgrade"]).command {
            Command::Skill { cmd: SkillCmd::Run { name } } => assert_eq!(name, "dep-upgrade"),
            _ => panic!("expected run"),
        }
    }

    #[test]
    fn parse_schedule_watch() {
        match parse(&["schedule", "daily", "check deps"]).command {
            Command::Schedule { frequency, objective } => {
                assert_eq!(frequency, "daily");
                assert_eq!(objective, "check deps");
            }
            _ => panic!("expected schedule"),
        }
        match parse(&["watch", "monitor logs"]).command {
            Command::Watch { objective } => assert_eq!(objective, "monitor logs"),
            _ => panic!("expected watch"),
        }
    }

    #[test]
    fn parse_global_flags() {
        let cli = parse(&["--verbose", "--format", "json", "--profile", "strict", "status"]);
        assert!(cli.verbose);
        assert!(matches!(cli.format, OutputFormat::Json));
        assert_eq!(cli.profile, Some("strict".into()));
    }

    // ── Dispatch tests ───────────────────────────────────────────

    #[tokio::test]
    async fn dispatch_stub_commands() {
        // Test a representative sample of stub-dispatched commands
        dispatch(Command::Init).await.unwrap();
        dispatch(Command::Up).await.unwrap();
        dispatch(Command::Down).await.unwrap();
        dispatch(Command::Doctor).await.unwrap();
        dispatch(Command::Status).await.unwrap();
        dispatch(Command::Daemon { cmd: DaemonCmd::Start }).await.unwrap();
        dispatch(Command::Daemon { cmd: DaemonCmd::Stop }).await.unwrap();
        dispatch(Command::Pack { name: "test".into() }).await.unwrap();
        dispatch(Command::Session { cmd: SessionCmd::List }).await.unwrap();
        dispatch(Command::Session { cmd: SessionCmd::Resume { session_id: "s1".into() } }).await.unwrap();
        // Fork of a non-existent session returns an error (expected); only assert it doesn't panic unexpectedly.
        let _ = dispatch(Command::Session { cmd: SessionCmd::Fork { session_id: "00000000-0000-0000-0000-000000000001".into() } }).await;
        dispatch(Command::Checkpoint { cmd: CheckpointCmd::Create }).await.unwrap();
        dispatch(Command::Checkpoint { cmd: CheckpointCmd::List }).await.unwrap();
        dispatch(Command::Rollback { checkpoint_id: "cp1".into() }).await.unwrap();
        dispatch(Command::Replay { session_id: "s1".into() }).await.unwrap();
    }

    #[tokio::test]
    async fn dispatch_creature_commands() {
        dispatch(Command::Creature { cmd: CreatureCmd::List }).await.unwrap();
        dispatch(Command::Creature { cmd: CreatureCmd::Spawn { name: "raven".into(), count: 2 } }).await.unwrap();
        dispatch(Command::Creature { cmd: CreatureCmd::Inspect { instance: "m-1".into() } }).await.unwrap();
        dispatch(Command::Creature { cmd: CreatureCmd::Stop { instance: "m-1".into() } }).await.unwrap();
        dispatch(Command::Creature { cmd: CreatureCmd::Promote { instance: "m-1".into() } }).await.unwrap();
        dispatch(Command::Creature { cmd: CreatureCmd::Leash { instance: "m-1".into() } }).await.unwrap();
    }

    #[tokio::test]
    async fn dispatch_tool_world_browser_phone() {
        dispatch(Command::Tool { cmd: ToolCmd::List }).await.unwrap();
        dispatch(Command::Tool { cmd: ToolCmd::Inspect { name: "bash".into() } }).await.unwrap();
        dispatch(Command::Tool { cmd: ToolCmd::Healthcheck { name: "mcp".into() } }).await.unwrap();
        dispatch(Command::Tool { cmd: ToolCmd::Test { name: "git".into() } }).await.unwrap();
        dispatch(Command::Tool { cmd: ToolCmd::Disable { name: "comms".into() } }).await.unwrap();
        dispatch(Command::World { cmd: WorldCmd::List }).await.unwrap();
        dispatch(Command::World { cmd: WorldCmd::Spawn { kind: "vm".into() } }).await.unwrap();
        dispatch(Command::Browser { cmd: BrowserCmd::Open { target: "staging".into() } }).await.unwrap();
        dispatch(Command::Phone { cmd: PhoneCmd::Boot { device: "pixel8".into() } }).await.unwrap();
    }

    #[tokio::test]
    async fn dispatch_policy_trace_eval_memory_skill() {
        dispatch(Command::Approve { cmd: ApproveCmd::Next }).await.unwrap();
        dispatch(Command::Approve { cmd: ApproveCmd::Plan }).await.unwrap();
        dispatch(Command::Deny { call_id: "4".into() }).await.unwrap();
        dispatch(Command::Policy { cmd: PolicyCmd::Show }).await.unwrap();
        dispatch(Command::Policy { cmd: PolicyCmd::Edit }).await.unwrap();
        dispatch(Command::Autonomy { cmd: AutonomyCmd::Set { level: "low".into() } }).await.unwrap();
        dispatch(Command::Leash { target: "all".into() }).await.unwrap();
        dispatch(Command::Trace { cmd: TraceCmd::Live }).await.unwrap();
        dispatch(Command::Trace { cmd: TraceCmd::Show { task_id: "t1".into() } }).await.unwrap();
        dispatch(Command::Trace { cmd: TraceCmd::Export { session_id: "00000000-0000-0000-0000-000000000002".into() } }).await.unwrap();
        dispatch(Command::Eval { cmd: EvalCmd::Run { name: "auth".into() } }).await.unwrap();
        dispatch(Command::Eval { cmd: EvalCmd::Compare { checkpoint_a: "a".into(), checkpoint_b: "b".into() } }).await.unwrap();
        dispatch(Command::Memory { cmd: MemoryCmd::List }).await.unwrap();
        dispatch(Command::Memory { cmd: MemoryCmd::Show { episode_id: "e1".into() } }).await.unwrap();
        dispatch(Command::Skill { cmd: SkillCmd::List }).await.unwrap();
        dispatch(Command::Skill { cmd: SkillCmd::Inspect { name: "hotfix".into() } }).await.unwrap();
        dispatch(Command::Skill { cmd: SkillCmd::Run { name: "hotfix".into() } }).await.unwrap();
        dispatch(Command::Schedule { frequency: "daily".into(), objective: "check".into() }).await.unwrap();
        dispatch(Command::Watch { objective: "monitor".into() }).await.unwrap();
    }

    #[tokio::test]
    async fn dispatch_run_objective_config_gating_and_local_path() {
        // Sequence the env-dependent cases in one test to avoid parallel
        // env races between sibling tests.
        // Case 1: no key + non-local URL => config error.
        unsafe {
            std::env::remove_var("CHIMERA_API_KEY");
            std::env::remove_var("OPENAI_API_KEY");
            std::env::set_var("CHIMERA_BASE_URL", "https://api.openai.com/v1");
        }
        let res = dispatch(Command::Run { objective: "test".into(), interactive: false }).await;
        assert!(res.is_err(), "expected config-gate error without key");
        let msg = format!("{}", res.unwrap_err());
        assert!(
            msg.contains("API key") || msg.contains("no API key"),
            "unexpected error: {msg}"
        );

        // Case 2: localhost URL skips gate; agent loop runs and captures the
        // (failed) connection into an Ok result.
        unsafe {
            std::env::set_var("CHIMERA_BASE_URL", "http://127.0.0.1:9999/v1");
            std::env::set_var("CHIMERA_MODEL", "test-model");
        }
        let res = dispatch(Command::Run { objective: "test".into(), interactive: false }).await;
        unsafe {
            std::env::remove_var("CHIMERA_BASE_URL");
            std::env::remove_var("CHIMERA_MODEL");
        }
        assert!(res.is_ok(), "expected Ok with captured error in result");
    }

    // ── build_harness ────────────────────────────────────────────

    #[test]
    fn build_harness_succeeds() {
        let _harness = build_harness();
    }
}
