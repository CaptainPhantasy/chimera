//! chimera-tui::external — In-chat external tool (MCP) connections.
//!
//! Shows the saved connection status, offers a guided setup form that appends
//! one validated entry to the saved connection file, starts a connection once
//! on an explicit check, and attaches the discovered tools to this chat so
//! they are usable in ordinary requests. Every external tool call still goes
//! through the exact-action approval flow in chat; attaching never pre-approves
//! a tool call, only the one server start shown and confirmed in the overlay.

use crate::dialog::{Choice, Dialog, Field};
use anyhow::{Context, Result};
use chimera_core::external_tools::{startup_actions, ExternalTools};
use chimera_mcp::{
    load_servers, save_server, validate_server, McpClient, McpServer, McpTransport, StdioMcpClient,
    StdioOptions,
};
use chimera_tools::{DefaultToolRouter, ToolCall, ToolPolicy};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const MENU_NOTE: &str = "Enter checks a connection (it starts the configured program once) and can then attach it to this chat. Every external tool call asks for approval here in chat.";
const EMPTY_NOTE: &str = "No external tool connections are configured yet. Choose \"Add a connection…\" for guided setup; nothing is started until you check or attach it.";
const SETUP_NOTE: &str = "Only local stdio connections are supported: CHIMERA starts the program you name and speaks its tool protocol over standard input and output. Sign-in is supported by passing credentials as environment variables; they are stored owner-only in the connection file and are never displayed again. Nothing starts until you check or attach the connection.";
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// A finished background step waiting to move into the overlay.
enum Step {
    Listed(Vec<McpServer>),
    Checked {
        server: McpServer,
    },
    Attached {
        tools: Arc<ExternalTools>,
        names: Vec<String>,
        count: usize,
    },
}

/// Which background step is running, so the result lands in the right view.
enum Kind {
    List,
    Check(String),
    Attach(String),
}

/// One exact server start the user confirmed in the overlay, handed to
/// `discover` as the approved startup action.
struct PendingAttach {
    name: String,
    servers: Vec<McpServer>,
    options: StdioOptions,
    actor: uuid::Uuid,
    actions: Vec<ToolCall>,
}

/// External tool connection state for one chat screen.
#[derive(Default)]
pub struct External {
    path: Option<PathBuf>,
    servers: Vec<McpServer>,
    status: BTreeMap<String, String>,
    staged: Option<McpServer>,
    pending_attach: Option<PendingAttach>,
    attached: Option<Arc<ExternalTools>>,
    attached_names: Vec<String>,
    job: Option<tokio::task::JoinHandle<Result<Step>>>,
    kind: Option<Kind>,
}

impl External {
    /// Start listing saved connections and show a waiting panel.
    pub fn open(&mut self, path: Option<PathBuf>) -> Dialog {
        self.begin(path, "Loading saved external tool connections.")
    }

    fn begin(&mut self, path: Option<PathBuf>, note: &str) -> Dialog {
        self.cancel();
        self.path = path.clone();
        self.job = Some(tokio::spawn(async move {
            Self::list(path).await.map(Step::Listed)
        }));
        self.kind = Some(Kind::List);
        Dialog::busy("External tool connections", note)
    }

    fn refresh(&mut self, note: &str) -> Dialog {
        self.begin(self.path.clone(), note)
    }

    /// Every saved connection. A missing file simply means none are configured.
    async fn list(path: Option<PathBuf>) -> Result<Vec<McpServer>> {
        let path = path
            .context("No saved connection file is available; set CHIMERA_MCP_CONFIG or $HOME.")?;
        if !path.exists() {
            return Ok(Vec::new());
        }
        load_servers(&path)
    }

    fn status_text(&self, server: &McpServer) -> String {
        if let Some(status) = self.status.get(&server.name) {
            return status.clone();
        }
        let credentials = server.env.len();
        let arguments = if server.args.is_empty() {
            String::new()
        } else {
            format!(" {}", server.args.join(" "))
        };
        let command = server.command.as_deref().unwrap_or("(no command)");
        format!(
            "configured · stdio · {command}{arguments} · {credentials} credential variables — Enter to check"
        )
    }

