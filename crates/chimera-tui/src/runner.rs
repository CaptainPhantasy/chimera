//! chimera-tui::runner — The main event loop that drives the terminal.
//!
//! Enters raw mode, creates a ratatui terminal on stdout, polls keyboard
//! events, draws frames at the frame budget, and integrates with the agent
//! loop via a channel-based streaming model.

use crate::message::SystemKind;
use chimera_core::{
    agent::{AgentConfig, DoneReason},
    runtime::{RunEvent, SavedSession},
};
use chimera_tools::ToolRouter;
use std::cell::Cell;
use std::io::{self, Stdout};
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::{Arc, Once};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::chat;
use crate::input::InputBar;
use crate::message::{Message, MessageLog};
use crate::status::StatusInfo;
use crate::theme::{Palette, Styles};

// ---------------------------------------------------------------------------
// Terminal setup/teardown
// ---------------------------------------------------------------------------

pub type TuiTerminal = Terminal<CrosstermBackend<Stdout>>;

thread_local! {
    // Terminal ownership stays on the thread that enabled raw mode.
    static TERMINAL_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

fn install_terminal_panic_hook() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if TERMINAL_ACTIVE.try_with(Cell::get).unwrap_or(false) {
                let _ = restore_terminal();
            }
            previous(info);
        }));
    });
}

/// Initialize the terminal for full-screen TUI rendering.
pub fn init_terminal() -> Result<TuiTerminal> {
    install_terminal_panic_hook();
    enable_raw_mode().context("failed to enable raw mode")?;
    TERMINAL_ACTIVE.with(|active| active.set(true));
    let mut stdout = io::stdout();
    let result = (|| {
        execute!(stdout, EnterAlternateScreen, event::EnableBracketedPaste)
            .context("failed to enter alternate screen")?;
        Terminal::new(CrosstermBackend::new(stdout)).context("failed to create terminal")
    })();
    if result.is_err() {
        let _ = restore_terminal();
    }
    result
}

