//! Tool execution bound to one persisted local world, including its approvals.
use anyhow::{Context, Result};
use chimera_sandbox::{LocalWorldManager, WorldId, WorldManager, WorldState, WorldStatus};
use chimera_tools::{
    DefaultToolRouter, ToolCall, ToolDescriptor, ToolPolicy, ToolResult, ToolRouter,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub struct WorldTools {
    manager: Arc<LocalWorldManager>,
    world: WorldId,
    root: PathBuf,
    specification: Value,
    inner: DefaultToolRouter,
    approvals: Mutex<HashMap<String, Instant>>,
}

impl WorldTools {
    pub async fn new(manager: Arc<LocalWorldManager>, world: WorldId) -> Result<Self> {
        Self::with_read_only(manager, world, false).await
    }

    /// Read-only mode may further restrict a world; it cannot make it writable.
    pub async fn with_read_only(
        manager: Arc<LocalWorldManager>,
        world: WorldId,
        read_only: bool,
    ) -> Result<Self> {
        let operation = manager.claim_operation(world)?;
        let state = &operation.state;
        let root = state
            .root_path
            .as_ref()
            .context("world has no root")?
            .canonicalize()?;
        anyhow::ensure!(
            Some(&root) == state.root_path.as_ref(),
            "world root changed its physical location"
        );
        let policy = if read_only || !state.spec.writable {
            ToolPolicy::read_only()
        } else {
            ToolPolicy::supervised()
        };
        let router = Self {
            manager,
            world,
            root: root.clone(),
            specification: serde_json::to_value(&state.spec)?,
            inner: DefaultToolRouter::with_policy(Some(root), policy),
            approvals: Mutex::new(HashMap::new()),
        };
        drop(operation);
        Ok(router)
    }

    pub fn world(&self) -> WorldId {
        self.world
    }
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn validate_state(&self, state: &WorldState) -> Result<()> {
        anyhow::ensure!(
            state.id == self.world && state.status == WorldStatus::Running,
            "selected world is not running: {:?}",
            state.status
        );
        anyhow::ensure!(
            state.root_path.as_ref() == Some(&self.root),
            "selected world root changed; refusing host fallback"
        );
        anyhow::ensure!(
            serde_json::to_value(&state.spec)? == self.specification,
            "selected world restrictions changed; attach it again explicitly"
        );
        anyhow::ensure!(
            !std::fs::symlink_metadata(&self.root)?
                .file_type()
                .is_symlink()
                && self.root.canonicalize()? == self.root,
            "selected world root was replaced"
        );
        if let Some(limit) = state.spec.limits.timeout_secs {
            let elapsed = (chrono::Utc::now() - state.created_at).num_seconds().max(0) as u64;
            anyhow::ensure!(elapsed < limit, "selected world lifetime has expired");
        }
        Ok(())
    }

    async fn check_state(&self) -> Result<()> {
        self.validate_state(&self.manager.inspect(self.world).await?)
    }

    fn check_identity(&self, call: &ToolCall) -> Result<()> {
        anyhow::ensure!(
            call.world == Some(self.world),
            "action must name the selected world {}",
            self.world
        );
        anyhow::ensure!(
            call.args.is_object() && serde_json::to_vec(&call.args)?.len() <= 1_048_576,
            "tool arguments must be an object of at most 1 MiB"
        );
        Ok(())
    }

    fn local_call(&self, call: &ToolCall) -> ToolCall {
        let mut local = call.clone();
        // Only this private, root-bound router receives the stripped identity.
        // The approval and saved event keep the actual world identifier.
        local.world = None;
        local
    }

    async fn descriptor(&self, name: &str) -> Result<ToolDescriptor> {
        self.inner
            .list()
            .await?
            .into_iter()
            .find(|tool| tool.name == name)
            .context("unknown or unavailable tool in selected world")
    }

    async fn shell(&self, call: &ToolCall) -> Result<ToolResult> {
        let (command, cwd, timeout) = if call.tool == "bash" {
            let command = call
                .args
                .get("command")
                .and_then(Value::as_str)
                .context("command must be a string")?;
            let cwd = optional_string(&call.args, "cwd")?.unwrap_or(".");
            let timeout = match call.args.get("timeout_secs") {
                None => 120,
                Some(value) => value
                    .as_u64()
                    .context("timeout_secs must be a positive integer")?,
            };
            (command, cwd, timeout)
        } else {
            ("git -c core.fsmonitor=false -c core.untrackedCache=false --no-optional-locks status --porcelain=v1", optional_string(&call.args, "path")?.unwrap_or("."), 30)
        };
        anyhow::ensure!(!command.is_empty(), "command is empty");
        let output = self
            .manager
            .execute_in(self.world, Path::new(cwd), command, timeout)
            .await?;
        if call.tool == "git_status" {
            let files: Vec<Value> = output.stdout.lines().filter(|line| !line.is_empty()).map(|line|json!({"status":line.get(..2).unwrap_or("").trim(),"path":line.get(3..).unwrap_or("")})).collect();
            Ok(ToolResult {
                ok: output.exit_code == 0,
                output: json!({"files":files,"exit_code":output.exit_code,"stderr":output.stderr,"world":self.world}),
                evidence: vec![],
            })
        } else {
            Ok(ToolResult {
                ok: output.exit_code == 0,
                output: json!({"exit_code":output.exit_code,"stdout":output.stdout,"stderr":output.stderr,"duration_ms":output.duration_ms,"world":self.world}),
                evidence: vec![],
            })
        }
    }
}

fn optional_string<'a>(args: &'a Value, name: &str) -> Result<Option<&'a str>> {
    args.get(name)
        .map(|value| {
            value
                .as_str()
                .with_context(|| format!("{name} must be a string"))
        })
        .transpose()
}