    /// The saved connection menu: one row per connection plus setup and detach.
    fn menu(&self, note: &str) -> Dialog {
        let mut rows = Vec::new();
        for server in &self.servers {
            let label = if self.attached_names.contains(&server.name) {
                format!("{} — attached", server.name)
            } else {
                server.name.clone()
            };
            rows.push(Choice::new(
                label,
                self.status_text(server),
                format!("external:check:{}", server.name),
            ));
        }
        rows.push(Choice::new(
            "Add a connection…",
            "Saved guided setup; no config file editing.",
            "external:setup",
        ));
        if self.attached.is_some() {
            rows.push(Choice::new(
                "Detach external tools",
                "Stop the attached server processes; tools leave the chat.",
                "external:detach",
            ));
        }
        Dialog::choices("External tool connections", note, rows)
    }

    /// The guided setup form. The credential field is masked like an API key.
    fn form(staged: Option<&McpServer>, note: &str) -> Dialog {
        let (name, command, arguments, env) = match staged {
            Some(server) => (
                server.name.clone(),
                server.command.clone().unwrap_or_default(),
                server.args.join(" "),
                server
                    .env
                    .iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            None => (String::new(), String::new(), String::new(), String::new()),
        };
        Dialog::form(
            "Add an external connection",
            note,
            vec![
                Field::new("Connection name", &name, false),
                Field::new("Program to start", &command, false),
                Field::new(
                    "Arguments (quote arguments containing spaces)",
                    &arguments,
                    false,
                ),
                Field::new(
                    "Environment credentials (NAME=value, comma separated)",
                    &env,
                    true,
                ),
            ],
            "external:save",
        )
    }

    /// Parse the quoted argument field: whitespace separated, quotes group.
    fn split_arguments(input: &str) -> Result<Vec<String>> {
        let mut arguments = Vec::new();
        let mut current = String::new();
        let mut quote: Option<char> = None;
        for character in input.chars() {
            match quote {
                Some(open) if character == open => quote = None,
                Some(_) => current.push(character),
                None if character == '"' || character == '\'' => quote = Some(character),
                None if character.is_whitespace() => {
                    if !current.is_empty() {
                        arguments.push(std::mem::take(&mut current));
                    }
                }
                None => current.push(character),
            }
        }
        anyhow::ensure!(quote.is_none(), "An argument quote is not closed.");
        if !current.is_empty() {
            arguments.push(current);
        }
        Ok(arguments)
    }

    /// Parse the credential field: NAME=value pairs, comma separated.
    fn parse_env(input: &str) -> Result<HashMap<String, String>> {
        let mut env = HashMap::new();
        for pair in input.split(',') {
            let pair = pair.trim();
            if pair.is_empty() {
                continue;
            }
            let (name, value) = pair
                .split_once('=')
                .with_context(|| format!("Environment entry {pair:?} needs the form NAME=value"))?;
            let name = name.trim().to_string();
            anyhow::ensure!(
                env.insert(name.clone(), value.to_string()).is_none(),
                "Environment variable {name} is set twice."
            );
        }
        Ok(env)
    }

    fn server(&self, name: &str) -> Result<McpServer> {
        self.servers
            .iter()
            .find(|server| server.name == name)
            .cloned()
            .context("That connection is no longer in the saved list. Use /mcp to reload it.")
    }

    fn options(root: &Path) -> StdioOptions {
        StdioOptions {
            working_directory: Some(root.to_path_buf()),
            request_timeout: CHECK_TIMEOUT,
            ..Default::default()
        }
    }

    /// Start the configured program once and answer the protocol ping.
    /// Nothing else happens; no tool call is approved by a check.
    async fn check(root: PathBuf, server: McpServer) -> Result<Step> {
        let client = StdioMcpClient::new(vec![server.clone()], Self::options(&root))?;
        let answered = async {
            anyhow::ensure!(
                client.ping(&server.name).await?,
                "the server answered an invalid protocol response"
            );
            client.shutdown().await
        }
        .await;
        match answered {
            Ok(()) => Ok(Step::Checked { server }),
            Err(error) => Err(error),
        }
    }

    /// Start the server with the exact action the user confirmed and list its
    /// tools. The returned router wraps the supervised built-in router, so a
    /// request that uses it goes through the ordinary chat approval flow.
    async fn attach(root: PathBuf, pending: PendingAttach) -> Result<Step> {
        let builtins = Arc::new(DefaultToolRouter::with_policy(
            Some(root),
            ToolPolicy::supervised(),
        ));
        let names = pending
            .servers
            .iter()
            .map(|server| server.name.clone())
            .collect();
        let tools = Arc::new(
            ExternalTools::discover(
                builtins,
                pending.servers,
                pending.options,
                pending.actor,
                &pending.actions,
            )
            .await?,
        );
        let count = tools.external_descriptors().len();
        Ok(Step::Attached {
            tools,
            names,
            count,
        })
    }

    /// Abandon a running background step and staged forms. Safe when nothing
    /// is running; attached tools stay attached to the chat.
    pub fn cancel(&mut self) {
        if let Some(job) = self.job.take() {
            job.abort();
        }
        self.kind = None;
        self.staged = None;
        self.pending_attach = None;
    }

    /// The tools attached to this chat, used by the request path.
    pub fn attached(&self) -> Option<Arc<ExternalTools>> {
        self.attached.clone()
    }

    /// Stop attached external servers; used when the chat screen exits.
    pub async fn shutdown(&mut self) {
        self.cancel();
        if let Some(attached) = self.attached.take() {
            if let Err(error) = attached.shutdown().await {
                tracing::error!("could not stop an attached external server cleanly: {error:#}");
            }
        }
        self.attached_names.clear();
    }

    /// Handle one confirmed overlay choice. `Ok(Some(message))` is a success
    /// note the caller shows in chat; errors keep the overlay open.
    pub async fn action(
        &mut self,
        action: &str,
        values: &[String],
        root: &Path,
        read_only: bool,
        dialog: &mut Option<Dialog>,
    ) -> Result<Option<String>> {
        if action == "external:menu" {
            *dialog = Some(self.menu(MENU_NOTE));
        } else if action == "external:retry" {
            let path = self.path.clone();
            *dialog = Some(self.begin(path, "Loading saved external tool connections again."));
        } else if action == "external:setup" {
            *dialog = Some(Self::form(None, SETUP_NOTE));
        } else if action == "external:setup-edit" {
            let staged = self.staged.clone();
            *dialog = Some(Self::form(
                staged.as_ref(),
                "Edit the connection, then press Enter on the last field to save it.",
            ));
        } else if action == "external:save" {
            anyhow::ensure!(values.len() == 4, "The connection form is incomplete.");
            let name = values[0].trim();
            anyhow::ensure!(!name.is_empty(), "Enter a connection name.");
            let command = values[1].trim();
            anyhow::ensure!(
                !command.is_empty(),
                "Enter the program that starts the server."
            );
            let arguments = Self::split_arguments(&values[2])?;
            let env = Self::parse_env(&values[3])?;
            let server = McpServer {
                name: name.to_string(),
                transport: McpTransport::Stdio,
                command: Some(command.to_string()),
                args: arguments,
                env,
            };
            validate_server(&server)?;
            self.staged = Some(server.clone());
            *dialog = Some(Self::save_confirmation(
                self.path.as_deref(),
                &server,
                self.servers.as_slice(),
            ));
        } else if action == "external:save-confirm" {
            let server = self.staged.clone().context(
                "No connection is waiting to be saved. Use /mcp and fill the setup form.",
            )?;
            let path = self.path.clone().context(
                "No saved connection file is available; set CHIMERA_MCP_CONFIG or $HOME.",
            )?;
            save_server(&path, server.clone()).map_err(|error| {
                anyhow::anyhow!(
                    "Could not save this connection; your previous connections are preserved. {error}"
                )
            })?;
            self.staged = None;
            let saved_path = path.display().to_string();
            *dialog = Some(self.refresh(&format!(
                "Connection {} saved to {saved_path}. It is not started until you check or attach it.",
                server.name
            )));
            return Ok(Some(format!(
                "Connection {} saved to {saved_path}. Existing connections were preserved.",
                server.name
            )));
        } else if let Some(name) = action.strip_prefix("external:check:") {
            let server = self.server(name)?;
            anyhow::ensure!(
                !read_only,
                "Read-only plan mode cannot start external programs, so it cannot check this connection. Use it without read-only mode."
            );
            let root = root.to_path_buf();
            self.cancel();
            self.job = Some(tokio::spawn(async move { Self::check(root, server).await }));
            self.kind = Some(Kind::Check(name.to_string()));
            *dialog = Some(Dialog::busy(
                &format!("Checking {name}"),
                "Starting the configured program once to check it, then stopping it again.",
            ));
        } else if let Some(name) = action.strip_prefix("external:attach:") {
            let server = self.server(name)?;
            anyhow::ensure!(
                !read_only,
                "Read-only plan mode cannot attach external tools because attaching starts the server."
            );
            anyhow::ensure!(
                self.attached.is_none(),
                "External tools are already attached. Detach them before attaching different ones."
            );
            let options = Self::options(root);
            let actor = uuid::Uuid::new_v4();
            let actions = startup_actions(std::slice::from_ref(&server), &options, actor)?;
            let shown = chimera_core::approval::display_value(serde_json::to_value(&actions)?);
            let mut summary = serde_json::to_string_pretty(&shown)?;
            if summary.chars().count() > 900 {
                summary = summary.chars().take(900).collect::<String>() + "\n…";
            }
            self.pending_attach = Some(PendingAttach {
                name: server.name.clone(),
                servers: vec![server],
                options,
                actor,
                actions,
            });
            *dialog = Some(Dialog::choices(
                &format!("Attach {name} to this chat?"),
                &format!(
                    "Attaching approves this one exact server start: mcp_server_start. CHIMERA will run the program exactly as shown (private values are masked):\n{summary}\n\nEvery external tool call still asks for approval in chat with /approve or /deny."
                ),
                vec![
                    Choice::new(
                        "Attach and start the server",
                        "Its tools join the next request in this chat.",
                        format!("external:attach-confirm:{name}"),
                    ),
                    Choice::new("Back to connections", "Do not start anything.", "external:menu"),
                ],
            ));
        } else if let Some(name) = action.strip_prefix("external:attach-confirm:") {
            anyhow::ensure!(
                !read_only,
                "Read-only plan mode cannot attach external tools because attaching starts the server."
            );
            let pending = self.pending_attach.take().context(
                "No attach is waiting for confirmation. Use /mcp and check the connection first.",
            )?;
            anyhow::ensure!(
                pending.name == name,
                "That attach confirmation no longer matches the checked connection."
            );
            let root = root.to_path_buf();
            self.cancel();
            self.job = Some(tokio::spawn(
                async move { Self::attach(root, pending).await },
            ));
            self.kind = Some(Kind::Attach(name.to_string()));
            *dialog = Some(Dialog::busy(
                &format!("Attaching {name}"),
                "Starting the server and listing its tools. Tool calls will still require approval in chat.",
            ));
        } else if action == "external:detach" {
            let attached = self.attached.take();
            if let Some(attached) = attached {
                attached.shutdown().await.map_err(|error| {
                    anyhow::anyhow!("Could not stop the external servers cleanly: {error:#}")
                })?;
            }
            self.attached_names.clear();
            *dialog = Some(self.menu(
                "External tools detached; their processes were stopped and the tools left this chat.",
            ));
            return Ok(Some(
                "External tools detached; their processes were stopped.".to_string(),
            ));
        } else {
            return Err(anyhow::anyhow!(
                "That connection choice is no longer available. Use /mcp to reopen the list."
            ));
        }
        Ok(None)
    }

    fn save_confirmation(
        path: Option<&Path>,
        server: &McpServer,
        existing: &[McpServer],
    ) -> Dialog {
        let path = path
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "(no connection file available)".to_string());
        let arguments = if server.args.is_empty() {
            String::new()
        } else {
            format!(" {}", server.args.join(" "))
        };
        let replacing = existing.iter().any(|entry| entry.name == server.name);
        let replace_note = if replacing {
            format!(
                "This replaces the saved connection named {}.\n",
                server.name
            )
        } else {
            String::new()
        };
        Dialog::choices(
            "Save this connection?",
            &format!(
                "Saves to {path}.\nRuns: {}{}\n{} private environment variables (sign-in credentials) are stored owner-only and never displayed.\n{replace_note}",
                server.command.as_deref().unwrap_or("(no command)"),
                arguments,
                server.env.len()
            ),
            vec![
                Choice::new(
                    "Save connection",
                    "Appended to the saved connection file; other entries are preserved.",
                    "external:save-confirm",
                ),
                Choice::new("Edit the entry", "Back to the setup form.", "external:setup-edit"),
            ],
        )
    }