/// Restore the terminal to its original state.
pub fn restore_terminal() -> Result<()> {
    TERMINAL_ACTIVE.with(|active| active.set(false));
    let raw = disable_raw_mode().context("failed to disable raw mode");
    let screen = execute!(
        io::stdout(),
        event::DisableBracketedPaste,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    )
    .context("failed to leave alternate screen");
    raw?;
    screen?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Events the TUI processes.
pub enum TuiEvent {
    /// A keyboard event.
    Key(KeyEvent),
    /// A tick (frame budget elapsed).
    Tick,
    /// A message from the agent loop (streaming text, tool calls, etc.).
    AgentMessage(AgentUpdate),
    /// The agent loop finished.
    AgentDone,
    /// A user-submitted objective or command.
    Submit(String),
    DialogAction(String, Vec<String>),
    DialogClosed,
}

/// Updates pushed from the agent loop into the TUI.
#[derive(Debug, Clone)]
pub enum AgentUpdate {
    /// Incremental streaming text from the model.
    TextDelta(String),
    /// The model finished a text turn.
    TextDone,
    /// A tool call started.
    ToolCallStart { name: String, arguments: String },
    /// A tool call completed.
    ToolCallResult {
        name: String,
        result: String,
        ok: bool,
        duration_ms: u64,
    },
    /// An iteration boundary.
    Iteration { n: usize },
    /// A system status message.
    System {
        text: String,
        kind: crate::message::SystemKind,
    },
    /// Token usage update.
    Tokens { prompt: u64, completion: u64 },
}

// ---------------------------------------------------------------------------
// TUI state
// ---------------------------------------------------------------------------

/// The full interactive TUI session state.
pub struct Tui {
    pub styles: Styles,
    pub log: MessageLog,
    pub input: InputBar,
    pub status: StatusInfo,
    pub objective: String,
    pub should_quit: bool,
    pub show_help: bool,
    pub dialog: Option<crate::dialog::Dialog>,
    pub help_scroll: usize,
    /// Receiver for user-submit events.
    pub rx: mpsc::Receiver<TuiEvent>,
    /// Sender for user-submit events (cloned for input handling).
    pub tx: mpsc::Sender<TuiEvent>,
    /// Width hint for text wrapping (updated on resize).
    pub width: usize,
    /// Terminal height for computing the visible conversation rows.
    pub height: usize,
}

impl Tui {
    /// Create a new TUI session with the given objective and model info.
    pub fn new(objective: String, model: String, provider: String) -> Self {
        let (tx, rx) = mpsc::channel(16);
        Self {
            styles: Styles::dark(),
            log: MessageLog::new(100),
            input: InputBar::new(),
            status: StatusInfo {
                provider,
                model,
                mode: "RUN".into(),
                prompt_tokens: 0,
                completion_tokens: 0,
                usage_available: false,
                iteration: 0,
                max_iterations: 40,
                session_id: short_id(),
                tool_calls: 0,
                running: false,
            },
            objective: objective.clone(),
            should_quit: false,
            show_help: false,
            dialog: None,
            help_scroll: 0,
            rx,
            tx,
            width: 80,
            height: 24,
        }
    }

    /// Render a single frame.
    pub fn render(&self, f: &mut ratatui::Frame) {
        if self.show_help {
            crate::help::render_scrolled(f, &self.styles, self.help_scroll);
            return;
        }
        let mut visible_input = self.input.clone();
        visible_input.focused = self.dialog.is_none();
        chat::render(
            f,
            &self.styles,
            &self.log,
            &self.status,
            &visible_input,
            &self.objective,
            self.width,
        );
        if let Some(dialog) = &self.dialog {
            dialog.render(f, &self.styles);
        }
    }

    /// Handle a keyboard event.
    fn handle_key(&mut self, key: KeyEvent) {
        // Text and navigation may repeat; release events never perform an action.
        if key.kind == KeyEventKind::Release {
            return;
        }

        // Global keys first.
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    self.should_quit = true;
                    return;
                }
                KeyCode::Char('l') => {
                    // Toggle dark/light by comparing ember color.
                    let is_dark = self.styles.palette.ember == Palette::dark().ember;
                    self.styles = if is_dark {
                        Styles::new(Palette::light())
                    } else {
                        Styles::dark()
                    };
                    return;
                }
                _ => {}
            }
        }

        if let Some(dialog) = &mut self.dialog {
            match dialog.key(key) {
                crate::dialog::Outcome::Stay => {}
                crate::dialog::Outcome::Close => {
                    self.dialog = None;
                    let _ = self.tx.try_send(TuiEvent::DialogClosed);
                }
                crate::dialog::Outcome::Action(action, values) => {
                    let _ = self.tx.try_send(TuiEvent::DialogAction(action, values));
                }
            }
            return;
        }
        // Help overlay.
        if self.show_help {
            match key.code {
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => {
                    self.show_help = false;
                }
                KeyCode::Up => self.help_scroll = self.help_scroll.saturating_sub(1),
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(10),
                KeyCode::Down | KeyCode::PageDown => {
                    let step = if key.code == KeyCode::Down { 1 } else { 10 };
                    let area =
                        ratatui::layout::Rect::new(0, 0, self.width as u16, self.height as u16);
                    let limit = crate::help::scroll_limit(area, &self.styles);
                    self.help_scroll = self.help_scroll.saturating_add(step).min(limit);
                }
                _ => {}
            }
            return;
        }

        // Input bar handling.
        match key.code {
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                self.input.insert_char('\n');
            }
            KeyCode::Enter => {
                let text = self.input.submit();
                if !text.trim().is_empty() {
                    let _ = self.tx.try_send(TuiEvent::Submit(text));
                }
            }
            KeyCode::F(2) => {
                let _ = self.tx.try_send(TuiEvent::Submit("/connect".into()));
            }
            KeyCode::F(1) => {
                self.show_help = true;
                self.help_scroll = 0;
            }
            KeyCode::Tab => {
                if let Some(Message::ToolCall { expanded, .. }) = self
                    .log
                    .messages
                    .iter_mut()
                    .rev()
                    .find(|m| matches!(m, Message::ToolCall { .. }))
                {
                    *expanded = !*expanded;
                }
            }
            KeyCode::Up => {
                self.input.history_prev();
            }
            KeyCode::Down => {
                self.input.history_next();
            }
            KeyCode::PageUp => {
                let limit = self.scroll_limit();
                for _ in 0..10 {
                    self.log.scroll_up(limit);
                }
            }
            KeyCode::PageDown => {
                self.log.scroll_offset = self.log.scroll_offset.min(self.scroll_limit());
                for _ in 0..10 {
                    self.log.scroll_down();
                }
            }
            KeyCode::Backspace => {
                self.input.backspace();
            }
            KeyCode::Delete => {
                self.input.delete_char();
            }
            KeyCode::Left => {
                if key.modifiers.contains(KeyModifiers::ALT) {
                    self.input.cursor_word_left();
                } else {
                    self.input.cursor_left();
                }
            }
            KeyCode::Right => {
                if key.modifiers.contains(KeyModifiers::ALT) {
                    self.input.cursor_word_right();
                } else {
                    self.input.cursor_right();
                }
            }
            KeyCode::Home => {
                self.input.cursor_home();
            }
            KeyCode::End => {
                self.input.cursor_end();
            }
            KeyCode::Char(' ') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Clear screen
                self.log = MessageLog::new(500);
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.insert_char(c);
            }
            _ => {}
        }
    }

    fn scroll_limit(&self) -> usize {
        let area = chat::message_area(
            ratatui::layout::Rect::new(0, 0, self.width as u16, self.height as u16),
            !self.input.suggestions.is_empty(),
        );
        self.log
            .to_lines(&self.styles, area.width as usize)
            .len()
            .saturating_sub(area.height as usize)
    }

    /// Apply an agent update to the message log.
    pub fn apply_agent_update(&mut self, update: AgentUpdate) {
        self.status.running = true;
        match update {
            AgentUpdate::TextDelta(text) => {
                // Append to the last assistant message if streaming, else create one.
                if let Some(Message::Assistant {
                    text: existing,
                    streaming: true,
                }) = self.log.last_mut()
                {
                    existing.push_str(&text);
                } else {
                    self.log.push(Message::Assistant {
                        text,
                        streaming: true,
                    });
                }
            }
            AgentUpdate::TextDone => {
                if let Some(Message::Assistant { streaming, .. }) = self.log.last_mut() {
                    *streaming = false;
                }
            }
            AgentUpdate::ToolCallStart { name, arguments } => {
                self.status.tool_calls += 1;
                self.log.push(Message::ToolCall {
                    id: format!("tc-{}", self.status.tool_calls),
                    name,
                    arguments,
                    result: None,
                    ok: None,
                    duration_ms: None,
                    expanded: false,
                });
            }
            AgentUpdate::ToolCallResult {
                name,
                result,
                ok,
                duration_ms,
            } => {
                // Approval and decision notices can follow the tool card.
                // Match the latest unfinished call, leaving completed cards intact.
                if let Some(Message::ToolCall { name: tn, result: r, ok: o, duration_ms: d, .. }) =
                    self.log.messages.iter_mut().rev().find(|message|matches!(message,Message::ToolCall{name:tn,result:None,..} if *tn==name))
                {
                    if *tn == name {
                        *r = Some(result);
                        *o = Some(ok);
                        *d = Some(duration_ms);
                    }
                }
            }
            AgentUpdate::Iteration { n } => {
                self.status.iteration = n;
                self.log.push(Message::Divider {
                    label: format!("Iteration {n}"),
                });
            }
            AgentUpdate::System { text, kind } => {
                self.log.push(Message::System { text, kind });
            }
            AgentUpdate::Tokens { prompt, completion } => {
                self.status.prompt_tokens = prompt;
                self.status.completion_tokens = completion;
            }
        }
    }
}