#[async_trait::async_trait]
impl ToolRouter for WorldTools {
    async fn bind_call(&self, mut call: ToolCall) -> Result<ToolCall> {
        anyhow::ensure!(
            call.world.is_none() || call.world == Some(self.world),
            "requested world differs from selected world {}",
            self.world
        );
        call.world = Some(self.world);
        self.check_identity(&call)?;
        self.check_state().await?;
        self.descriptor(&call.tool).await?;
        Ok(call)
    }

    async fn approval_required(&self, call: &ToolCall) -> Result<bool> {
        self.check_identity(call)?;
        self.check_state().await?;
        Ok(!self.descriptor(&call.tool).await?.read_only)
    }

    async fn approve_once(&self, call: &ToolCall) -> Result<()> {
        self.check_identity(call)?;
        self.check_state().await?;
        anyhow::ensure!(
            !self.descriptor(&call.tool).await?.read_only,
            "this world tool does not need a mutation grant"
        );
        let mut approvals = self
            .approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("world approvals unavailable"))?;
        approvals.retain(|_, expiry| *expiry > Instant::now());
        anyhow::ensure!(approvals.len() < 128, "too many pending world approvals");
        approvals.insert(
            serde_json::to_string(call)?,
            Instant::now() + Duration::from_secs(120),
        );
        Ok(())
    }

    async fn deny(&self, call: &ToolCall) -> Result<()> {
        self.approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("world approvals unavailable"))?
            .remove(&serde_json::to_string(call)?);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<ToolDescriptor>> {
        self.check_state().await?;
        self.inner.list().await
    }
    async fn input_schema(&self, tool: &str) -> Result<Option<Value>> {
        self.inner.input_schema(tool).await
    }
    async fn healthcheck(&self, tool: &str) -> Result<bool> {
        self.check_state().await?;
        Ok(self.descriptor(tool).await.is_ok())
    }

    async fn call(&self, call: ToolCall) -> Result<ToolResult> {
        let call = self.bind_call(call).await?;
        let descriptor = self.descriptor(&call.tool).await?;
        if !descriptor.read_only {
            let expiry = self
                .approvals
                .lock()
                .map_err(|_| anyhow::anyhow!("world approvals unavailable"))?
                .remove(&serde_json::to_string(&call)?);
            anyhow::ensure!(
                expiry.is_some_and(|until| until > Instant::now()),
                "approval required for this exact action in world {}",
                self.world
            );
        }
        if matches!(call.tool.as_str(), "bash" | "git_status") {
            return self.shell(&call).await;
        }
        let operation = self.manager.claim_operation(self.world)?;
        self.validate_state(&operation.state)?;
        let local = self.local_call(&call);
        if !descriptor.read_only {
            self.inner.approve_once(&local).await?;
        }
        let watchdog = async {
            loop {
                tokio::time::sleep(Duration::from_millis(30)).await;
                self.check_state().await?;
            }
        };
        let result = tokio::select! {
            result = self.inner.call(local) => result,
            stopped = watchdog => { let stopped: Result<()> = stopped; stopped?; anyhow::bail!("selected world stopped") },
        };
        // A close can be saved while a bounded synchronous file call finishes.
        // Keep ownership until that decision has also been observed.
        self.check_state().await?;
        drop(operation);
        result
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::{
        agent::{AgentConfig, DoneReason},
        runtime::{event_channel, RunEvent, SavedSession},
    };
    use chimera_llm::{
        CompletionRequest, CompletionResponse, FunctionCall, LlmError, LlmProvider,
        ToolCall as ToolCallRequest,
    };
    use chimera_sandbox::{ResourceLimits, WorldKind, WorldSpec};
    use tokio::sync::watch;
    use uuid::Uuid;

    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!("chimera-world-tools-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        root.canonicalize().unwrap()
    }
    fn spec(kind: WorldKind, writable: bool) -> WorldSpec {
        WorldSpec {
            kind,
            writable,
            image: None,
            root: None,
            env: HashMap::new(),
            network: false,
            label: Some("controlled world tool test".into()),
            limits: ResourceLimits::default(),
        }
    }
    async fn git(root: &Path, args: &[&str]) {
        let output = tokio::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    async fn repository() -> PathBuf {
        let root = fixture();
        git(&root, &["init", "-q"]).await;
        std::fs::write(root.join("source.txt"), "original").unwrap();
        git(&root, &["add", "source.txt"]).await;
        git(
            &root,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-qm",
                "fixture",
            ],
        )
        .await;
        root
    }
    fn call(tool: &str, args: Value, world: WorldId, actor: Uuid) -> ToolCall {
        ToolCall {
            tool: tool.into(),
            args,
            world: Some(world),
            requested_by: actor,
        }
    }
    struct WorldModel;
    #[async_trait::async_trait]
    impl LlmProvider for WorldModel {
        fn name(&self) -> &str {
            "controlled-world-model"
        }
        async fn complete(
            &self,
            request: &CompletionRequest,
        ) -> std::result::Result<CompletionResponse, LlmError> {
            let count = request
                .messages
                .iter()
                .filter(|m| m.role == chimera_llm::Role::Tool)
                .count();
            let (name, arguments) = match count {
                0 => ("read_file", json!({"path":"source.txt"})),
                1 => {
                    assert!(request
                        .messages
                        .last()
                        .unwrap()
                        .content
                        .as_ref()
                        .unwrap()
                        .contains("original"));
                    (
                        "write_file",
                        json!({"path":"source.txt","content":"edited in world"}),
                    )
                }
                2 => (
                    "bash",
                    json!({"command":"printf shell > shell.txt; cat source.txt","timeout_secs":5}),
                ),
                3 => {
                    assert!(request
                        .messages
                        .last()
                        .unwrap()
                        .content
                        .as_ref()
                        .unwrap()
                        .contains("edited in world"));
                    return Ok(CompletionResponse {
                        content: Some("verified world work".into()),
                        tool_calls: vec![],
                        finish_reason: Some("stop".into()),
                        usage: None,
                    });
                }
                _ => panic!("unexpected model turn"),
            };
            Ok(CompletionResponse {
                content: None,
                tool_calls: vec![ToolCallRequest {
                    id: format!("world-{count}"),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: name.into(),
                        arguments: arguments.to_string(),
                    },
                }],
                finish_reason: Some("tool_calls".into()),
                usage: None,
            })
        }
    }

    #[tokio::test]
    async fn saved_agent_reads_writes_and_executes_only_in_selected_worktree() {
        let root = repository().await;
        let manager = Arc::new(LocalWorldManager::new(root.clone()).unwrap());
        let id = manager
            .allocate(spec(WorldKind::WorktreeReadWrite, true))
            .await
            .unwrap();
        let tools = Arc::new(WorldTools::new(manager.clone(), id).await.unwrap());
        let world_root = tools.root().to_owned();
        let mut session = SavedSession::create(root.clone(), "world work")
            .await
            .unwrap();
        let (events, mut receive) = event_channel();
        let (_stop, stopped) = watch::channel(false);
        let decisions = tokio::spawn(async move {
            let mut approvals = vec![];
            while let Some(event) = receive.recv().await {
                if let RunEvent::Approval { call, answer, .. } = event {
                    assert_eq!(call.world, Some(id));
                    approvals.push(call.tool);
                    answer.send(true).unwrap();
                }
            }
            approvals
        });
        let result = session
            .run(
                &WorldModel,
                tools,
                "test",
                "world work",
                &AgentConfig::default(),
                crate::runtime::RunChannels { events, cancellation: stopped },
            )
            .await
            .unwrap();
        assert!(
            matches!(result.done_reason, DoneReason::FinalAnswer),
            "{:?}",
            result.error
        );
        assert_eq!(decisions.await.unwrap(), vec!["write_file", "bash"]);
        assert_eq!(
            std::fs::read_to_string(root.join("source.txt")).unwrap(),
            "original"
        );
        assert!(!root.join("shell.txt").exists());
        assert_eq!(
            std::fs::read_to_string(world_root.join("source.txt")).unwrap(),
            "edited in world"
        );
        assert_eq!(
            std::fs::read_to_string(world_root.join("shell.txt")).unwrap(),
            "shell"
        );
        let reopened = SavedSession::resume(root, session.id).await.unwrap();
        assert_eq!(
            reopened
                .messages
                .iter()
                .filter(|m| m.role == chimera_llm::Role::Tool)
                .count(),
            3
        );
        manager.destroy(id).await.unwrap();
        let preserved = manager.inspect(id).await.unwrap().root_path.unwrap();
        assert_eq!(
            std::fs::read_to_string(preserved.join("source.txt")).unwrap(),
            "edited in world"
        );
    }

    #[tokio::test]
    async fn wrong_world_changed_approval_and_readonly_requests_fail_before_write() {
        let root = fixture();
        let manager = Arc::new(LocalWorldManager::new(root.clone()).unwrap());
        let id = manager
            .allocate(spec(WorldKind::LocalShell, true))
            .await
            .unwrap();
        let router = WorldTools::new(manager.clone(), id).await.unwrap();
        let actor = Uuid::new_v4();
        let create = call(
            "write_file",
            json!({"path":"new.txt","content":"allowed"}),
            id,
            actor,
        );
        let mut wrong = create.clone();
        wrong.world = Some(Uuid::new_v4());
        assert!(router.bind_call(wrong).await.is_err());
        let mut unbound = create.clone();
        unbound.world = None;
        assert!(router.approve_once(&unbound).await.is_err());
        assert_eq!(router.bind_call(unbound).await.unwrap().world, Some(id));
        router.approve_once(&create).await.unwrap();
        let mut changed = create.clone();
        changed.args["content"] = json!("changed");
        assert!(router.call(changed).await.is_err());
        assert!(!root.join("new.txt").exists());
        assert!(router.call(create.clone()).await.unwrap().ok);
        let readonly = WorldTools::with_read_only(manager.clone(), id, true)
            .await
            .unwrap();
        assert!(readonly.approve_once(&create).await.is_err());
        assert!(!readonly
            .list()
            .await
            .unwrap()
            .iter()
            .any(|tool| tool.name == "bash" || tool.name == "write_file"));
        manager.pause(id).await.unwrap();
        assert!(router
            .call(call("read_file", json!({"path":"new.txt"}), id, actor))
            .await
            .is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("new.txt")).unwrap(),
            "allowed"
        );
    }

    #[tokio::test]
    async fn closing_world_cancels_owned_shell_and_never_runs_later_write() {
        let root = fixture();
        let manager = Arc::new(LocalWorldManager::new(root.clone()).unwrap());
        let id = manager
            .allocate(spec(WorldKind::LocalShell, true))
            .await
            .unwrap();
        let router = Arc::new(WorldTools::new(manager.clone(), id).await.unwrap());
        let request = call(
            "bash",
            json!({"command":"printf ready > ready; (sleep 2; printf bad > late) & wait","timeout_secs":10}),
            id,
            Uuid::new_v4(),
        );
        router.approve_once(&request).await.unwrap();
        let task = tokio::spawn(async move { router.call(request).await });
        for _ in 0..100 {
            if root.join("ready").exists() {
                break;
            }
            if task.is_finished() {
                panic!("shell stopped before ready: {:?}", task.await.unwrap());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            root.join("ready").exists(),
            "command never wrote its start marker"
        );
        let other = LocalWorldManager::new(root.clone()).unwrap();
        other.destroy(id).await.unwrap();
        let result = task.await.unwrap().unwrap();
        assert!(!result.ok);
        assert_eq!(result.output["exit_code"], 130);
        tokio::time::sleep(Duration::from_millis(2100)).await;
        assert!(!root.join("late").exists());
        assert_eq!(
            other.inspect(id).await.unwrap().status,
            WorldStatus::Destroyed
        );
    }

    #[tokio::test]
    async fn selected_world_rejects_host_cwd_and_expired_lifetime() {
        let root = fixture();
        let manager = Arc::new(LocalWorldManager::new(root).unwrap());
        let mut short = spec(WorldKind::LocalShell, true);
        short.limits.timeout_secs = Some(1);
        let id = manager.allocate(short).await.unwrap();
        let router = WorldTools::new(manager.clone(), id).await.unwrap();
        let request = call(
            "bash",
            json!({"command":"true","cwd":"/","timeout_secs":2}),
            id,
            Uuid::new_v4(),
        );
        router.approve_once(&request).await.unwrap();
        assert!(router
            .call(request)
            .await
            .unwrap_err()
            .to_string()
            .contains("outside"));
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert!(router
            .list()
            .await
            .unwrap_err()
            .to_string()
            .contains("expired"));
        assert!(manager.claim_operation(id).is_err());
    }

    #[tokio::test]
    async fn close_waits_for_file_operation_ownership_and_denies_new_work() {
        let root = fixture();
        let manager = Arc::new(LocalWorldManager::new(root.clone()).unwrap());
        let id = manager
            .allocate(spec(WorldKind::LocalShell, true))
            .await
            .unwrap();
        let guard = manager.claim_operation(id).unwrap();
        assert!(manager.claim_operation(id).is_err());
        let other = LocalWorldManager::new(root.clone()).unwrap();
        let closing = tokio::spawn(async move { other.destroy(id).await });
        for _ in 0..100 {
            if manager.inspect(id).await.unwrap().status == WorldStatus::Destroying {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            manager.inspect(id).await.unwrap().status,
            WorldStatus::Destroying
        );
        assert!(!closing.is_finished());
        assert!(manager.claim_operation(id).is_err());
        drop(guard);
        tokio::time::timeout(Duration::from_secs(1), closing)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            manager.inspect(id).await.unwrap().status,
            WorldStatus::Destroyed
        );
        assert!(root.exists());
        assert!(manager.claim_operation(id).is_err());
    }
}