    /// Move a finished background step into the overlay.
    pub async fn poll(&mut self, dialog: &mut Option<Dialog>) {
        if !self.job.as_ref().is_some_and(|job| job.is_finished()) {
            return;
        }
        let Some(job) = self.job.take() else { return };
        let Some(kind) = self.kind.take() else { return };
        let result = job.await.unwrap_or_else(|_| {
            Err(anyhow::anyhow!(
                "The connection request stopped. Try again."
            ))
        });
        match (kind, result) {
            (Kind::List, Ok(Step::Listed(servers))) => {
                self.servers = servers;
                let note = if self.servers.is_empty() {
                    EMPTY_NOTE
                } else {
                    MENU_NOTE
                };
                *dialog = Some(self.menu(note));
            }
            (Kind::List, Err(error)) => {
                let mut menu = self.menu(&error.to_string());
                if let crate::dialog::Body::Choices { rows, .. } = &mut menu.body {
                    rows.push(Choice::new(
                        "Try again",
                        "Reload the saved connection list.",
                        "external:retry",
                    ));
                }
                *dialog = Some(menu);
            }
            (Kind::Check(name), Ok(Step::Checked { server })) => {
                self.status.insert(
                    server.name.clone(),
                    "reachable — Enter to attach it to this chat or check again".to_string(),
                );
                *dialog = Some(Dialog::choices(
                    &format!("{name} is reachable"),
                    "Attach it to this chat? Its tools are listed for the next request, and every external tool call still asks for approval in chat.",
                    vec![
                        Choice::new(
                            "Attach to this chat",
                            "Starts the server with the exact action shown next.",
                            format!("external:attach:{}", server.name),
                        ),
                        Choice::new("Back to connections", "Nothing is started.", "external:menu"),
                    ],
                ));
            }
            (Kind::Check(name), Err(error)) => {
                self.status
                    .insert(name.clone(), "failed — Enter to check again".to_string());
                *dialog = Some(self.menu(&error.to_string()));
            }
            (
                Kind::Attach(name),
                Ok(Step::Attached {
                    tools,
                    names,
                    count,
                }),
            ) => {
                self.attached = Some(tools);
                self.attached_names = names;
                for server in &self.servers {
                    if self.attached_names.contains(&server.name) {
                        self.status.insert(
                            server.name.clone(),
                            "attached — Enter to check again".to_string(),
                        );
                    }
                }
                *dialog = Some(self.menu(&format!(
                    "{name} is attached with {count} external tools. Every external tool call asks for approval in chat."
                )));
            }
            (Kind::Attach(name), Err(error)) => {
                self.status.insert(
                    name.clone(),
                    "attach failed — check again to retry".to_string(),
                );
                *dialog = Some(self.menu(&error.to_string()));
            }
            (_, Ok(_)) => {}
        }
    }
}