/// Generate a short session ID string.
fn short_id() -> String {
    let id = uuid::Uuid::new_v4().to_string();
    id[..8].to_string()
}

// ---------------------------------------------------------------------------
// Main loop
// ---------------------------------------------------------------------------

/// Start a saved conversation with the configured model.
pub async fn run_interactive(initial_objective: Option<String>) -> Result<()> {
    run_saved_interactive(initial_objective, None, None).await
}

pub async fn run_interactive_with_config(
    initial: Option<String>,
    model: String,
    provider: String,
) -> Result<()> {
    run_screen(initial, None, None, Some((model, provider))).await
}

pub async fn run_saved_interactive(
    initial: Option<String>,
    resume: Option<uuid::Uuid>,
    profile: Option<String>,
) -> Result<()> {
    run_screen(initial, resume, profile, None).await
}

/// A failed read must never make default settings eligible to run a request.
struct ScreenSettings {
    loaded: bool,
    overrides: Option<(String, String)>,
}

impl ScreenSettings {
    fn new(overrides: Option<(String, String)>) -> Self {
        Self {
            loaded: false,
            overrides,
        }
    }

    fn load(&mut self) -> Result<chimera_config::Config> {
        self.accept(chimera_config::Config::load())
    }

    fn accept(&mut self, result: Result<chimera_config::Config>) -> Result<chimera_config::Config> {
        self.loaded = false;
        // Parsing errors may contain private file values. Keep details out of chat.
        let mut config =
            result.map_err(|_| anyhow::anyhow!("Model settings could not be read."))?;
        if let Some((model, provider)) = &self.overrides {
            config.provider.model = model.clone();
            config.provider.name = provider.clone();
        }
        config
            .validate_credentials()
            .map_err(|_| anyhow::anyhow!("Model setup needed."))?;
        self.loaded = true;
        Ok(config)
    }

    fn ready(&self, config: &chimera_config::Config) -> bool {
        self.loaded && config.validate_credentials().is_ok()
    }
}

fn show_setup_needed(tui: &mut Tui) {
    tui.status.running = false;
    tui.status.mode = "SETUP".into();
    tui.log.push(Message::System {
        text: "Model setup needed. Use /connect to choose a provider and enter its key. Your request has not been sent.".into(),
        kind: SystemKind::Warning,
    });
}

/// Report a saved default model, naming any override this process keeps using.
fn model_saved_message(config: &chimera_config::Config, provider: &str, chosen: &str) -> String {
    if config.provider.model == chosen {
        format!(
            "{chosen} is saved as your default model for {provider}. Your draft is ready to send."
        )
    } else {
        format!("{chosen} is saved as your default model for {provider}, but this session keeps using {} until CHIMERA is restarted.", config.provider.model)
    }
}

