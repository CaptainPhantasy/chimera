use super::*;
use chimera_core::skill::SkillCapture;
use chimera_eval::Evaluator;
use chimera_memory::MemoryStore;
use chimera_mcp::McpClient;
use chimera_sandbox::WorldManager;
use std::path::Path;

pub(crate) async fn approve(call: chimera_tools::ToolCall, format: OutputFormat) -> Result<()> {
    #[cfg(unix)]
    let mut interrupts=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    use chimera_core::approval::{ApprovalStore,Decision};
    let store=ApprovalStore::new(workspace_root());
    let pending=store.request(call.requested_by,call.clone(),std::time::Duration::from_secs(300))?;
    struct CancelPending(ApprovalStore,uuid::Uuid);
    impl Drop for CancelPending {fn drop(&mut self){let _=self.0.cancel(self.1);}}
    let _pending_guard=CancelPending(ApprovalStore::new(workspace_root()),pending.id);
    let (answer, receive)=tokio::sync::oneshot::channel();
    let flow=tokio::select! {
        biased;
        _=async {
            #[cfg(unix)] {let _=interrupts.recv().await;}
            #[cfg(not(unix))] {let _=tokio::signal::ctrl_c().await;}
        }=>anyhow::bail!("action cancelled; nothing executed"),
        result=show_run_event(RunEvent::Approval { id:pending.id,call:call.clone(),answer },format,None)=>result?,
    };
    anyhow::ensure!(!matches!(flow,RunEventFlow::Cancelled(_)),"action cancelled; nothing executed");
    match receive.await {
        Ok(approved) => {if store.get(pending.id)?.decision==Decision::Pending {store.decide(pending.id,approved)?;}},
        Err(_) if std::env::var("CHIMERA_WAIT_APPROVAL").as_deref()==Ok("1")=> {
            while store.get(pending.id)?.decision==Decision::Pending {
                tokio::select! {
                    _=tokio::time::sleep(std::time::Duration::from_millis(100))=>(),
                    _=async {
                        #[cfg(unix)] {let _=interrupts.recv().await;}
                        #[cfg(not(unix))] {let _=tokio::signal::ctrl_c().await;}
                    }=>anyhow::bail!("action cancelled; nothing executed"),
                }
            }
        },
        Err(_)=>anyhow::bail!("approval display closed; nothing executed"),
    }
    anyhow::ensure!(store.get(pending.id)?.decision==Decision::Approved,"action denied or expired; nothing executed");
    store.consume(pending.id,call.requested_by,&call)?;
    Ok(())
}

struct CommandChecks { format: OutputFormat, profile: Option<String> }
#[async_trait::async_trait]
impl chimera_eval::CheckRunner for CommandChecks {
    async fn run(&self, command:&str, cwd:&Path, timeout_secs:u64) -> Result<chimera_eval::CheckOutput> {
        anyhow::ensure!(cwd.canonicalize()? == workspace_root().canonicalize()?,"check belongs to another project");
        let saved_policy=chimera_core::approval::PolicyStore::new(workspace_root());
        saved_policy.check("bash",false)?;
        let router=chimera_tools::DefaultToolRouter::with_policy(Some(cwd.into()),policy(self.profile.as_deref(),false)?);
        let call=chimera_tools::ToolCall { tool:"bash".into(),args:json!({"command":command,"timeout_secs":timeout_secs}),world:None,requested_by:uuid::Uuid::new_v4() };
        if router.approval_required(&call).await? { approve(call.clone(),self.format).await?; router.approve_once(&call).await?; }
        saved_policy.check("bash",false)?;
        let result=router.call(call).await?;
        Ok(chimera_eval::CheckOutput {
            exit_code:result.output["exit_code"].as_i64().unwrap_or(if result.ok {0} else {1}) as i32,
            stdout:result.output["stdout"].as_str().unwrap_or("").into(),
            stderr:result.output["stderr"].as_str().or_else(||result.output["error"].as_str()).unwrap_or("").into(),
            timed_out:result.output["timed_out"].as_bool().unwrap_or(false),
        })
    }
}

