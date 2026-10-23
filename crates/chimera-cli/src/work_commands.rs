use super::*;
use chimera_core::daemon::{self, CommandJobRunner, DaemonAction};
use chimera_core::scheduler::{
    FileWorkflowScheduler, ScheduleFrequency, ScheduleSpec, WorkflowScheduler,
};
use chimera_core::team::{TeamRegistry, TeamRequest, TeamStatus};

async fn run_team(
    mut request: TeamRequest,
    format: OutputFormat,
    profile: Option<&str>,
) -> Result<()> {
    // Catch an interrupt before publishing the team identifier or starting work.
    #[cfg(unix)]
    let mut interrupts = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let cfg = Config::load()?;
    cfg.validate_credentials()?;
    request.read_only |= policy(profile, false)?.read_only;
    let registry = TeamRegistry::new(workspace_root())?;
    let record = registry.create(request)?;
    output(
        format,
        json!({"team":record}),
        format!(
            "Team {}: {} specialist sessions.",
            record.id,
            record.members.len()
        ),
    )?;
    let (events, mut receive) = tokio::sync::mpsc::channel(256);
    let (cancel, cancelled) = tokio::sync::watch::channel(false);
    let config = AgentConfig {
        max_iterations: cfg.agent.max_iterations,
        temperature: Some(cfg.provider.temperature),
        max_tokens: Some(cfg.provider.max_tokens),
        ..Default::default()
    };
    let execute = registry.execute(
        record.id,
        Arc::new(cfg.build_provider()),
        cfg.provider.model,
        config,
        events,
        cancelled,
    );
    tokio::pin!(execute);
    // Events reach the terminal through one sequential worker, so prompts keep
    // a single input reader, while this loop keeps draining the team channel:
    // a specialist waiting on approval can no longer stall the others until
    // their event channel fills and the team stops them.
    let (forward, mut worker_receive) =
        tokio::sync::mpsc::unbounded_channel::<chimera_core::team::TeamEvent>();
    let worker_cancel = cancel.clone();
    let mut worker = tokio::spawn(async move {
        let mut held_prompts: Vec<tokio::sync::oneshot::Sender<bool>> = Vec::new();
        while let Some(event) = worker_receive.recv().await {
            if matches!(format, OutputFormat::Json) {
                output(
                    format,
                    json!({"event":"specialist","team":event.team_id,"member":event.member_id}),
                    "",
                )?;
            }
            retain_cancelled_prompt(
                show_run_event(event.event, format, Some(&worker_cancel)).await?,
                &mut held_prompts,
            );
        }
        Ok::<_, anyhow::Error>(held_prompts)
    });
    let result = loop {
        tokio::select! {
            result=&mut execute=>break result?,
            done=&mut worker=>{
                match done {
                    Ok(Err(error))=>return Err(error),
                    Ok(Ok(_))=>anyhow::bail!("event display stopped while the team was still running"),
                    Err(join)=>anyhow::bail!("event display task failed: {join}"),
                }
            },
            Some(event)=receive.recv()=>{
                if forward.send(event).is_err() {
                    match (&mut worker).await {
                        Ok(Err(error))=>return Err(error),
                        _=>anyhow::bail!("event display stopped; team work was cancelled"),
                    }
                }
            },
            _=async {
                #[cfg(unix)]
                {let _=interrupts.recv().await;}
                #[cfg(not(unix))]
                {let _=tokio::signal::ctrl_c().await;}
            }=>{cancel.send_replace(true);},
        }
    };
    // Preserve event order: anything still queued passes through the same
    // worker before the channel is closed and its output is collected.
    while let Ok(event) = receive.try_recv() {
        let _ = forward.send(event);
    }
    drop(forward);
    let cancelled_prompts = worker.await??;
    let _ = cancelled_prompts;
    output(
        format,
        json!({"team":result}),
        serde_json::to_string_pretty(&result)?,
    )?;
    anyhow::ensure!(
        result.status == TeamStatus::Completed,
        "team {} ended {:?}; inspect its saved member sessions",
        result.id,
        result.status
    );
    Ok(())
}

fn frequency(text: &str) -> Result<ScheduleFrequency> {
    Ok(match text {
        "hourly"=>ScheduleFrequency::Interval {every_secs:3600},
        "daily"=>ScheduleFrequency::Interval {every_secs:86400},
        value if value.starts_with("every:")=>ScheduleFrequency::Interval {every_secs:value[6..].parse().context("every: must be followed by seconds")?},
        value if value.starts_with("once:")=>ScheduleFrequency::Once {at:chrono::DateTime::parse_from_rfc3339(&value[5..])?.with_timezone(&chrono::Utc)},
        value if value.starts_with("cron:")=>ScheduleFrequency::Cron {expression:value[5..].into()},
        _=>anyhow::bail!("frequency must be hourly, daily, every:<seconds>, once:<date with time zone>, or cron:<five numeric UTC fields>"),
    })
}