async fn run_screen(
    initial: Option<String>,
    resume: Option<uuid::Uuid>,
    profile: Option<String>,
    overrides: Option<(String, String)>,
) -> Result<()> {
    let mut settings = ScreenSettings::new(overrides);
    let mut config = settings.load().unwrap_or_default();
    anyhow::ensure!(
        matches!(
            profile.as_deref(),
            None | Some("supervised") | Some("read-only")
        ),
        "unknown policy; choose supervised or read-only"
    );
    let root = std::env::current_dir()?;
    let session = match resume {
        Some(id) => SavedSession::resume(root.clone(), id).await?,
        None => {
            SavedSession::create(
                root.clone(),
                initial.as_deref().unwrap_or("Interactive conversation"),
            )
            .await?
        }
    };
    let mut tui = Tui::new(
        initial.clone().unwrap_or_default(),
        config.provider.model.clone(),
        config.provider.name.clone(),
    );
    tui.status.session_id = session.id.to_string();
    tui.status.max_iterations = config.agent.max_iterations;
    for message in &session.messages {
        match message.role {
            chimera_llm::Role::User => tui.log.push(Message::User {
                text: message.content.clone().unwrap_or_default(),
            }),
            chimera_llm::Role::Assistant => tui.log.push(Message::Assistant {
                text: message.content.clone().unwrap_or_default(),
                streaming: false,
            }),
            _ => {}
        }
    }
    if !settings.ready(&config) {
        show_setup_needed(&mut tui);
    }
    let mut session = Some(session);
    let mut active: Option<
        tokio::task::JoinHandle<(SavedSession, Result<chimera_core::agent::AgentRunResult>)>,
    > = None;
    let (cancel, _) = tokio::sync::watch::channel(false);
    let (events, mut updates) = chimera_core::runtime::event_channel();
    let mut pending: Option<(uuid::Uuid, tokio::sync::oneshot::Sender<bool>)> = None;
    let mut connections = crate::connect::Connections::default();
    let mut models = crate::models::Models::default();
    let mut sessions = crate::sessions::Sessions::default();
    let mut memory = crate::memory::Memory::default();
    let restore = TerminalGuard::new();
    let mut terminal = init_terminal()?;
    if let Some(objective) = initial.filter(|s| !s.trim().is_empty()) {
        // Setup commands must not erase a request supplied on the command line.
        tui.input.history.push_back(objective.clone());
        tui.tx.try_send(TuiEvent::Submit(objective))?;
    }
    let result: Result<()> = async {
        loop {
            connections.poll(&mut tui.dialog).await;
            models.poll(&mut tui.dialog).await;
            sessions.poll(&mut tui.dialog).await;
            memory.poll(&mut tui.dialog).await;
            // A busy stream must still leave time for keys and drawing.
            for _ in 0..128 {
                let Ok(update) = updates.try_recv() else { break };
                match update {
                    RunEvent::Approval { id, call, answer } => {
                        let shown=chimera_core::approval::display_value(call.args);
                        tui.log.push(Message::System { text: format!("Approval {id}: {} {}. Use /approve or /deny. Expires in five minutes.", call.tool, shown), kind: SystemKind::Warning });
                        pending = Some((id, answer));
                    }
                    RunEvent::Update { kind, value } => {
                        match kind.as_str() {
                            "text" => tui.apply_agent_update(AgentUpdate::TextDelta(value.as_str().unwrap_or("").into())),
                            "tool_call" => {
                                tui.apply_agent_update(AgentUpdate::TextDone);
                                tui.apply_agent_update(AgentUpdate::ToolCallStart {
                                    name: value["name"].as_str().unwrap_or("tool").into(), arguments: value["arguments"].to_string(),
                                });
                            }
                            "tool_result" => tui.apply_agent_update(AgentUpdate::ToolCallResult {
                                name: value["name"].as_str().unwrap_or("tool").into(), result: value["output"].to_string(),
                                ok: value["ok"].as_bool().unwrap_or(false), duration_ms: value["duration_ms"].as_u64().unwrap_or(0),
                            }),
                            "text_done" => tui.apply_agent_update(AgentUpdate::TextDone),
                            "tokens" => {
                                tui.status.prompt_tokens = value["prompt"].as_u64().unwrap_or(0);
                                tui.status.completion_tokens = value["completion"].as_u64().unwrap_or(0);
                                tui.status.usage_available = value["available"].as_bool().unwrap_or(false);
                            }
                            "iteration" => tui.status.iteration = value.as_u64().or_else(|| value["n"].as_u64()).unwrap_or(0) as usize,
                            _ => {},
                        }
                    }
                }
            }
            if active.as_ref().is_some_and(|task| task.is_finished()) && updates.is_empty() {
                let (saved, result) = active.take().context("finished agent task is missing")?.await.context("agent task stopped unexpectedly")?;
                session = Some(saved);
                pending = None;
                tui.apply_agent_update(AgentUpdate::TextDone);
                tui.status.running = false;
                let (text, kind) = match result {
                    Ok(result) => {
                        tui.status.iteration = result.iterations;
                        tui.status.prompt_tokens = result.total_prompt_tokens;
                        tui.status.completion_tokens = result.total_completion_tokens;
                        tui.status.usage_available = result.usage_available;
                        match result.done_reason {
                            DoneReason::FinalAnswer => ("Answer saved.".into(), SystemKind::Success),
                            DoneReason::MaxIterations => ("Stopped at the work limit. Progress saved; the request is unfinished.".into(), SystemKind::Warning),
                            DoneReason::Error => (result.error.unwrap_or_else(|| "Run failed. Progress saved.".into()), SystemKind::Error),
                        }
                    }
                    Err(error) => (format!("{error:#}"), SystemKind::Error),
                };
                tui.log.push(Message::System { text, kind });
            }
            while let Ok(input) = tui.rx.try_recv() {
                let text = match input {
                    TuiEvent::DialogClosed => { connections.cancel(); models.cancel(); sessions.cancel(); memory.cancel(); continue; },
                    TuiEvent::DialogAction(action, _values) if action.starts_with("models:") => {
                        match models.action(&action, &config, &mut tui.dialog).await {
                            Ok(Some(saved)) => {
                                let chosen = saved.provider.model.clone();
                                let provider = saved.provider.name.clone();
                                match settings.load() {
                                    Ok(loaded) => {
                                        config = loaded;
                                        tui.status.model = config.provider.model.clone();
                                        tui.status.provider = config.provider.name.clone();
                                        tui.status.mode = if profile.as_deref() == Some("read-only") { "PLAN" } else { "RUN" }.into();
                                        tui.log.push(Message::System { text: model_saved_message(&config, &provider, &chosen), kind: SystemKind::Success });
                                    }
                                    Err(_) => tui.log.push(Message::System { text: "The model choice was saved, but settings could not be read back. Fix the settings file, then use /reload.".into(), kind: SystemKind::Warning }),
                                }
                            },
                            Ok(None) => {},
                            Err(error) => if let Some(dialog) = &mut tui.dialog { dialog.note = error.to_string(); }
                        }
                        continue;
                    },
                    TuiEvent::DialogAction(action, _values) if action.starts_with("sessions:") => {
                        match sessions.action(&action, &mut tui.dialog).await {
                            Ok(Some(resumed)) => {
                                tui.log = MessageLog::new(100);
                                for message in &resumed.messages {
                                    match message.role {
                                        chimera_llm::Role::User => tui.log.push(Message::User {
                                            text: message.content.clone().unwrap_or_default(),
                                        }),
                                        chimera_llm::Role::Assistant => tui.log.push(Message::Assistant {
                                            text: message.content.clone().unwrap_or_default(),
                                            streaming: false,
                                        }),
                                        _ => {}
                                    }
                                }
                                tui.status.session_id = resumed.id.to_string();
                                session = Some(resumed);
                                tui.log.push(Message::System { text: "Conversation opened in this screen. Your unsent text is ready to send.".into(), kind: SystemKind::Success });
                            },
                            Ok(None) => {},
                            Err(error) => if let Some(dialog) = &mut tui.dialog { dialog.note = error.to_string(); }
                        }
                        continue;
                    },
                    TuiEvent::DialogAction(action, values) if action.starts_with("memory:") => {
                        match memory.action(&action, &values, &mut tui.dialog).await {
                            Ok(Some(message)) => {
                                tui.log.push(Message::System { text: message, kind: SystemKind::Success });
                            },
                            Ok(None) => {},
                            Err(error) => if let Some(dialog) = &mut tui.dialog { dialog.note = error.to_string(); }
                        }
                        continue;
                    },
                    TuiEvent::DialogAction(action, values) => {
                        match connections.action(&action, &values, &config, &mut tui.dialog).await {
                            Ok(Some(saved)) => {
                                config = settings.accept(Ok(saved))?;
                                tui.status.model = config.provider.model.clone();
                                tui.status.provider = config.provider.name.clone();
                                tui.status.mode = if profile.as_deref() == Some("read-only") { "PLAN" } else { "RUN" }.into();
                                tui.log.push(Message::System { text: format!("Connected to {}. {} is saved as your default model. Your draft is ready to send.", config.provider.name, config.provider.model), kind: SystemKind::Success });
                            },
                            Ok(None) => {},
                            Err(error) => if let Some(dialog) = &mut tui.dialog { dialog.note = error.to_string(); }
                        }
                        continue;
                    },
                    TuiEvent::Submit(text) => text,
                    _ => continue,
                };
                let unsent = text.clone();
                let trimmed = text.trim();
                let (command, argument) = trimmed.split_once(' ').unwrap_or((trimmed, ""));
                let mut plan = profile.as_deref() == Some("read-only");
                let objective = match command {
                    "/quit" | "/exit" => { tui.should_quit = true; break; }
                    "/clear" => { tui.log = MessageLog::new(100); continue; }
                    "/help" | "/?" => { tui.show_help = true; tui.help_scroll = 0; continue; }
                    "/theme" => { tui.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL)); continue; }
                    "/cancel" => { let _ = cancel.send(true); continue; }
                    "/approve" | "/deny" => {
                        if let Some((id, answer)) = pending.take() {
                            let allowed = command == "/approve";
                            let delivered = answer.send(allowed).is_ok();
                            tui.log.push(Message::System { text: format!("Decision for {id}: {}", if delivered { if allowed {"approved once"} else {"denied"} } else {"expired"}), kind: SystemKind::Info });
                        } else { tui.log.push(Message::System { text: "No action is waiting for approval.".into(), kind: SystemKind::Warning }); }
                        continue;
                    }
                    "/setup" | "/connect" => {
                        if active.is_some() { tui.log.push(Message::System { text: "Finish or cancel the current request before connecting a provider.".into(), kind: SystemKind::Warning }); }
                        else { tui.dialog = Some(connections.open(&config)); }
                        continue;
                    }
                    "/reload" => {
                        if active.is_some() {
                            tui.log.push(Message::System { text: "Cancel or finish the current request before reloading settings.".into(), kind: SystemKind::Warning });
                        } else {
                            match settings.load() {
                                Ok(loaded) => {
                                    config = loaded;
                                    tui.status.model = config.provider.model.clone();
                                    tui.status.provider = config.provider.name.clone();
                                    tui.status.max_iterations = config.agent.max_iterations;
                                    tui.status.mode = if profile.as_deref() == Some("read-only") { "PLAN" } else { "RUN" }.into();
                                    tui.log.push(Message::System { text: "Model settings reloaded. Send your request when ready.".into(), kind: SystemKind::Success });
                                }
                                Err(_) => {
                                    tui.log.push(Message::System { text: "Could not reload model settings. Fix the settings file, then use /reload.".into(), kind: SystemKind::Warning });
                                    show_setup_needed(&mut tui);
                                }
                            }
                        }
                        continue;
                    }
                    "/model" => {
                        if argument.is_empty() { tui.log.push(Message::System { text: format!("Current model: {}. Use /models to list this service's models and save a default.", config.provider.model), kind: SystemKind::Info }); }
                        else if active.is_some() { tui.log.push(Message::System { text: "Cancel or finish the current request before changing the model.".into(), kind: SystemKind::Warning }); }
                        else if !settings.ready(&config) { show_setup_needed(&mut tui); }
                        else {
                            let mut changed = config.clone();
                            changed.provider.model = argument.trim().into();
                            if changed.validate().is_ok() {
                                config = changed;
                                tui.status.model = config.provider.model.clone();
                                tui.log.push(Message::System { text: format!("{} is used for this chat only. Use /models to choose and save a default.", config.provider.model), kind: SystemKind::Info });
                            } else { tui.log.push(Message::System { text: "That model setting is not valid. The previous model is unchanged.".into(), kind: SystemKind::Warning }); }
                        }
                        continue;
                    }
                    "/models" => {
                        if active.is_some() { tui.log.push(Message::System { text: "Cancel or finish the current request before changing the model.".into(), kind: SystemKind::Warning }); }
                        else if !settings.ready(&config) { show_setup_needed(&mut tui); }
                        else { tui.dialog = Some(models.open(&config)); }
                        continue;
                    }
                    "/tools" => {
                        let tools = chimera_tools::DefaultToolRouter::new(Some(root.clone())).list().await?;
                        tui.log.push(Message::System { text: tools.iter().map(|t| format!("{}: {}", t.name, t.description)).collect::<Vec<_>>().join("\n"), kind: SystemKind::Info });
                        continue;
                    }
                    "/sessions" => {
                        if active.is_some() { tui.log.push(Message::System { text: "Cancel or finish the current request before switching conversations.".into(), kind: SystemKind::Warning }); }
                        else { tui.dialog = Some(sessions.open(root.clone())); }
                        continue;
                    }
                    "/memory" => {
                        if active.is_some() { tui.log.push(Message::System { text: "Cancel or finish the current request before browsing saved facts.".into(), kind: SystemKind::Warning }); }
                        else { tui.dialog = Some(memory.open(root.clone())); }
                        continue;
                    }
                    "/remember" => {
                        if active.is_some() { tui.log.push(Message::System { text: "Cancel or finish the current request before saving a fact.".into(), kind: SystemKind::Warning }); }
                        else { tui.dialog = Some(memory.remember_form(root.clone(), argument)); }
                        continue;
                    }
                    "/run" => argument.to_string(),
                    "/plan" => { plan = true; argument.to_string() }
                    _ if command.starts_with('/') => { tui.log.push(Message::System { text: format!("Unknown command {command}. Use /help."), kind: SystemKind::Warning }); continue; }
                    _ => text,
                };
                if objective.trim().is_empty() { continue; }
                if active.is_some() {
                    tui.input.buffer = objective.clone(); tui.input.cursor = objective.len();
                    tui.log.push(Message::System { text: "A request is already running. Finish it or use /cancel; your new text remains in the input.".into(), kind: SystemKind::Warning });
                    continue;
                }
                if !settings.ready(&config) {
                    tui.input.buffer = unsent;
                    tui.input.cursor = tui.input.buffer.len();
                    show_setup_needed(&mut tui);
                    continue;
                }
                tui.objective = objective.clone();
                tui.status.mode = if plan { "PLAN" } else { "RUN" }.into();
                tui.log.push(Message::User { text: objective.clone() });
                tui.status.running = true;
                cancel.send_replace(false);
                let cancellation = cancel.subscribe();
                let provider = config.build_provider();
                let model = config.provider.model.clone();
                let agent_config = AgentConfig { max_iterations: config.agent.max_iterations, temperature: Some(config.provider.temperature), max_tokens: Some(config.provider.max_tokens), ..Default::default() };
                let policy = if plan { chimera_tools::ToolPolicy::read_only() } else { chimera_tools::ToolPolicy::supervised() };
                let tools = Arc::new(chimera_tools::DefaultToolRouter::with_policy(Some(root.clone()), policy));
                let events = events.clone();
                let mut saved = session.take().context("saved conversation unavailable")?;
                active = Some(tokio::spawn(async move {
                    let result = saved.run(&provider, tools, &model, &objective, &agent_config, chimera_core::runtime::RunChannels { events, cancellation }).await;
                    (saved, result)
                }));
            }
            if tui.should_quit { break; }
            if let Ok(size) = terminal.size() {
                tui.width = size.width as usize;
                tui.height = size.height as usize;
                if tui.show_help {
                    tui.help_scroll = tui.help_scroll.min(crate::help::scroll_limit(
                        ratatui::layout::Rect::new(0, 0, size.width, size.height), &tui.styles,
                    ));
                }
            }
            terminal.draw(|frame| tui.render(frame))?;
            if event::poll(Duration::from_millis(1))? {
                match event::read()? {
                    Event::Key(key) => tui.handle_key(key),
                    Event::Paste(text) => if let Some(dialog) = &mut tui.dialog { dialog.paste(&text); } else { for character in text.chars() { tui.input.insert_char(character); } },
                    _ => {},
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    }.await;
    cancel.send_replace(true);
    if let Some(task) = active {
        let _ = task.await.context("could not stop the running request")?;
    }
    drop(restore);
    result
}

/// Restores an active terminal when its owning scope exits, including on error.
/// Create this before `init_terminal`. It must remain on the terminal's thread.
#[derive(Default)]
pub struct TerminalGuard {
    _owner_thread: PhantomData<Rc<()>>,
}

impl TerminalGuard {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if TERMINAL_ACTIVE.try_with(Cell::get).unwrap_or(false) {
            let _ = restore_terminal();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_settings_never_allow_default_or_previous_config_to_run() {
        let mut state = ScreenSettings::new(None);
        let mut local = chimera_config::Config::default();
        local.provider.base_url = "http://127.0.0.1:12434/v1".into();
        let valid = state.accept(Ok(local.clone())).unwrap();
        assert!(state.ready(&valid));
        let error = state
            .accept(Err(anyhow::anyhow!("secret-sentinel")))
            .unwrap_err();
        assert!(!error.to_string().contains("secret-sentinel"));
        assert!(!state.ready(&valid));
        assert!(state.accept(Ok(chimera_config::Config::default())).is_err());
        assert!(!state.ready(&valid));
        let recovered = state.accept(Ok(local)).unwrap();
        assert!(state.ready(&recovered));
    }

    #[test]
    fn explicit_model_and_provider_survive_settings_reload() {
        let mut state =
            ScreenSettings::new(Some(("chosen-model".into(), "chosen-provider".into())));
        let mut local = chimera_config::Config::default();
        local.provider.base_url = "http://127.0.0.1:12434/v1".into();
        for _ in 0..2 {
            let config = state.accept(Ok(local.clone())).unwrap();
            assert_eq!(config.provider.model, "chosen-model");
            assert_eq!(config.provider.name, "chosen-provider");
            assert!(state.ready(&config));
        }
    }

    #[test]
    fn help_scroll_preserves_input_and_normal_q_is_text() {
        let mut tui = Tui::new(String::new(), "test".into(), "test".into());
        tui.width = 40;
        tui.height = 15;
        tui.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        tui.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        for _ in 0..20 {
            tui.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        }
        assert!(tui.help_scroll > 0);
        tui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!tui.show_help);
        assert!(!tui.should_quit);
        assert_eq!(tui.input.text(), "q");
        tui.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        assert_eq!(tui.help_scroll, 0);
    }

    #[test]
    fn tui_initial_state() {
        let tui = Tui::new("test objective".into(), "gpt-4o".into(), "openai".into());
        assert_eq!(tui.objective, "test objective");
        assert_eq!(tui.status.model, "gpt-4o");
        assert!(!tui.should_quit);
        assert!(!tui.show_help);
    }

    #[test]
    fn short_id_is_8_chars() {
        let id = short_id();
        assert_eq!(id.len(), 8);
    }

    #[test]
    fn normal_typing_paste_history_and_tool_controls() {
        let mut tui = Tui::new(
            "edit a file".into(),
            "configured-model".into(),
            "provider".into(),
        );
        for c in "why? café 🦋".chars() {
            tui.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(tui.input.text(), "why? café 🦋");
        assert!(!tui.show_help);
        tui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        assert_eq!(tui.input.text(), "why? café 🦋\n");
        tui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            matches!(tui.rx.try_recv().unwrap(),TuiEvent::Submit(text) if text=="why? café 🦋\n")
        );
        tui.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(tui.input.text(), "why? café 🦋\n");
        tui.apply_agent_update(AgentUpdate::ToolCallStart {
            name: "read_file".into(),
            arguments: "{}".into(),
        });
        tui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(matches!(
            tui.log.messages.back(),
            Some(Message::ToolCall { expanded: true, .. })
        ));
    }

    #[test]
    fn approved_tool_result_survives_intervening_status_messages() {
        let mut tui = Tui::new("edit".into(), "test".into(), "test".into());
        tui.apply_agent_update(AgentUpdate::ToolCallStart {
            name: "write_file".into(),
            arguments: "{\"path\":\"sample.txt\"}".into(),
        });
        tui.log.push(Message::System {
            text: "Review this action".into(),
            kind: SystemKind::Warning,
        });
        tui.log.push(Message::System {
            text: "Approved once".into(),
            kind: SystemKind::Info,
        });
        tui.apply_agent_update(AgentUpdate::ToolCallResult {
            name: "write_file".into(),
            result: "{\"bytes\":19,\"path\":\"sample.txt\"}".into(),
            ok: true,
            duration_ms: 7,
        });
        tui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        let Some(Message::ToolCall {
            result,
            ok,
            duration_ms,
            expanded,
            ..
        }) = tui.log.messages.front()
        else {
            panic!("tool card missing")
        };
        assert_eq!(
            result.as_deref(),
            Some("{\"bytes\":19,\"path\":\"sample.txt\"}")
        );
        assert_eq!(*ok, Some(true));
        assert_eq!(*duration_ms, Some(7));
        assert!(*expanded);
    }

    #[test]
    fn a_long_single_message_can_scroll_to_its_beginning_and_back() {
        use ratatui::backend::TestBackend;
        let mut tui = Tui::new("long answer".into(), "test".into(), "test".into());
        let text = (1..=60)
            .map(|n| format!("row {n:03} café 世界 🦋"))
            .collect::<Vec<_>>()
            .join("\n");
        tui.log.push(Message::Assistant {
            text,
            streaming: false,
        });
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let screen = |terminal: &Terminal<TestBackend>| {
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        };
        terminal.draw(|frame| tui.render(frame)).unwrap();
        assert!(screen(&terminal).contains("row 060"));
        for _ in 0..20 {
            tui.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        }
        terminal.draw(|frame| tui.render(frame)).unwrap();
        assert!(screen(&terminal).contains("row 001"));
        for _ in 0..20 {
            tui.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        }
        terminal.draw(|frame| tui.render(frame)).unwrap();
        assert!(screen(&terminal).contains("row 060"));
    }

    #[test]
    fn long_international_input_fits_all_supported_terminal_sizes() {
        use ratatui::backend::TestBackend;
        let mut tui = Tui::new(
            "Edit café 🦋".repeat(30),
            "test-model".into(),
            "provider".into(),
        );
        for c in "café 世界 🦋".repeat(200).chars() {
            tui.input.insert_char(c);
        }
        for (width, height) in [(80, 24), (100, 30), (160, 45)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            for styles in [Styles::dark(), Styles::new(Palette::light())] {
                tui.styles = styles;
                terminal.draw(|frame| tui.render(frame)).unwrap();
                let cursor = terminal.get_cursor_position().unwrap();
                assert!(cursor.x < width && cursor.y < height);
                tui.show_help = true;
                terminal.draw(|frame| tui.render(frame)).unwrap();
                tui.show_help = false;
            }
        }
    }

    #[test]
    fn both_themes_paint_chat_and_help_backgrounds() {
        use ratatui::backend::TestBackend;
        let mut tui = Tui::new("theme check".into(), "test".into(), "test".into());
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        for palette in [Palette::dark(), Palette::light()] {
            tui.styles = Styles::new(palette);
            for help in [false, true] {
                tui.show_help = help;
                terminal.draw(|frame| tui.render(frame)).unwrap();
                for position in [(79, 0), (79, 10), (2, 20)] {
                    assert_eq!(
                        terminal.backend().buffer()[position].bg,
                        palette.bg,
                        "background at {position:?}, help={help}"
                    );
                }
            }
        }
    }

    #[test]
    fn saved_model_message_names_an_override_that_still_wins() {
        let config = chimera_config::Config {
            provider: chimera_config::ProviderConfig {
                model: "pinned".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(model_saved_message(&config, "Local", "pinned")
            .contains("pinned is saved as your default model for Local"));
        let message = model_saved_message(&config, "Local", "chosen");
        assert!(message.contains("chosen is saved as your default model for Local"));
        assert!(message.contains("keeps using pinned until CHIMERA is restarted"));
    }
}
