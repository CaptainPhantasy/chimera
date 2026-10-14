mod commands;
mod device_commands;
mod work_commands;
mod notify_commands;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use chimera_config::Config;
use chimera_core::{agent::{AgentConfig, DoneReason}, runtime::{SavedSession, RunEvent}, ExecutionMode};
use chimera_llm::LlmProvider;
use chimera_tools::ToolRouter;
use chimera_session::SessionStore;
use serde_json::{json, Value};
use std::io::{IsTerminal, Write};

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
    /// Check settings, credentials, saved sessions, and tool availability.
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
        /// Explicit external tool-server settings (ordinary terminal mode).
        #[arg(long)]
        mcp_config: Option<std::path::PathBuf>,
        /// Run tools inside this saved execution environment.
        #[arg(long)]
        world: Option<String>,
    },
    Plan {
        /// The objective description.
        objective: String,
    },
    /// Run specialist sessions with the project's saved permissions.
    Swarm {
        /// The objective description.
        objective: String,
    },
    /// Launch a named creature pack.
    Pack {
        /// Pack name (e.g. "ship-hotfix", "incident-response").
        name: String,
        /// Work this team should perform.
        #[arg(long)]
        objective: Option<String>,
        /// Explicit JSON pack definition, including its limits.
        #[arg(long)]
        definition: Option<std::path::PathBuf>,
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
    /// Connect to an explicitly selected external tool-server configuration.
    Mcp {
        #[arg(long)]
        config: std::path::PathBuf,
        #[command(subcommand)]
        cmd: McpCmd,
    },
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
    /// Inspect, pause, resume or cancel saved jobs.
    Jobs { #[command(subcommand)] cmd: JobsCmd },
    /// Deliver an explicitly reviewed message to a saved destination.
    Notify { #[command(subcommand)] cmd: NotifyCmd },
    /// Watch project files and run after they change.
    Watch {
        /// The watch objective description.
        objective: String,
        #[arg(long,default_value="**")]
        pattern: String,
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
    /// Inspect the running service.
    Status,
    /// Run the service in this terminal until stopped.
    Serve,
}

#[derive(Debug, Subcommand)]
pub enum JobsCmd {
    List, Inspect {id:String}, Pause {id:String}, Resume {id:String}, Cancel {id:String},
}

#[derive(Debug, Subcommand)]
pub enum NotifyCmd {
    /// Save a terminal inbox or an HTTPS webhook selected by the owner.
    Destination {name:String, #[arg(long)] url:Option<String>, #[arg(long)] terminal:bool},
    Destinations,
    Disable {name:String},
    /// Review and send a message once.
    Send {destination:String,subject:String,body:String},
    /// Send a pending action for review; delivery never approves it.
    Approval {id:String,#[arg(long)] destination:String},
    List,
    Inspect {id:String},
    Inbox,
    Acknowledge {id:String},
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
    /// Recover the previous saved record; preserve the damaged record.
    Recover { session_id: String },
    /// Export private-data-free diagnostics as JSON.
    Export { session_id: String },
    /// Fork a session into a new lineage.
    Fork {
        /// Session ID to fork.
        session_id: String,
    },
}

// ----- Checkpoint -----

#[derive(Debug, Subcommand)]
pub enum CheckpointCmd {
    /// Save conversation state and explicitly selected project files.
    Create {
        /// Session identifier; defaults to the most recently created session.
        #[arg(long)]
        session: Option<String>,
        /// Relative project files to capture, including a currently absent path.
        #[arg(long = "file")]
        files: Vec<std::path::PathBuf>,
    },
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
        /// Request for these specialist instances.
        #[arg(long)]
        objective: Option<String>,
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
    /// Re-enable a tool without granting action approval.
    Enable { name: String },
    /// Disable a tool.
    Disable {
        /// Tool name.
        name: String,
    },
}

// ----- World -----

#[derive(Debug, Subcommand)]
pub enum WorldCmd {
    /// Inspect an existing environment.
    Inspect { id: String },
    /// Stop accepting commands and stop running work.
    Close { id: String },
    /// Pause an environment and cancel its owned work.
    Pause { id: String },
    /// Resume a paused environment.
    Resume { id: String },
    /// Run one explicitly approved command inside an environment.
    Exec { id: String, command: String, #[arg(long, default_value="120")] timeout: u64 },
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
        #[arg(long)]
        actions: Option<std::path::PathBuf>,
        #[arg(long = "allow-origin")]
        allowed_origins: Vec<String>,
        #[arg(long,default_value=".chimera/artifacts")]
        output_dir: std::path::PathBuf,
    },
}

// ----- Phone -----

#[derive(Debug, Subcommand)]
pub enum PhoneCmd {
    /// Boot a mobile emulator.
    Boot {
        /// Device spec (e.g. "pixel8").
        device: String,
        #[arg(long)]
        app: std::path::PathBuf,
        #[arg(long)]
        package: String,
        #[arg(long)]
        network: bool,
        #[arg(long,default_value=".chimera/artifacts")]
        output_dir: std::path::PathBuf,
    },
}

// ----- Approve -----

#[derive(Debug, Subcommand)]
pub enum ApproveCmd {
    /// Review the oldest pending action in an interactive terminal.
    Next,
    /// Show pending actions and their exact arguments.
    List,
    /// Approve this exact pending identifier once.
    Action { id: String },
    /// Approve the current plan.
    Plan,
}

// ----- Policy -----

#[derive(Debug, Subcommand)]
pub enum PolicyCmd {
    /// Show current policy.
    Show,
    /// Show the private policy file and supported changes.
    Edit,
    /// Persist supervised or read-only permissions for this project.
    Set { profile: String },
}

// ----- Autonomy -----

#[derive(Debug, Subcommand)]
pub enum AutonomyCmd {
    /// Set the global autonomy level.
    Set {
        /// none: stop tools; low: review every action; medium: review file changes and commands.
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
    /// Save an evaluation containing an explicit command and target file.
    Define { name: String, #[arg(long)] file: String, #[arg(long)] command: String },
    /// List saved evaluations.
    List,
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
    /// Save a fact for this project.
    Remember { key: String, content: String },
    /// Archive a saved fact.
    Forget { episode_id: String },
    /// Combine exact duplicates without discarding distinct facts.
    Compact,
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
    /// Capture the objective of a completed saved conversation.
    Capture { name: String, session_id: String, #[arg(long, default_value="Saved workflow")] description: String },
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

#[derive(Debug, Subcommand)]
pub enum McpCmd {
    Tools { server: String },
    Call { server: String, tool: String, #[arg(long, default_value="{}")] args: String },
    Resources { server: String },
    Read { server: String, uri: String },
    Healthcheck { server: String },
}

fn workspace_root() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

fn output(format: OutputFormat, value: Value, human: impl AsRef<str>) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    match format {
        OutputFormat::Json => writeln!(stdout, "{}", serde_json::to_string(&value)?)?,
        OutputFormat::Human => writeln!(stdout, "{}", human.as_ref())?,
    }
    stdout.flush()?;
    Ok(())
}

fn policy(profile: Option<&str>, plan: bool) -> Result<chimera_tools::ToolPolicy> {
    anyhow::ensure!(matches!(profile, None | Some("supervised") | Some("read-only")),
        "unknown policy. Choose supervised or read-only with --profile");
    let saved=chimera_core::approval::PolicyStore::new(workspace_root()).get()?;
    Ok(if plan || profile == Some("read-only") || saved.read_only { chimera_tools::ToolPolicy::read_only() }
       else { chimera_tools::ToolPolicy::supervised() })
}

#[derive(Debug)]
enum RunEventFlow {
    Continue,
    // Keep the reply alive until the caller has polled cancellation through
    // the session/team. Closing it sooner can look like denial to a worker.
    Cancelled(Option<tokio::sync::oneshot::Sender<bool>>),
}

fn retain_cancelled_prompt(flow: RunEventFlow, held: &mut Vec<tokio::sync::oneshot::Sender<bool>>) {
    if let RunEventFlow::Cancelled(Some(answer)) = flow { held.push(answer); }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptDecision { Allow, Deny, Cancel }

fn finish_approval(
    store: &chimera_core::approval::ApprovalStore,
    id: uuid::Uuid,
    answer: tokio::sync::oneshot::Sender<bool>,
    decision: PromptDecision,
    cancellation: Option<&tokio::sync::watch::Sender<bool>>,
) -> Result<RunEventFlow> {
    if decision == PromptDecision::Cancel {
        // Set cancellation before dropping the answer channel. A concurrently
        // running specialist must observe cancellation, not an ordinary denial.
        if let Some(cancellation) = cancellation { cancellation.send_replace(true); }
        if cancellation.is_some() {
            return Ok(RunEventFlow::Cancelled(Some(answer)));
        }
        store.cancel(id)?;
        drop(answer);
        return Ok(RunEventFlow::Cancelled(None));
    }
    let _ = answer.send(decision == PromptDecision::Allow);
    Ok(RunEventFlow::Continue)
}

/// Tokio's standard-input adapter owns a blocking worker that cannot be
/// cancelled. A separate nonblocking terminal descriptor leaves no worker
/// behind when an approval is interrupted or decided from another process.
#[cfg(unix)]
async fn read_prompt_line() -> std::io::Result<String> {
    use std::os::unix::fs::OpenOptionsExt;
    #[cfg(any(target_os="macos",target_os="ios",target_os="freebsd",target_os="openbsd",target_os="netbsd",target_os="dragonfly"))]
    const NONBLOCK:i32=0x4;
    #[cfg(not(any(target_os="macos",target_os="ios",target_os="freebsd",target_os="openbsd",target_os="netbsd",target_os="dragonfly")))]
    const NONBLOCK:i32=0x800;
    let terminal=std::fs::OpenOptions::new().read(true).custom_flags(NONBLOCK).open("/dev/tty")?;
    read_nonblocking_prompt_line(terminal).await
}

#[cfg(unix)]
async fn read_nonblocking_prompt_line(mut terminal: impl std::io::Read) -> std::io::Result<String> {
    let mut line=Vec::new();
    loop {
        // macOS rejects registering /dev/tty with kqueue (and AsyncFd).
        // A nonblocking read plus a short timer is cancellable on every turn
        // and preserves the terminal's canonical input, echo and EOF handling.
        let mut byte=[0u8;1];
        match terminal.read(&mut byte) {
            Ok(0) => break,
            Ok(_) if byte[0]==b'\n' => break,
            Ok(_) => {
                if line.len()>=4096 {return Err(std::io::Error::new(std::io::ErrorKind::InvalidData,"approval reply exceeds 4096 bytes"));}
                line.push(byte[0]);
            },
            Err(error) if error.kind()==std::io::ErrorKind::WouldBlock => {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            },
            Err(error) if error.kind()==std::io::ErrorKind::Interrupted => tokio::task::yield_now().await,
            Err(error)=>return Err(error),
        }
    }
    String::from_utf8(line).map_err(|error|std::io::Error::new(std::io::ErrorKind::InvalidData,error))
}

#[cfg(not(unix))]
async fn read_prompt_line() -> std::io::Result<String> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported,"interactive approval input is unavailable on this platform; use an exact saved action decision from another process"))
}

async fn show_run_event(event: RunEvent, format: OutputFormat, cancellation: Option<&tokio::sync::watch::Sender<bool>>) -> Result<RunEventFlow> {
    match event {
        RunEvent::Update { kind, value } => match format {
            OutputFormat::Json => output(format, json!({"event":kind,"value":value}), "")?,
            OutputFormat::Human => {
                if kind == "text" { print!("{}", value.as_str().unwrap_or("")); std::io::stdout().flush()?; }
                else if kind == "tool_call" { eprintln!("\nTool: {}", value["name"].as_str().unwrap_or("unknown")); }
                else if kind == "tool_result" && value["ok"] == false { eprintln!("Tool failed: {}", value["output"]); }
            }
        },
        RunEvent::Approval { id, call, answer } => {
            // Register before displaying the prompt, including direct commands
            // which have no surrounding saved-session signal listener.
            #[cfg(unix)]
            let mut interrupts=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
            let approvals=chimera_core::approval::ApprovalStore::new(workspace_root());
            if cancellation.is_some_and(|sender| *sender.borrow()) {
                return finish_approval(&approvals,id,answer,PromptDecision::Cancel,cancellation);
            }
            let shown=chimera_core::approval::display_value(serde_json::to_value(&call)?);
            let decision = if matches!(format, OutputFormat::Human) && std::io::stdin().is_terminal() {
                eprintln!("\nAction {id}: {} {}\nAllow this exact action once? Type yes or no:", call.tool, shown["args"]);
                tokio::select! {
                    biased;
                    _ = async {
                        #[cfg(unix)] {let _=interrupts.recv().await;}
                        #[cfg(not(unix))] {let _=tokio::signal::ctrl_c().await;}
                    } => PromptDecision::Cancel,
                    result = read_prompt_line() => if result?.trim() == "yes" {PromptDecision::Allow} else {PromptDecision::Deny},
                    decision = async {
                        loop {
                            if let Ok(row)=chimera_core::approval::ApprovalStore::new(workspace_root()).get(id) {
                                use chimera_core::approval::Decision;
                                match row.decision {Decision::Pending=>(),Decision::Approved=>break PromptDecision::Allow,Decision::Cancelled=>break PromptDecision::Cancel,_=>break PromptDecision::Deny}
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                    } => decision,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(300)) => PromptDecision::Deny,
                }
            } else if std::env::var("CHIMERA_WAIT_APPROVAL").as_deref()==Ok("1") {
                output(format,json!({"event":"approval_required","id":id,"call":shown}),format!("Waiting for exact approval {id}; use chimera approve action {id} in this project."))?;
                return Ok(RunEventFlow::Continue);
            } else {
                output(format, json!({"event":"approval_required","id":id,"call":shown,"approved":false}),
                    "Action denied: use an interactive terminal to review and approve it.")?;
                PromptDecision::Deny
            };
            return finish_approval(&approvals,id,answer,decision,cancellation);
        }
    }
    Ok(RunEventFlow::Continue)
}

async fn run_objective(objective: String, mode: ExecutionMode, format: OutputFormat, profile: Option<&str>) -> Result<()> {
    run_objective_with_tools(objective,mode,format,profile,None,None).await
}

async fn run_objective_with_tools(objective: String, mode: ExecutionMode, format: OutputFormat, profile: Option<&str>,mcp_config:Option<std::path::PathBuf>,world:Option<String>) -> Result<()> {
    // Install before publishing a session or starting work. Creating the
    // ctrl_c future inside select leaves a gap before its first poll.
    #[cfg(unix)]
    let mut interrupts=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let cfg = Config::load().context("could not load CHIMERA settings")?;
    cfg.validate_credentials()?;
    let provider = cfg.build_provider();
    let tools:Arc<dyn ToolRouter> = Arc::new(chimera_tools::DefaultToolRouter::with_policy(Some(workspace_root()), policy(profile, matches!(mode, ExecutionMode::Plan))?));
    anyhow::ensure!(world.is_none() || mcp_config.is_none(),"external tool programs cannot attach to an isolated world; run them separately with explicit startup review");
    let watched_world=world.as_deref().map(uuid::Uuid::parse_str).transpose()?;
    let tools:Arc<dyn ToolRouter>=if let Some(id)=world {
        Arc::new(chimera_core::world_tools::WorldTools::with_read_only(Arc::new(chimera_sandbox::LocalWorldManager::new(workspace_root())?),uuid::Uuid::parse_str(&id)?,policy(profile,matches!(mode,ExecutionMode::Plan))?.read_only).await?)
    } else {tools};
    let external=if let Some(path)=mcp_config {
        anyhow::ensure!(!policy(profile,matches!(mode,ExecutionMode::Plan))?.read_only,"read-only execution cannot launch external programs");
        Some(commands::external_tools(tools.clone(),path,format).await?)
    } else {None};
    let tools:Arc<dyn ToolRouter>=external.as_ref().map(|p|p.clone() as Arc<dyn ToolRouter>).unwrap_or(tools);
    let config = AgentConfig { max_iterations: cfg.agent.max_iterations,
        temperature: Some(cfg.provider.temperature), max_tokens: Some(cfg.provider.max_tokens), ..Default::default() };
    let mut session = SavedSession::create(workspace_root(), &objective).await?;
    output(format, json!({"event":"session","id":session.id,"model":cfg.provider.model,"provider":provider.name()}),
        format!("Session {} · {} · {}",session.id,provider.name(),cfg.provider.model))?;
    let (events, mut receive) = chimera_core::runtime::event_channel();
    let (cancel, cancellation) = tokio::sync::watch::channel(false);
    // This task also runs while the foreground is waiting for a human reply.
    // Dropping the guard stops it on every exit, including display errors.
    struct StopWatcher(Option<tokio::task::JoinHandle<()>>);
    impl Drop for StopWatcher { fn drop(&mut self) {if let Some(task)=self.0.take(){task.abort();}} }
    let _watcher=StopWatcher(if let Some(id)=watched_world {
        let manager=chimera_sandbox::LocalWorldManager::new(workspace_root())?;
        let cancel=cancel.clone();
        Some(tokio::spawn(async move {
            use chimera_sandbox::{WorldManager,WorldStatus};
            loop {
                let available=match manager.inspect(id).await {
                    Ok(state)=>state.status==WorldStatus::Running && state.spec.limits.timeout_secs.is_none_or(|limit| (chrono::Utc::now()-state.created_at).num_seconds()<limit as i64),
                    Err(_)=>false,
                };
                if !available {cancel.send_replace(true);break;}
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }))
    } else {None});
    let mut prompt_cancel=cancel.subscribe();
    let mut cancelled_prompts=Vec::new();
    let result = {
        let run = session.run(&provider, tools, &cfg.provider.model, &objective, &config, chimera_core::runtime::RunChannels { events, cancellation });
        tokio::pin!(run);
        let result = loop {
            tokio::select! {
                result = &mut run => break result,
                Some(event) = receive.recv() => {
                    tokio::select! {
                        result=show_run_event(event, format, Some(&cancel))=>retain_cancelled_prompt(result?,&mut cancelled_prompts),
                        _=async {if !*prompt_cancel.borrow(){let _=prompt_cancel.changed().await;}}=>(),
                    }
                },
                _ = async {
                    #[cfg(unix)]
                    {let _=interrupts.recv().await;}
                    #[cfg(not(unix))]
                    {let _=tokio::signal::ctrl_c().await;}
                } => { cancel.send_replace(true); }
            }
        };
        while let Ok(event) = receive.try_recv() { retain_cancelled_prompt(show_run_event(event, format, Some(&cancel)).await?,&mut cancelled_prompts); }
        result
    };
    if let Some(external)=external {external.shutdown().await?;}
    let result=result?;
    let success = matches!(result.done_reason, DoneReason::FinalAnswer);
    let message = match result.done_reason {
        DoneReason::FinalAnswer => format!("\nAnswer saved to session {}.", session.id),
        DoneReason::MaxIterations => "Work limit reached. The request is unfinished; progress was saved.".into(),
        DoneReason::Error => result.error.clone().unwrap_or_else(|| "Run failed; progress was saved.".into()),
    };
    output(format, json!({"event":"result","session":session.id,"success":success,"result":result}), &message)?;
    anyhow::ensure!(success, "{message}");
    Ok(())
}

async fn select_session(store: &chimera_session::FileSessionStore, selected: Option<String>) -> Result<uuid::Uuid> {
    if let Some(id) = selected { return uuid::Uuid::parse_str(&id).context("invalid session identifier"); }
    let mut latest = None;
    for id in store.list().await? {
        let state=store.load(id).await?;
        if latest.as_ref().is_none_or(|(date,_)|state.meta.created_at > *date) { latest=Some((state.meta.created_at,id)); }
    }
    latest.map(|(_,id)|id).context("no saved session; run a request first or specify --session")
}

async fn dispatch_with_options(cmd: Command, format: OutputFormat, profile: Option<String>) -> Result<()> {
    let effective = policy(profile.as_deref(), false)?;
    let store = chimera_session::FileSessionStore::new(workspace_root());
    let router = chimera_tools::DefaultToolRouter::with_policy(Some(workspace_root()), effective.clone());
    match cmd {
        Command::Init => {
            let path = chimera_config::init_config()?;
            output(format, json!({"config":path}), format!("Settings: {}. Set your model address and name before running a task.",path.display()))
        }
        Command::Doctor => {
            let config = Config::load()?;
            config.validate_credentials()?;
            let sessions = store.list().await?;
            for id in &sessions { store.load(*id).await?; }
            let tools = router.list().await?;
            output(format, json!({"settings":"valid","model":config.provider.model,"provider":config.provider.name,"sessions":sessions.len(),"tools":tools,"connection_tested":false}),
                format!("Settings valid. Model: {}. {} saved conversations readable; {} tools registered. Model connection has not been tested; use a small run to check it.",config.provider.model,sessions.len(),tools.len()))
        }
        Command::Status => {
            let sessions = store.list().await?;
            output(format, json!({"version":env!("CARGO_PKG_VERSION"),"workspace":workspace_root(),"sessions":sessions,"policy":if effective.read_only {"read-only"} else {"supervised"}}),
                format!("CHIMERA {}\nWorkspace: {}\nSaved conversations: {}\nPolicy: {}",env!("CARGO_PKG_VERSION"),workspace_root().display(),sessions.len(),if effective.read_only {"read-only"} else {"supervised"}))
        }
        Command::Run { objective, interactive, mcp_config, world } => {
            if interactive {
                anyhow::ensure!(mcp_config.is_none() && world.is_none(),"external tools and saved environments currently use ordinary terminal run; omit --interactive");
                anyhow::ensure!(matches!(format, OutputFormat::Human), "the interactive screen requires --format human");
                chimera_tui::runner::run_saved_interactive(Some(objective), None, profile).await
            } else { run_objective_with_tools(objective, ExecutionMode::Run, format, profile.as_deref(),mcp_config,world).await }
        }
        Command::Plan { objective } => run_objective(objective, ExecutionMode::Plan, format, profile.as_deref()).await,
        Command::Tui { objective } => {
            anyhow::ensure!(matches!(format, OutputFormat::Human), "the interactive screen requires --format human");
            chimera_tui::runner::run_saved_interactive(objective, None, profile).await
        }
        Command::Session { cmd: SessionCmd::List } => {
            let mut rows = Vec::new();
            for id in store.list().await? {
                let state = store.load(id).await?;
                rows.push(json!({"id":id,"label":state.meta.label,"objective":state.meta.objective,"active":state.active}));
            }
            output(format, json!({"sessions":rows}), rows.iter().map(|row|format!("{}  {}",row["id"].as_str().unwrap_or(""),row["objective"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n"))
        }
        Command::Session { cmd: SessionCmd::Fork { session_id } } => {
            let parent = uuid::Uuid::parse_str(&session_id).context("invalid session identifier")?;
            let child = store.fork(parent).await?;
            output(format, json!({"parent":parent,"session":child}), format!("Forked {parent} into {child}"))
        }
        Command::Session { cmd: SessionCmd::Resume { session_id } } => {
            let id = uuid::Uuid::parse_str(&session_id).context("invalid session identifier")?;
            anyhow::ensure!(matches!(format, OutputFormat::Human), "resume opens the terminal screen; use --format human");
            chimera_tui::runner::run_saved_interactive(None, Some(id), profile).await
        }
        Command::Session { cmd: SessionCmd::Export { session_id } } | Command::Trace { cmd: TraceCmd::Export { session_id } } => {
            let id = uuid::Uuid::parse_str(&session_id).context("invalid session identifier")?;
            // Export is always machine-readable, with secrets removed.
            println!("{}", serde_json::to_string(&store.export_redacted(id).await?)?);
            Ok(())
        }
        Command::Replay { session_id } | Command::Trace { cmd: TraceCmd::Show {task_id:session_id} } => {
            let state=store.load(uuid::Uuid::parse_str(&session_id)?).await?;
            let events=state.effective_events()?;
            let messages=events.iter().rev().find_map(|event|match event {chimera_session::SessionEvent::Custom {key,payload} if key=="messages"=>Some(payload.clone()),_=>None}).unwrap_or(json!([]));
            let human=messages.as_array().context("saved messages are damaged")?.iter().map(|m|format!("{}: {}",m["role"].as_str().unwrap_or("event"),m["content"].as_str().unwrap_or("[tool call]"))).collect::<Vec<_>>().join("\n\n");
            output(format,json!({"session":state.id,"messages":messages,"executes_tools":false}),human)
        }
        Command::Session { cmd: SessionCmd::Recover { session_id } } => {
            let id = uuid::Uuid::parse_str(&session_id).context("invalid session identifier")?;
            let state = store.recover_previous(id).await?;
            output(format,json!({"session":state.id,"recovered":true}),format!("Recovered previous record for {}. The damaged record was preserved.",state.id))
        }
        Command::Checkpoint { cmd: CheckpointCmd::Create { session, files } } => {
            let id = select_session(&store, session).await?;
            let checkpoint = store.checkpoint_with_files(id,&files).await?;
            output(format,json!({"session":id,"checkpoint":checkpoint,"files":files}),format!("Checkpoint {checkpoint} saved for session {id}; {} selected files captured.",files.len()))
        }
        Command::Checkpoint { cmd: CheckpointCmd::List } => {
            let mut rows = Vec::new();
            for id in store.list().await? {
                for checkpoint in store.load(id).await?.checkpoints {
                    rows.push(json!({"session":id,"checkpoint":checkpoint}));
                }
            }
            output(format,json!({"checkpoints":rows}),rows.iter().map(|row|format!("{}  session {}",row["checkpoint"],row["session"])).collect::<Vec<_>>().join("\n"))
        }
        Command::Rollback { checkpoint_id } => {
            anyhow::ensure!(!effective.read_only,"read-only policy does not allow restoring files");
            let checkpoint = uuid::Uuid::parse_str(&checkpoint_id).context("invalid checkpoint identifier")?;
            let mut owner = None;
            for id in store.list().await? { if store.load(id).await?.checkpoints.contains(&checkpoint) { owner = Some(id); break; } }
            let id = owner.context("checkpoint was not found in this project")?;
            let preview = store.preview_restore(id,checkpoint).await?;
            if !preview.restored_files.is_empty() {
                anyhow::ensure!(matches!(format,OutputFormat::Human) && std::io::stdin().is_terminal(),"restoring files requires an interactive terminal to review the current files");
                #[cfg(unix)]
                let mut interrupts=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
                eprintln!("Restore these files from checkpoint {checkpoint}: {}\nCurrent versions will be preserved. Type yes to restore, or no to leave them unchanged:",preview.restored_files.iter().map(|f|f.path.display().to_string()).collect::<Vec<_>>().join(", "));
                let line=tokio::select! {
                    biased;
                    _=async {
                        #[cfg(unix)] {let _=interrupts.recv().await;}
                        #[cfg(not(unix))] {let _=tokio::signal::ctrl_c().await;}
                    }=>anyhow::bail!("restore cancelled; nothing changed"),
                    result=read_prompt_line()=>result?,
                    _=tokio::time::sleep(std::time::Duration::from_secs(300))=>anyhow::bail!("restore approval expired; nothing changed"),
                };
                anyhow::ensure!(line.trim()=="yes","restore cancelled; nothing changed");
            }
            store.restore_checkpoint(id,checkpoint,&preview.current_files).await?;
            output(format,json!({"session":id,"restored_checkpoint":checkpoint}),format!("Restored checkpoint {checkpoint} for session {id}. Previous file versions and history were preserved."))
        }
        Command::Tool { cmd: ToolCmd::List } => {
            let tools = router.list().await?;
            output(format, json!({"tools":tools}), tools.iter().map(|t| format!("{}: {}",t.name,t.description)).collect::<Vec<_>>().join("\n"))
        }
        Command::Tool { cmd: ToolCmd::Inspect { name } } => {
            let tool = router.list().await?.into_iter().find(|tool|tool.name==name).with_context(||format!("unknown tool: {name}"))?;
            output(format, json!(tool), format!("{}: {}\nRead only: {}",tool.name,tool.description,tool.read_only))
        }
        Command::Tool { cmd: ToolCmd::Healthcheck { name } } => {
            anyhow::ensure!(router.healthcheck(&name).await?, "unknown or unavailable tool: {name}");
            output(format, json!({"tool":name,"registered":true}), format!("{name}: registered. Execution requires a permitted call."))
        }
        Command::Policy { cmd: PolicyCmd::Show } => {
            let saved=chimera_core::approval::PolicyStore::new(workspace_root()).get()?;
            output(format, json!({"saved":saved,"effective_read_only":effective.read_only,"writes_require_exact_approval":true,"network":false}),
                format!("Read only: {}. Action review: {}. Disabled tools: {}. Changes and shell commands always require exact approval.",effective.read_only,saved.autonomy,saved.disabled_tools.iter().cloned().collect::<Vec<_>>().join(", ")))
        }
        other => commands::execute(other, format, profile).await,
    }
}

#[cfg(test)]
async fn dispatch(cmd: Command) -> Result<()> { dispatch_with_options(cmd, OutputFormat::Human, None).await }

fn console_log_filter(command: &Command, verbose: bool) -> &'static str {
    // Fullscreen requests already show and save their errors through RunEvent.
    // A console log writes outside Ratatui's screen buffer and corrupts it.
    // Startup/final errors are still printed by main after terminal restoration.
    if matches!(command, Command::Tui { .. } | Command::Run { interactive:true, .. }
        | Command::Session { cmd:SessionCmd::Resume { .. } }) { "off" }
    else if verbose { "debug" } else { "warn" }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    tracing_subscriber::fmt().with_writer(std::io::stderr)
        .with_env_filter(console_log_filter(&cli.command,cli.verbose)).init();
    match dispatch_with_options(cli.command, cli.format, cli.profile).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            match cli.format {
                OutputFormat::Json => println!("{}",json!({"success":false,"error":format!("{error:#}")})),
                OutputFormat::Human => eprintln!("Error: {error:#}"),
            }
            std::process::ExitCode::FAILURE
        }
    }
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
    fn console_logs_do_not_overwrite_fullscreen_entrypoints() {
        use std::sync::{Arc,Mutex};
        #[derive(Clone)]
        struct Capture(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Capture {
            fn write(&mut self,bytes:&[u8])->std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);Ok(bytes.len())
            }
            fn flush(&mut self)->std::io::Result<()> {Ok(())}
        }
        for args in [vec!["tui"],vec!["run","--interactive","test"],vec!["session","resume","saved-id"],vec!["run","test"],vec!["plan","test"],vec!["session","list"]] {
            for verbose in [false,true] {
                let cli=parse(&args);
                let fullscreen=args==["tui"] || args.contains(&"--interactive") || args.contains(&"resume");
                let captured=Capture(Arc::default());let writer=captured.clone();
                let subscriber=tracing_subscriber::fmt().without_time().with_ansi(false)
                    .with_writer(move||writer.clone()).with_env_filter(console_log_filter(&cli.command,verbose)).finish();
                tracing::subscriber::with_default(subscriber,|| {
                    tracing::warn!(error="controlled provider error", "provider request failed");
                    tracing::debug!("controlled diagnostic detail");
                });
                let output=String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
                if fullscreen {assert!(output.is_empty(),"fullscreen {args:?} wrote {output:?}");}
                else {
                    assert!(output.contains("controlled provider error"),"ordinary command lost warning: {args:?}");
                    assert_eq!(output.contains("controlled diagnostic detail"),verbose);
                }
            }
        }
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
            Command::Run { objective, .. } => assert_eq!(objective, "fix auth"),
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
            Command::Pack { name, .. } => assert_eq!(name, "ship-hotfix"),
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
        assert!(matches!(parse(&["checkpoint", "create"]).command, Command::Checkpoint { cmd: CheckpointCmd::Create { .. } }));
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
            Command::Creature { cmd: CreatureCmd::Spawn { name, count, .. } } => {
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
            Command::Browser { cmd: BrowserCmd::Open { target, .. } } => assert_eq!(target, "staging"),
            _ => panic!("expected open"),
        }
        match parse(&["phone", "boot", "pixel8", "--app", "app.apk", "--package", "com.example.app"]).command {
            Command::Phone { cmd: PhoneCmd::Boot { device, .. } } => assert_eq!(device, "pixel8"),
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
            Command::Watch { objective, .. } => assert_eq!(objective, "monitor logs"),
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

    #[tokio::test]
    async fn unfinished_commands_and_missing_tools_fail() {
        for command in [Command::Pack{name:"missing".into(),objective:None,definition:None},Command::Tool{cmd:ToolCmd::Healthcheck{name:"does-not-exist".into()}}] {
            assert!(dispatch(command).await.is_err());
        }
    }

    #[test]
    fn invalid_policy_cannot_silently_select_another_policy() {
        assert!(policy(Some("strict"),false).is_err());
        assert!(policy(Some("supervised"),true).unwrap().read_only);
        assert!(policy(Some("read-only"),false).unwrap().read_only);
    }

    fn prompt_test_root()->std::path::PathBuf {
        let root=std::env::temp_dir().join(format!("chimera-prompt-order-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();root
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prompt_reader_accepts_yes_after_waiting_without_executing_an_action() {
        use std::{io::Write,os::unix::net::UnixStream};
        let (input,mut writer)=UnixStream::pair().unwrap();
        input.set_nonblocking(true).unwrap();
        let write=async {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            writer.write_all(b"yes\n").unwrap();
        };
        let read=read_nonblocking_prompt_line(input);
        let ((),line)=tokio::time::timeout(std::time::Duration::from_secs(1),async {tokio::join!(write,read)}).await.unwrap();
        assert_eq!(line.unwrap(),"yes");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prompt_reader_cancellation_closes_input_without_a_blocking_worker() {
        use std::{io::Read,os::unix::net::UnixStream};
        let (input,mut writer)=UnixStream::pair().unwrap();
        input.set_nonblocking(true).unwrap();writer.set_nonblocking(true).unwrap();
        assert!(tokio::time::timeout(std::time::Duration::from_millis(30),read_nonblocking_prompt_line(input)).await.is_err());
        assert_eq!(writer.read(&mut [0]).unwrap(),0,"cancelled input descriptor must close immediately");
    }

    struct PromptModel;
    #[async_trait::async_trait]
    impl LlmProvider for PromptModel {
        fn name(&self)->&str {"controlled-approval-order"}
        async fn complete(&self,request:&chimera_llm::CompletionRequest)->std::result::Result<chimera_llm::CompletionResponse,chimera_llm::LlmError> {
            assert_ne!(request.messages.last().unwrap().role,chimera_llm::Role::Tool,"denied or cancelled action must not request another model response");
            Ok(chimera_llm::CompletionResponse {content:None,tool_calls:vec![chimera_llm::ToolCall{id:"review-write".into(),kind:"function".into(),function:chimera_llm::FunctionCall{name:"write_file".into(),arguments:json!({"path":"effect.txt","content":"must not happen"}).to_string()}}],finish_reason:Some("tool_calls".into()),usage:None})
        }
    }

    #[tokio::test]
    async fn prompt_cancel_preserves_session_cancellation_while_no_remains_denial() {
        use chimera_core::approval::{ApprovalStore,Decision};
        for decision in [PromptDecision::Cancel,PromptDecision::Deny] {
            let root=prompt_test_root();let approvals=ApprovalStore::new(root.clone());
            let mut session=SavedSession::create(root.clone(),"request a reviewed write").await.unwrap();
            let tools=Arc::new(chimera_tools::DefaultToolRouter::new(Some(root.clone())));
            let (events,mut receive)=chimera_core::runtime::event_channel();let (cancel,cancelled)=tokio::sync::watch::channel(false);
            let config=AgentConfig::default();let mut held=Vec::new();let mut action=None;
            let outcome={
                let run=session.run(&PromptModel,tools,"test","request a reviewed write",&config,chimera_core::runtime::RunChannels { events, cancellation: cancelled });tokio::pin!(run);
                tokio::time::timeout(std::time::Duration::from_secs(5),async {
                    loop {tokio::select! {
                        result=&mut run=>break result,
                        Some(event)=receive.recv()=>if let RunEvent::Approval{id,answer,..}=event {
                            action=Some(id);retain_cancelled_prompt(finish_approval(&approvals,id,answer,decision,Some(&cancel)).unwrap(),&mut held);
                        },
                    }}
                }).await.unwrap()
            };
            let state=session.store.load(session.id).await.unwrap();assert!(!state.active);
            if decision==PromptDecision::Cancel {
                assert!(outcome.unwrap_err().to_string().contains("cancelled"));assert!(*cancel.borrow());
                assert_eq!(approvals.get(action.unwrap()).unwrap().decision,Decision::Cancelled);
                assert!(state.events.iter().any(|event|matches!(event,chimera_session::SessionEvent::Custom{key,payload} if key=="run_outcome" && payload["outcome"]=="cancelled")));
            } else {
                assert!(matches!(outcome.unwrap().done_reason,DoneReason::Error));assert!(!*cancel.borrow());
                assert_eq!(approvals.get(action.unwrap()).unwrap().decision,Decision::Denied);
                assert!(state.events.iter().any(|event|matches!(event,chimera_session::SessionEvent::Custom{key,payload} if key=="run_outcome" && payload["outcome"]=="failed")));
            }
            assert!(!root.join("effect.txt").exists());
        }
    }

    #[tokio::test]
    async fn prompt_cancel_reaches_concurrent_team_before_reply_channels_close() {
        use chimera_core::{approval::{ApprovalStore,Decision},team::{TeamRegistry,TeamRequest,TeamStatus}};
        let root=prompt_test_root();let registry=TeamRegistry::new(root.clone()).unwrap();
        let request=TeamRequest{objective:"request reviewed writes".into(),creatures:vec!["mantis".into(),"mantis".into()],scope:Default::default(),max_parallel:2,read_only:false};
        let record=registry.create(request).unwrap();let approvals=ApprovalStore::new(root.clone());
        let (events,mut receive)=tokio::sync::mpsc::channel(256);let (cancel,cancelled)=tokio::sync::watch::channel(false);
        let mut held=Vec::new();let mut count=0;
        let execute=registry.execute(record.id,Arc::new(PromptModel),"test".into(),AgentConfig::default(),events,cancelled);tokio::pin!(execute);
        let result=tokio::time::timeout(std::time::Duration::from_secs(5),async {
            loop {tokio::select! {
                result=&mut execute=>break result.unwrap(),
                Some(event)=receive.recv()=>if let RunEvent::Approval{id,answer,..}=event.event {
                    count+=1;retain_cancelled_prompt(finish_approval(&approvals,id,answer,PromptDecision::Cancel,Some(&cancel)).unwrap(),&mut held);
                },
            }}
        }).await.unwrap();
        assert!(count>=1);assert_eq!(result.status,TeamStatus::Cancelled);
        assert!(result.members.iter().all(|member|member.status==TeamStatus::Cancelled));
        assert!(approvals.list().unwrap().iter().all(|row|row.decision==Decision::Cancelled));
        assert!(!root.join("effect.txt").exists());
    }

    #[tokio::test]
    async fn direct_prompt_cancel_closes_pending_action_without_denial_or_execution() {
        use chimera_core::approval::{ApprovalStore,Decision};
        let root=prompt_test_root();let approvals=ApprovalStore::new(root.clone());let actor=uuid::Uuid::new_v4();
        let call=chimera_tools::ToolCall{tool:"write_file".into(),args:json!({"path":"effect.txt","content":"no"}),world:None,requested_by:actor};
        let row=approvals.request(actor,call.clone(),std::time::Duration::from_secs(300)).unwrap();
        let (answer,receive)=tokio::sync::oneshot::channel();
        assert!(matches!(finish_approval(&approvals,row.id,answer,PromptDecision::Cancel,None).unwrap(),RunEventFlow::Cancelled(None)));
        assert!(receive.await.is_err());assert_eq!(approvals.get(row.id).unwrap().decision,Decision::Cancelled);
        assert!(approvals.consume(row.id,actor,&call).is_err());assert!(!root.join("effect.txt").exists());
    }
}