async fn start_daemon(format: OutputFormat) -> Result<()> {
    let root = workspace_root();
    if let Ok(status) = daemon::request(&root, DaemonAction::Status).await {
        return output(
            format,
            json!(status),
            format!("Service is already running as process {}.", status.pid),
        );
    }
    let records = chimera_memory::storage::ProjectRecords::new(root.clone(), "daemon");
    let log = records.path("service-log")?.with_extension("log");
    if let Ok(meta) = std::fs::symlink_metadata(&log) {
        anyhow::ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "unsafe service log path"
        );
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&log)?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .args(["--format", "json", "daemon", "serve"])
        .current_dir(&root)
        .stdin(std::process::Stdio::null())
        .stdout(file.try_clone()?)
        .stderr(file);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .context("could not start background service")?;
    let until = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait()? {
            anyhow::bail!("service exited with {status}; read {}", log.display());
        }
        if let Ok(status) = daemon::request(&root, DaemonAction::Status).await {
            return output(
                format,
                json!({"service":status,"log":log}),
                format!(
                    "Service running as process {}. Log: {}",
                    status.pid,
                    log.display()
                ),
            );
        }
        if tokio::time::Instant::now() >= until {
            #[cfg(unix)]
            {
                unsafe extern "C" {
                    fn kill(pid: i32, signal: i32) -> i32;
                }
                unsafe {
                    kill(child.id() as i32, 2);
                }
            }
            for _ in 0..30 {
                if child.try_wait()?.is_some() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            if child.try_wait()?.is_none() {
                child.kill()?;
                child.wait()?;
            }
            anyhow::bail!(
                "service startup did not finish in ten seconds; stopped this attempt. Read {}",
                log.display()
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

pub(crate) async fn execute(
    command: Command,
    format: OutputFormat,
    profile: Option<String>,
) -> Result<()> {
    match command {
        Command::Up
        | Command::Daemon {
            cmd: DaemonCmd::Start,
        } => start_daemon(format).await,
        Command::Down
        | Command::Daemon {
            cmd: DaemonCmd::Stop,
        } => {
            let status = daemon::request(workspace_root(), DaemonAction::Stop).await?;
            output(
                format,
                json!(status),
                "Service stop requested; running work is being cancelled.",
            )
        }
        Command::Daemon {
            cmd: DaemonCmd::Status,
        } => {
            let status = daemon::request(workspace_root(), DaemonAction::Status).await?;
            output(
                format,
                json!(status),
                serde_json::to_string_pretty(&status)?,
            )
        }
        Command::Daemon {
            cmd: DaemonCmd::Serve,
        } => daemon::serve(workspace_root(), CommandJobRunner::current_executable()?).await,
        Command::Schedule {
            frequency: when,
            objective,
        } => {
            let id = FileWorkflowScheduler::new(workspace_root())
                .create(ScheduleSpec {
                    label: objective.chars().take(80).collect(),
                    objective,
                    frequency: frequency(&when)?,
                    pack: None,
                    enabled: true,
                })
                .await?;
            output(
                format,
                json!({"job":id}),
                format!("Saved job {id}. Start execution with chimera up."),
            )
        }
        Command::Watch { objective, pattern } => {
            let id = FileWorkflowScheduler::new(workspace_root())
                .create(ScheduleSpec {
                    label: objective.chars().take(80).collect(),
                    objective,
                    frequency: ScheduleFrequency::Watch { pattern },
                    pack: None,
                    enabled: true,
                })
                .await?;
            output(
                format,
                json!({"job":id}),
                format!("Saved watcher {id}. Start execution with chimera up."),
            )
        }
        Command::Jobs { cmd } => {
            let store = FileWorkflowScheduler::new(workspace_root());
            let value = match cmd {
                JobsCmd::List => json!({"jobs":store.all()?}),
                JobsCmd::Inspect { id } => json!(store.details(uuid::Uuid::parse_str(&id)?)?),
                JobsCmd::Pause { id } => {
                    let id = uuid::Uuid::parse_str(&id)?;
                    store.pause(id).await?;
                    json!(store.details(id)?)
                }
                JobsCmd::Resume { id } => {
                    let id = uuid::Uuid::parse_str(&id)?;
                    store.resume(id).await?;
                    json!(store.details(id)?)
                }
                JobsCmd::Cancel { id } => {
                    let id = uuid::Uuid::parse_str(&id)?;
                    store.cancel(id).await?;
                    json!(store.details(id)?)
                }
            };
            output(format, value.clone(), serde_json::to_string_pretty(&value)?)
        }
        Command::Swarm { objective } => {
            run_team(TeamRequest::swarm(objective), format, profile.as_deref()).await
        }
        Command::Pack {
            name,
            objective,
            definition,
        } => {
            let pack = if let Some(path) = definition {
                let meta = std::fs::symlink_metadata(&path)?;
                anyhow::ensure!(
                    meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= 100_000,
                    "pack definition must be a regular JSON file under 100000 bytes"
                );
                let pack: chimera_core::pack::PackSpec =
                    serde_json::from_slice(&std::fs::read(path)?)?;
                anyhow::ensure!(pack.name == name, "pack name differs from its definition");
                pack
            } else {
                chimera_core::team::builtin_packs()
                    .into_iter()
                    .find(|p| p.name == name)
                    .context("unknown pack; supply --definition for an explicit JSON pack")?
            };
            let objective = objective.context("a pack needs --objective describing the work")?;
            run_team(
                TeamRequest::from_pack(&pack, objective)?,
                format,
                profile.as_deref(),
            )
            .await
        }
        Command::Creature {
            cmd:
                CreatureCmd::Spawn {
                    name,
                    count,
                    objective,
                },
        } => {
            anyhow::ensure!(
                (1..=8).contains(&count),
                "specialist count must be from 1 through 8"
            );
            let objective = objective
                .context("spawning specialists needs --objective describing their work")?;
            let request = TeamRequest {
                objective,
                creatures: vec![name; count as usize],
                scope: Default::default(),
                max_parallel: count as usize,
                read_only: false,
            };
            run_team(request, format, profile.as_deref()).await
        }
        Command::Creature {
            cmd: CreatureCmd::List,
        } => {
            let teams = TeamRegistry::new(workspace_root())?.list()?;
            output(
                format,
                json!({"specialists":chimera_core::team::specialists(),"teams":teams}),
                serde_json::to_string_pretty(&teams)?,
            )
        }
        Command::Creature { cmd } => {
            let registry = TeamRegistry::new(workspace_root())?;
            let text = match &cmd {
                CreatureCmd::Inspect { instance }
                | CreatureCmd::Stop { instance }
                | CreatureCmd::Promote { instance }
                | CreatureCmd::Leash { instance } => instance,
                _ => unreachable!(),
            };
            let id = uuid::Uuid::parse_str(text)
                .context("use the saved specialist identifier from creature list")?;
            let (team, member) = registry.find_member(id)?;
            let value = match cmd {
                CreatureCmd::Inspect { .. } => json!({"team":team.id,"member":member}),
                CreatureCmd::Stop { .. } => json!(registry.stop_member(team.id, id)?),
                CreatureCmd::Promote { .. } => json!(registry.promote(team.id, id)?),
                CreatureCmd::Leash { .. } => json!(registry.leash(team.id, id)?),
                _ => unreachable!(),
            };
            output(format, value.clone(), serde_json::to_string_pretty(&value)?)
        }
        Command::Leash { target } => {
            let registry = TeamRegistry::new(workspace_root())?;
            let id = uuid::Uuid::parse_str(&target)?;
            let (team, _) = registry.find_member(id)?;
            let value = registry.leash(team.id, id)?;
            output(
                format,
                json!(value),
                format!("Specialist {id} now requires approval for every action."),
            )
        }
        Command::Trace {
            cmd: TraceCmd::Live,
        } => {
            let store = chimera_session::FileSessionStore::new(workspace_root());
            let mut counts = std::collections::HashMap::new();
            loop {
                for id in store.list().await? {
                    let state = store.load(id).await?;
                    let count = counts.entry(id).or_insert(0);
                    // Live diagnostics show types and status, never private message bodies.
                    for (index, event) in state.events.iter().enumerate().skip(*count) {
                        let name = match event {
                            chimera_session::SessionEvent::Custom { key, .. } => key.as_str(),
                            _ => "session_event",
                        };
                        output(
                            format,
                            json!({"session":id,"event_number":index,"kind":name,"active":state.active}),
                            format!("{id} #{index} {name}"),
                        )?;
                    }
                    *count = state.events.len();
                }
                tokio::select! {_=tokio::signal::ctrl_c()=>break,_=tokio::time::sleep(std::time::Duration::from_millis(300))=>()}
            }
            Ok(())
        }
        unfinished => {
            anyhow::bail!("This command is unavailable: {unfinished:?}. No work was performed.")
        }
    }
}