impl Drop for External {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialog::{Body, Outcome};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_root() -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "chimera-external-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// A minimal real MCP stdio server: initialize, ping and tools/list.
    const FIXTURE: &str = r#"
import json, sys
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    method = request['method']
    if method == 'initialize':
        result = {'protocolVersion': '2025-11-25', 'capabilities': {'tools': {}},
                  'serverInfo': {'name': 'fixture', 'version': '1'}}
    elif method == 'tools/list':
        result = {'tools': [{'name': 'echo', 'description': 'Echo the query.',
                             'inputSchema': {'type': 'object',
                                             'properties': {'query': {'type': 'string'}},
                                             'required': ['query'],
                                             'additionalProperties': False}}]}
    elif method == 'ping':
        result = {}
    else:
        print(json.dumps({'jsonrpc': '2.0', 'id': request['id'],
                          'error': {'code': -32601, 'message': 'not found'}}), flush=True)
        continue
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#;

    fn fixture_server(root: &Path, name: &str) -> McpServer {
        let script = root.join("mcp-fixture.py");
        if !script.exists() {
            std::fs::write(&script, FIXTURE).unwrap();
        }
        McpServer {
            name: name.into(),
            transport: McpTransport::Stdio,
            command: Some(
                std::env::var("CHIMERA_TEST_PYTHON").unwrap_or_else(|_| "python3".into()),
            ),
            args: vec!["-u".into(), script.to_string_lossy().into()],
            env: HashMap::from([("FIXTURE_KEY".into(), "fixture-secret-value".into())]),
        }
    }