pub(super) async fn execute(command: Command, format:OutputFormat, profile:Option<String>) -> Result<()> {
    match command {
        Command::Notify {cmd}=>notify_commands::execute(cmd,format,profile.as_deref()).await,
        Command::Approve {cmd}=> {
            use chimera_core::approval::{ApprovalStore,Decision};
            let store=ApprovalStore::new(workspace_root());
            match cmd {
                ApproveCmd::List=> {let rows:Vec<_>=store.list()?.into_iter().filter(|r|r.decision==Decision::Pending).collect();let shown=chimera_core::approval::display_value(json!({"actions":rows}));output(format,shown.clone(),serde_json::to_string_pretty(&shown)?)},
                ApproveCmd::Action {id}=> {let id=uuid::Uuid::parse_str(&id)?;let row=store.get(id)?;let shown=chimera_core::approval::display_value(json!({"action":row}));output(format,shown.clone(),format!("Approving {} {} once",row.call.tool,shown["action"]["call"]["args"]))?;let row=store.decide(id,true)?;output(format,chimera_core::approval::display_value(json!({"decision":row})),format!("Approved {id} once."))},
                ApproveCmd::Next=> {let row=store.list()?.into_iter().find(|r|r.decision==Decision::Pending).context("no pending action")?;approve(row.call.clone(),format).await?;store.decide(row.id,true)?;output(format,json!({"approved":row.id}),format!("Approved {} once.",row.id))},
                ApproveCmd::Plan=>anyhow::bail!("blanket plan approval is unavailable; use approve list and approve action <id> for an exact pending action"),
            }
        }
        Command::Deny {call_id}=> {let id=uuid::Uuid::parse_str(&call_id)?;chimera_core::approval::ApprovalStore::new(workspace_root()).decide(id,false)?;output(format,json!({"denied":id}),format!("Denied {id}."))}
        Command::Policy {cmd:PolicyCmd::Edit}=>output(format,json!({"path":workspace_root().join(".chimera/policy/settings.json"),"commands":["policy set supervised","policy set read-only","autonomy set none","autonomy set low","autonomy set medium"]}),"Use policy set supervised or policy set read-only. Use autonomy set none to stop tools, low to review every action, or medium to review changes and commands."),
        Command::Policy {cmd:PolicyCmd::Set {profile}}=> {
            anyhow::ensure!(matches!(profile.as_str(),"supervised"|"read-only"),"profile must be supervised or read-only");
            let saved=chimera_core::approval::PolicyStore::new(workspace_root()).change(|p|{p.read_only=profile=="read-only";Ok(())})?;
            output(format,json!(saved),format!("Project policy saved: {profile}."))
        }
        Command::Autonomy {cmd:AutonomyCmd::Set {level}}=> {
            let saved=chimera_core::approval::PolicyStore::new(workspace_root()).change(|p|{p.autonomy=level.clone();Ok(())})?;
            output(format,json!(saved),format!("Action review saved: {level}."))
        }
        Command::Leash {target} if target=="all"=> {
            let saved=chimera_core::approval::PolicyStore::new(workspace_root()).change(|p|{p.autonomy="low".into();Ok(())})?;
            output(format,json!(saved),"Every agent tool action now needs its own approval.")
        }
        Command::Tool {cmd:cmd @ (ToolCmd::Disable {..} | ToolCmd::Enable {..})}=> {
            let (name,enable)=match cmd {ToolCmd::Enable {name}=>(name,true),ToolCmd::Disable {name}=>(name,false),_=>unreachable!()};
            let router=chimera_tools::DefaultToolRouter::new(Some(workspace_root()));
            anyhow::ensure!(router.list().await?.iter().any(|t|t.name==name),"unknown tool: {name}");
            let saved=chimera_core::approval::PolicyStore::new(workspace_root()).change(|p|{if enable {p.disabled_tools.remove(&name);} else {p.disabled_tools.insert(name.clone());}Ok(())})?;
            output(format,json!(saved),format!("{name}: {}",if enable {"enabled; normal action approvals still apply"} else {"disabled"}))
        }
        Command::Tool {cmd:ToolCmd::Test {name}}=> {
            let router=chimera_tools::DefaultToolRouter::with_policy(Some(workspace_root()),policy(profile.as_deref(),false)?);
            let args=match name.as_str() {"glob"=>json!({"pattern":"*","path":"."}),"grep"=>json!({"pattern":"CHIMERA","path":"."}),"git_status"=>json!({"path":"."}),_=>anyhow::bail!("{name} has no safe generic test; use a real permitted call with explicit arguments")};
            chimera_core::approval::PolicyStore::new(workspace_root()).check(&name,true)?;
            let result=router.call(chimera_tools::ToolCall {tool:name,args,world:None,requested_by:uuid::Uuid::new_v4()}).await?;
            output(format,json!(result),serde_json::to_string_pretty(&result)?)?;anyhow::ensure!(result.ok,"tool test failed");Ok(())
        }
        Command::Browser {cmd:BrowserCmd::Open {target,actions,allowed_origins,output_dir}}=>device_commands::browser(target,actions,allowed_origins,output_dir,format,profile.as_deref()).await,
        Command::Phone {cmd:PhoneCmd::Boot {device,app,package,network,output_dir}}=>device_commands::phone(device,app,package,network,output_dir,format,profile.as_deref()).await,
        Command::World {cmd} => {
            let manager=chimera_sandbox::LocalWorldManager::new(workspace_root())?;
            match cmd {
                WorldCmd::List => {
                    let mut worlds=Vec::new();
                    for id in manager.list().await? { worlds.push(manager.inspect(id).await?); }
                    output(format,json!({"worlds":worlds}),worlds.iter().map(|w|format!("{} {:?} {}",w.id,w.status,w.root_path.as_deref().unwrap_or(Path::new("")).display())).collect::<Vec<_>>().join("\n"))
                }
                WorldCmd::Spawn {kind} => {
                    use chimera_sandbox::{WorldKind,WorldSpec,ResourceLimits};
                    let kind=match kind.as_str() {
                        "local_shell"|"local-shell"=>WorldKind::LocalShell,
                        "worktree"|"worktree_read_write"=>WorldKind::WorktreeReadWrite,
                        "worktree_read_only"=>WorldKind::WorktreeReadOnly,
                        _=>anyhow::bail!("environment type {kind} is unavailable; supported types are local_shell, worktree_read_only and worktree_read_write"),
                    };
                    let read_only=policy(profile.as_deref(),false)?.read_only || kind==WorldKind::WorktreeReadOnly;
                    let id=manager.allocate(WorldSpec {kind,image:None,root:Some(workspace_root()),env:Default::default(),network:false,writable:!read_only,label:None,limits:ResourceLimits::default()}).await?;
                    let state=manager.inspect(id).await?;
                    output(format,json!(state),format!("Environment {id}: {}",state.root_path.as_deref().unwrap_or(Path::new("")).display()))
                }
                WorldCmd::Inspect {id}=> {
                    let state=manager.inspect(uuid::Uuid::parse_str(&id)?).await?;
                    output(format,json!(state),serde_json::to_string_pretty(&state)?)
                }
                WorldCmd::Close {id}=> { let id=uuid::Uuid::parse_str(&id)?; manager.destroy(id).await?; output(format,json!({"closed":id}),format!("Closed {id}. Any working copy was preserved in quarantine.")) }
                WorldCmd::Pause {id}=> { let id=uuid::Uuid::parse_str(&id)?; manager.pause(id).await?; output(format,json!({"paused":id}),format!("Paused {id}")) }
                WorldCmd::Resume {id}=> { let id=uuid::Uuid::parse_str(&id)?; manager.resume(id).await?; output(format,json!({"resumed":id}),format!("Resumed {id}")) }
                WorldCmd::Exec {id,command,timeout}=> {
                    anyhow::ensure!(!policy(profile.as_deref(),false)?.read_only,"read-only policy rejects shell commands");
                    let saved_policy=chimera_core::approval::PolicyStore::new(workspace_root());
                    saved_policy.check("bash",false)?;
                    let id=uuid::Uuid::parse_str(&id)?;
                    let state=manager.inspect(id).await?;
                    approve(chimera_tools::ToolCall {tool:"world_exec".into(),args:json!({"command":command,"root":state.root_path,"timeout_secs":timeout}),world:Some(id),requested_by:uuid::Uuid::new_v4()},format).await?;
                    saved_policy.check("bash",false)?;
                    let result=manager.execute(id,&command,timeout).await?;
                    output(format,json!(result),format!("{}{}",result.stdout,result.stderr))?;
                    anyhow::ensure!(result.exit_code==0,"environment command failed: exit {}",result.exit_code);
                    Ok(())
                }
            }
        }
        Command::Memory {cmd}=> {
            let memory=chimera_memory::FileMemoryStore::new(workspace_root());
            match cmd {
                MemoryCmd::List=> { let items=memory.retrieve(chimera_memory::MemoryQuery {limit:1000,..Default::default()}).await?; output(format,json!({"memory":items}),items.iter().map(|v|format!("{} {}: {}",v.id,v.key,v.content)).collect::<Vec<_>>().join("\n")) }
                MemoryCmd::Show {episode_id}=> { let item=memory.show(uuid::Uuid::parse_str(&episode_id)?).await?; output(format,json!(item),format!("{}: {}",item.key,item.content)) }
                MemoryCmd::Remember {key,content}=> {
                    let now=chrono::Utc::now();
                    let id=memory.remember(chimera_memory::MemoryItem {id:uuid::Uuid::new_v4(),layer:chimera_memory::MemoryLayer::Semantic,key,content,metadata:json!({"source":"user"}),created_at:now,last_accessed:now,session_id:None,creature_id:None,relevance:1.0}).await?;
                    output(format,json!({"memory":id}),format!("Saved project memory {id}"))
                }
                MemoryCmd::Forget {episode_id}=> { let id=uuid::Uuid::parse_str(&episode_id)?; memory.forget(id).await?; output(format,json!({"archived":id}),format!("Archived memory {id}")) }
                MemoryCmd::Compact=> { memory.compact(chimera_memory::MemoryLayer::Semantic).await?; output(format,json!({"compacted":true}),"Combined duplicates; distinct facts and archived originals were preserved.") }
            }
        }
        Command::Skill {cmd}=> {
            let skills=chimera_core::skill::FileSkillCapture::new(workspace_root());
            match cmd {
                SkillCmd::List=> { let rows=skills.list().await?; output(format,json!({"skills":rows}),serde_json::to_string_pretty(&rows)?) }
                SkillCmd::Inspect {name}=> { let value=skills.resolve(&name).await?; output(format,json!(value),serde_json::to_string_pretty(&value)?) }
                SkillCmd::Capture {name,session_id,description}=> { let id=skills.capture_from_session(&name,&description,uuid::Uuid::parse_str(&session_id)?).await?; output(format,json!({"skill":id,"name":name}),format!("Captured workflow {name} ({id})")) }
                SkillCmd::Run {name}=>run_objective(skills.execution_objective(&name).await?,ExecutionMode::Run,format,profile.as_deref()).await,
            }
        }
        Command::Eval {cmd}=> {
            let evaluator=chimera_eval::FileEvaluator::new(workspace_root(),Arc::new(CommandChecks {format,profile}));
            match cmd {
                EvalCmd::Define {name,file,command}=> {
                    evaluator.save_definition(chimera_eval::EvalRequest {name:name.clone(),target:chimera_eval::EvalTarget::Artifact {path:file},graders:vec![chimera_eval::GraderSpec {name:"command".into(),config:json!({"command":command})}]})?;
                    output(format,json!({"evaluation":name}),format!("Saved evaluation {name}. Running it still requires current action approval."))
                }
                EvalCmd::List=> { let values=evaluator.definitions()?; output(format,json!({"evaluations":values}),values.join("\n")) }
                EvalCmd::Run {name}=> { let report=evaluator.run_named(&name).await?; output(format,json!(report),serde_json::to_string_pretty(&report)?)?; anyhow::ensure!(report.passed,"evaluation failed; see saved command output"); Ok(()) }
                EvalCmd::Compare {checkpoint_a,checkpoint_b}=> { let report=evaluator.compare(uuid::Uuid::parse_str(&checkpoint_a)?,uuid::Uuid::parse_str(&checkpoint_b)?).await?; output(format,json!(report),serde_json::to_string_pretty(&report)?) }
            }
        }
        Command::Mcp {config,cmd}=> {
            anyhow::ensure!(!policy(profile.as_deref(),false)?.read_only,"read-only policy does not start external programs");
            let metadata=std::fs::symlink_metadata(&config)?;
            anyhow::ensure!(metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len()<=1_048_576,"tool-server settings must be a regular file no larger than 1 MiB");
            let servers:Vec<chimera_mcp::McpServer>=serde_json::from_slice(&std::fs::read(&config)?)?;
            let server_name=match &cmd {McpCmd::Tools {server}|McpCmd::Call {server,..}|McpCmd::Resources {server}|McpCmd::Read {server,..}|McpCmd::Healthcheck {server}=>server};
            let server=servers.iter().find(|s| &s.name==server_name).context("server name is not configured")?;
            approve(chimera_tools::ToolCall {tool:"external_server".into(),args:json!({"server":server.name,"command":server.command,"arguments":server.args,"operation":format!("{cmd:?}")}),world:None,requested_by:uuid::Uuid::new_v4()},format).await?;
            let client=chimera_mcp::StdioMcpClient::new(servers,chimera_mcp::StdioOptions {working_directory:Some(workspace_root()),..Default::default()})?;
            let result:Result<Value>=async { Ok(match cmd {
                McpCmd::Tools {server}=>json!(client.list_tools(&server).await?),
                McpCmd::Call {server,tool,args}=> { let result=client.invoke(&server,&tool,serde_json::from_str(&args)?).await?; anyhow::ensure!(!result.is_error,"external tool failed: {}",serde_json::to_string(&result)?); json!(result) }
                McpCmd::Resources {server}=>json!(client.list_resources(&server).await?),
                McpCmd::Read {server,uri}=>json!(client.read_resource_contents(&server,&uri).await?),
                McpCmd::Healthcheck {server}=> { anyhow::ensure!(client.ping(&server).await?,"server did not answer"); json!({"server":server,"connected":true}) }
            }) }.await;
            let shutdown = client.shutdown().await;
            let value=result?;
            shutdown.context("external server cleanup failed")?;
            output(format,value.clone(),serde_json::to_string_pretty(&value)?)
        }
        other=>work_commands::execute(other,format,profile).await,
    }
}

/// Startup requires separate exact permission before even discovering server tools.
pub(crate) async fn external_tools(builtins:Arc<dyn ToolRouter>,path:std::path::PathBuf,format:OutputFormat)->Result<Arc<chimera_core::external_tools::ExternalTools>> {
    let meta=std::fs::symlink_metadata(&path)?;
    anyhow::ensure!(meta.is_file()&&!meta.file_type().is_symlink()&&meta.len()<=1_048_576,"external settings must be a regular JSON file under 1 MiB");
    let servers:Vec<chimera_mcp::McpServer>=serde_json::from_slice(&std::fs::read(path)?)?;
    let options=chimera_mcp::StdioOptions {working_directory:Some(workspace_root()),..Default::default()};
    let actor=uuid::Uuid::new_v4();
    let actions=chimera_core::external_tools::startup_actions(&servers,&options,actor)?;
    for action in &actions {approve(action.clone(),format).await?;}
    Ok(Arc::new(chimera_core::external_tools::ExternalTools::discover(builtins,servers,options,actor,&actions).await?))
}