    fn unreachable_server(name: &str) -> McpServer {
        McpServer {
            name: name.into(),
            transport: McpTransport::Stdio,
            command: Some("/nonexistent/chimera-missing-command".into()),
            args: vec![],
            env: HashMap::new(),
        }
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

    fn note(dialog: &Dialog) -> String {
        dialog.note.clone()
    }

    fn values(fields: [&str; 4]) -> Vec<String> {
        fields.iter().map(|field| field.to_string()).collect()
    }

    /// Run the overlay to its next choice view (menu, offer or confirmation).
    async fn wait_for_menu(external: &mut External, dialog: &mut Option<Dialog>) {
        for _ in 0..1000 {
            external.poll(dialog).await;
            if matches!(dialog.as_ref().map(|d| &d.body), Some(Body::Choices { .. })) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the connection request never finished");
    }

    async fn save_through_overlay(
        external: &mut External,
        dialog: &mut Option<Dialog>,
        values: [&str; 4],
    ) -> Option<String> {
        let values = values.map(str::to_string).to_vec();
        external
            .action("external:save", &values, Path::new("/tmp"), false, dialog)
            .await
            .unwrap();
        external
            .action(
                "external:save-confirm",
                &[],
                Path::new("/tmp"),
                false,
                dialog,
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn status_rows_reflect_a_configured_and_an_empty_connection_file() {
        let root = temp_root();
        std::fs::create_dir(&root).unwrap();
        let path = root.join("mcp.json");
        chimera_mcp::save_server(&path, fixture_server(&root, "fixture")).unwrap();
        let mut external = External::default();
        let mut dialog = Some(external.open(Some(path)));
        assert!(matches!(dialog.as_ref().map(|d| &d.body), Some(Body::Busy)));
        wait_for_menu(&mut external, &mut dialog).await;
        let listed = rows(dialog.as_ref().unwrap());
        let (label, detail, action) = &listed[0];
        assert_eq!(label, "fixture");
        assert_eq!(action, "external:check:fixture");
        assert!(detail.contains("configured"));
        assert!(detail.contains("credential variables"));
        assert!(
            !detail.contains("fixture-secret-value"),
            "credentials stay private"
        );
        assert!(listed.iter().any(|(_, _, a)| a == "external:setup"));
        // Searching works like every other overlay, and an unconfigured file
        // shows the helpful empty state instead of an empty list.
        let mut dialog = dialog.unwrap();
        dialog.paste("fixture");
        assert!(matches!(
            dialog.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Outcome::Action(action, _) if action == "external:check:fixture"
        ));
        let empty = temp_root();
        std::fs::create_dir(&empty).unwrap();
        let mut external = External::default();
        let mut dialog = Some(external.open(Some(empty.join("missing.json"))));
        wait_for_menu(&mut external, &mut dialog).await;
        let dialog = dialog.unwrap();
        assert!(note(&dialog).contains("No external tool connections"));
        assert!(rows(&dialog).iter().any(|(_, _, a)| a == "external:setup"));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[tokio::test]
    async fn setup_form_refuses_empty_and_invalid_input_and_changes_nothing() {
        let mut external = External::default();
        let mut dialog = Some(external.open(Some(temp_root().join("mcp.json"))));
        let error = external
            .action(
                "external:save",
                &values(["", "", "", ""]),
                Path::new("/tmp"),
                false,
                &mut dialog,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Enter a connection name"));
        let error = external
            .action(
                "external:save",
                &values(["fixture", "", "", ""]),
                Path::new("/tmp"),
                false,
                &mut dialog,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Enter the program"));
        let error = external
            .action(
                "external:save",
                &values(["fixture", "python3", "", "BROKEN"]),
                Path::new("/tmp"),
                false,
                &mut dialog,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("NAME=value"));
        let error = external
            .action(
                "external:save",
                &values(["fixture", "python3", "\"unterminated", ""]),
                Path::new("/tmp"),
                false,
                &mut dialog,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("quote is not closed"));
        assert!(external.staged.is_none(), "refused input stages nothing");
        let message = save_through_overlay(
            &mut external,
            &mut dialog,
            ["fixture", "python3", "-u script.py", "KEY=secret"],
        )
        .await;
        let message = message.expect("a valid staged connection is saved to the isolated file");
        assert!(message.contains("Connection fixture saved"));
        wait_for_menu(&mut external, &mut dialog).await;
        let dialog = dialog.as_ref().unwrap();
        assert!(external.staged.is_none());
        assert_eq!(external.servers.len(), 1, "the saved entry is listed");
        assert!(note(dialog).contains("Enter checks a connection"));
    }

    #[tokio::test]
    async fn saving_adds_an_entry_without_clobbering_existing_ones() {
        let root = temp_root();
        std::fs::create_dir(&root).unwrap();
        let path = root.join("mcp.json");
        chimera_mcp::save_server(&path, fixture_server(&root, "existing")).unwrap();
        let mut external = External::default();
        let mut dialog = Some(external.open(Some(path.clone())));
        wait_for_menu(&mut external, &mut dialog).await;
        let message = save_through_overlay(
            &mut external,
            &mut dialog,
            ["added", "deno", "task --serve", "TOKEN=a b"],
        )
        .await;
        let message = message.expect("a real save reports success in chat");
        assert!(message.contains("Connection added saved"));
        let servers = chimera_mcp::load_servers(&path).unwrap();
        let names = servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names, ["existing", "added"]);
        assert_eq!(servers[0].command.as_deref(), Some("python3"));
        assert_eq!(servers[1].env.get("TOKEN").map(String::as_str), Some("a b"));
        // Saving again with the same name replaces only that entry, in place.
        let message =
            save_through_overlay(&mut external, &mut dialog, ["existing", "bun", "", ""]).await;
        assert!(message.is_some());
        let servers = chimera_mcp::load_servers(&path).unwrap();
        let names = servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names, ["existing", "added"]);
        assert_eq!(servers[0].command.as_deref(), Some("bun"));
        assert!(servers[0].env.is_empty());
        assert_eq!(servers[1].env.get("TOKEN").map(String::as_str), Some("a b"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_unreachable_server_reports_a_real_error_not_health() {
        let root = temp_root();
        std::fs::create_dir(&root).unwrap();
        let path = root.join("mcp.json");
        chimera_mcp::save_server(&path, unreachable_server("broken")).unwrap();
        let mut external = External::default();
        let mut dialog = Some(external.open(Some(path)));
        wait_for_menu(&mut external, &mut dialog).await;
        external
            .action("external:check:broken", &[], &root, false, &mut dialog)
            .await
            .unwrap();
        for _ in 0..500 {
            external.poll(&mut dialog).await;
            let matches = matches!(dialog.as_ref().map(|d| &d.body), Some(Body::Choices { .. }));
            if matches {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let dialog = dialog.as_ref().expect("the overlay stays open on failure");
        let error_note = note(dialog);
        assert!(
            error_note.contains("could not start MCP server broken"),
            "real error surfaced: {error_note}"
        );
        assert!(!error_note.contains("reachable"));
        let rows = rows(dialog);
        let broken = rows
            .iter()
            .find(|(label, _, _)| label.contains("broken"))
            .expect("the failed connection stays listed");
        assert!(
            broken.1.contains("failed"),
            "status is honest: {}",
            broken.1
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_reachable_server_offers_attach_and_attaching_lists_real_tools() {
        let root = temp_root();
        std::fs::create_dir(&root).unwrap();
        let path = root.join("mcp.json");
        chimera_mcp::save_server(&path, fixture_server(&root, "fixture")).unwrap();
        let mut external = External::default();
        let mut dialog = Some(external.open(Some(path)));
        wait_for_menu(&mut external, &mut dialog).await;
        external
            .action("external:check:fixture", &[], &root, false, &mut dialog)
            .await
            .unwrap();
        wait_for_menu(&mut external, &mut dialog).await;
        let view = dialog.as_ref().expect("the check finishes into a choice");
        assert!(view.title.contains("reachable"));
        assert!(
            note(view).contains("Attach it to this chat?"),
            "the offer explains attachment and approvals"
        );
        assert!(rows(view)
            .iter()
            .any(|(label, _, _)| label == "Attach to this chat"));
        // The attach confirmation shows the exact startup action with the
        // credential value masked.
        external
            .action("external:attach:fixture", &[], &root, false, &mut dialog)
            .await
            .unwrap();
        let view = dialog.as_ref().unwrap();
        let confirmation = note(view);
        assert!(confirmation.contains("mcp_server_start"));
        assert!(confirmation.contains("FIXTURE_KEY"));
        assert!(
            !confirmation.contains("fixture-secret-value"),
            "the credential value never reaches the screen: {confirmation}"
        );
        external
            .action(
                "external:attach-confirm:fixture",
                &[],
                &root,
                false,
                &mut dialog,
            )
            .await
            .unwrap();
        wait_for_menu(&mut external, &mut dialog).await;
        let view = dialog.as_ref().expect("attaching lands back on the menu");
        assert!(note(view).contains("attached"));
        assert!(rows(view)
            .iter()
            .any(|(label, _, _)| label.contains("attached")));
        let attached = external.attached().expect("tools are attached to the chat");
        let names = attached
            .external_descriptors()
            .into_iter()
            .map(|descriptor| descriptor.name)
            .collect::<Vec<_>>();
        assert_eq!(names, ["mcp__fixture__echo"]);
        // A second attach is refused until the first is detached.
        external
            .action("external:check:fixture", &[], &root, false, &mut dialog)
            .await
            .unwrap();
        wait_for_menu(&mut external, &mut dialog).await;
        let error = external
            .action("external:attach:fixture", &[], &root, false, &mut dialog)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("already attached"));
        external.shutdown().await;
        assert!(external.attached().is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn read_only_mode_refuses_to_start_but_can_still_browse_and_save() {
        let root = temp_root();
        std::fs::create_dir(&root).unwrap();
        let path = root.join("mcp.json");
        chimera_mcp::save_server(&path, unreachable_server("broken")).unwrap();
        let mut external = External::default();
        let mut dialog = Some(external.open(Some(path)));
        wait_for_menu(&mut external, &mut dialog).await;
        let error = external
            .action("external:check:broken", &[], &root, true, &mut dialog)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Read-only plan mode"));
        assert!(external.job.is_none(), "a refused check starts nothing");
        let message = save_through_overlay(
            &mut external,
            &mut dialog,
            ["saved-in-readonly", "python3", "", ""],
        )
        .await;
        assert!(message.is_some(), "saving a connection is still allowed");
        let servers = chimera_mcp::load_servers(&root.join("mcp.json")).unwrap();
        assert_eq!(servers.len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_unknown_action_or_connection_is_refused_and_keeps_the_overlay() {
        let root = temp_root();
        std::fs::create_dir(&root).unwrap();
        let mut external = External::default();
        let mut dialog = Some(external.open(Some(root.join("mcp.json"))));
        wait_for_menu(&mut external, &mut dialog).await;
        let error = external
            .action("external:bogus", &[], &root, false, &mut dialog)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no longer available"));
        assert!(dialog.is_some());
        let error = external
            .action("external:check:missing", &[], &root, false, &mut dialog)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no longer in the saved list"));
        assert!(dialog.is_some());
        let error = external
            .action(
                "external:attach-confirm:fixture",
                &[],
                &root,
                false,
                &mut dialog,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("No attach is waiting"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn cancelling_clears_an_in_flight_job_and_staged_forms_but_keeps_attachment() {
        let root = temp_root();
        std::fs::create_dir(&root).unwrap();
        let path = root.join("mcp.json");
        chimera_mcp::save_server(&path, unreachable_server("broken")).unwrap();
        let mut external = External::default();
        let mut dialog = Some(external.open(Some(path)));
        wait_for_menu(&mut external, &mut dialog).await;
        external
            .action("external:check:broken", &[], &root, false, &mut dialog)
            .await
            .unwrap();
        assert!(external.job.is_some());
        external.cancel();
        assert!(external.job.is_none());
        let mut dialog = None;
        external.poll(&mut dialog).await;
        assert!(dialog.is_none());
        external
            .action(
                "external:save",
                &values(["staged", "python3", "", ""]),
                &root,
                false,
                &mut dialog,
            )
            .await
            .unwrap();
        assert!(external.staged.is_some());
        external.cancel();
        assert!(external.staged.is_none());
        assert!(external.pending_attach.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
