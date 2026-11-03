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
use std::collections::VecDeque;
use std::io::{self, Stdout};
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::{Arc, Once};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
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
        execute!(
            stdout,
            EnterAlternateScreen,
            event::EnableBracketedPaste,
            event::EnableMouseCapture
        )
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
        event::DisableMouseCapture,
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
    Queued(String, bool),
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
    pub plan_mode: bool,
    pub read_only: bool,
    queued: VecDeque<(String, bool)>,
    queue_paused: bool,
    history_view: Option<MessageLog>,
    history_page: usize,
    tool_views: Vec<Message>,
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
                run_started: None,
                active_tool: None,
                project: String::new(),
                branch: String::new(),
                queued: 0,
                context_tokens: 0,
                context_window: None,
            },
            objective: objective.clone(),
            should_quit: false,
            plan_mode: false,
            read_only: false,
            queued: VecDeque::new(),
            queue_paused: false,
            history_view: None,
            history_page: 0,
            tool_views: Vec::new(),
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
            self.history_view.as_ref().unwrap_or(&self.log),
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
                    self.queue_paused = true;
                    let _ = self.tx.try_send(TuiEvent::Submit("/cancel".into()));
                    return;
                }
                KeyCode::Char('q') => {
                    self.should_quit = true;
                    return;
                }
                KeyCode::Char('f') if self.dialog.is_none() && !self.show_help => {
                    self.search_form();
                    return;
                }
                KeyCode::Char('l') => {
                    // Toggle dark/light and remember the choice for next launch.
                    let is_dark = self.styles.palette.ember == Palette::dark().ember;
                    let theme = if is_dark { "light" } else { "dark" };
                    self.styles = if is_dark {
                        Styles::new(Palette::light())
                    } else {
                        Styles::dark()
                    };
                    if let Err(error) = chimera_config::set_theme(theme) {
                        self.log.push(Message::System {
                            text: format!("Theme applied, but it could not be saved: {error}"),
                            kind: SystemKind::Warning,
                        });
                    }
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

        if self.input.handle_edit_key(key) {
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
                // Enter with a highlighted suggestion completes it instead of
                // submitting, so arrow-selected commands never fire by surprise.
                if self.input.suggestion_sel.is_some() && !self.input.suggestions.is_empty() {
                    self.input.accept_suggestion();
                } else {
                    let text = self.input.submit();
                    if !text.trim().is_empty() {
                        let _ = self.tx.try_send(TuiEvent::Submit(text));
                    }
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
                if !self.input.suggestions.is_empty() {
                    // Tab completes the highlighted (or first) slash command.
                    self.input.accept_suggestion();
                } else if let Some(Message::ToolCall { expanded, .. }) = self
                    .log
                    .messages
                    .iter_mut()
                    .rev()
                    .find(|m| matches!(m, Message::ToolCall { .. }))
                {
                    *expanded = !*expanded;
                }
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
                let limit = self.scroll_limit();
                self.view_log_mut().scroll_up(limit);
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                let limit = self.scroll_limit();
                self.view_log_mut().scroll_offset = self.view_log_mut().scroll_offset.min(limit);
                self.view_log_mut().scroll_down();
            }
            KeyCode::BackTab => {
                // Shift+Tab folds every tool card at once.
                for message in &mut self.log.messages {
                    if let Message::ToolCall { expanded, .. } = message {
                        *expanded = false;
                    }
                }
            }
            KeyCode::Up => {
                if key.modifiers.contains(KeyModifiers::ALT) {
                    self.input.history_prev();
                } else if self.input.suggestions.is_empty() {
                    if !self.input.cursor_up() {
                        self.input.history_prev();
                    }
                } else {
                    self.input.suggestion_prev();
                }
            }
            KeyCode::Down => {
                if key.modifiers.contains(KeyModifiers::ALT) {
                    self.input.history_next();
                } else if self.input.suggestions.is_empty() {
                    if !self.input.cursor_down() {
                        self.input.history_next();
                    }
                } else {
                    self.input.suggestion_next();
                }
            }
            KeyCode::PageUp => {
                let limit = self.scroll_limit();
                for _ in 0..10 {
                    self.view_log_mut().scroll_up(limit);
                }
            }
            KeyCode::PageDown => {
                let limit = self.scroll_limit();
                self.view_log_mut().scroll_offset = self.view_log_mut().scroll_offset.min(limit);
                for _ in 0..10 {
                    self.view_log_mut().scroll_down();
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
                self.clear_view();
            }
            KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Compose the draft in the external editor; the command goes
                // through the normal path so the running request is honored.
                let _ = self.tx.try_send(TuiEvent::Submit("/editor".into()));
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
        self.history_view
            .as_ref()
            .unwrap_or(&self.log)
            .to_lines(&self.styles, area.width as usize)
            .len()
            .saturating_sub(area.height as usize)
    }

    /// Apply an agent update to the message log.
    pub fn apply_agent_update(&mut self, update: AgentUpdate) {
        self.status.running = true;
        if self.status.run_started.is_none() {
            self.status.run_started = Some(std::time::Instant::now());
        }
        match update {
            AgentUpdate::TextDelta(text) => {
                self.status.active_tool = None;
                // Append to the last assistant message if streaming, else create one.
                let mut grow = 0usize;
                if let Some(Message::Assistant {
                    text: existing,
                    streaming: true,
                }) = self.log.last_mut()
                {
                    let before = existing.split('\n').count();
                    existing.push_str(&text);
                    grow = existing.split('\n').count() - before;
                } else {
                    self.log.push(Message::Assistant {
                        text,
                        streaming: true,
                    });
                }
                // A detached reader's place must survive streaming text.
                if !self.log.follow {
                    self.log.scroll_offset += grow;
                }
            }
            AgentUpdate::TextDone => {
                if let Some(Message::Assistant { streaming, .. }) = self.log.last_mut() {
                    *streaming = false;
                }
            }
            AgentUpdate::ToolCallStart { name, arguments } => {
                self.status.tool_calls += 1;
                self.status.active_tool = Some(name.clone());
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
                        self.status.active_tool = None;
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

impl Tui {
    fn view_log_mut(&mut self) -> &mut MessageLog {
        self.history_view.as_mut().unwrap_or(&mut self.log)
    }
    fn clear_view(&mut self) {
        // Only remove an entry after it has been preserved successfully.
        while let Some(message) = self.log.messages.front() {
            let result = match &mut self.log.archive {
                Some(archive) => archive.append(message),
                None => Err(anyhow::anyhow!("scrollback is unavailable")),
            };
            if let Err(error) = result {
                self.log.archive_error = Some(error.to_string());
                self.dialog = Some(crate::dialog::Dialog::text(
                    "Screen retained",
                    "Could not preserve screen history. Remaining entries are still visible.",
                    error.to_string(),
                    vec![],
                ));
                return;
            }
            self.log.messages.pop_front();
        }
        self.log.scroll_offset = 0;
        self.log.follow = true;
        self.history_view = None;
        self.history_page = 0;
        self.notice("Screen cleared; conversation context is retained. /new starts fresh; /load-older inspects earlier output.");
    }
    fn notice(&mut self, text: &str) {
        self.log.push(Message::System {
            text: text.into(),
            kind: SystemKind::Info,
        });
    }
    fn attach_scrollback(&mut self, root: &std::path::Path) {
        match crate::history::Archive::create(root) {
            Ok(archive) => self.log.archive = Some(archive),
            Err(error) => self.log.archive_error = Some(error.to_string()),
        }
    }
    fn reset_log(&mut self, root: &std::path::Path) {
        self.log = MessageLog::new(100);
        self.attach_scrollback(root);
        self.history_view = None;
        self.history_page = 0;
    }
    fn restore_messages(&mut self, messages: &[chimera_llm::Message]) {
        for message in messages {
            match message.role {
                chimera_llm::Role::User => self.log.push(Message::User { text: message.content.clone().unwrap_or_default() }),
                chimera_llm::Role::Assistant => {
                    if let Some(text) = &message.content { if !text.is_empty() { self.log.push(Message::Assistant { text: text.clone(), streaming: false }); } }
                    for call in &message.tool_calls { self.log.push(Message::ToolCall { id: call.id.clone(), name: call.function.name.clone(), arguments: call.function.arguments.clone(), result: None, ok: None, duration_ms: None, expanded: false }); }
                }
                chimera_llm::Role::Tool => {
                    if let Some(Message::ToolCall { result, .. }) = self.log.messages.iter_mut().rev().find(|m| matches!(m, Message::ToolCall { id, .. } if Some(id) == message.tool_call_id.as_ref())) { *result = message.content.clone(); }
                }
                _ => {}
            }
        }
    }
    fn select_mode(&mut self, command: &str) -> bool {
        if command == "/run" && self.read_only {
            self.notice("This process was launched read-only. Restart with the supervised profile to allow changes.");
            return false;
        }
        self.plan_mode = command == "/plan";
        if !self.status.running {
            self.status.mode = if self.plan_mode { "PLAN" } else { "RUN" }.into();
        }
        self.notice(if self.plan_mode {
            "PLAN stays selected for follow-ups. Use /run to allow approved changes."
        } else {
            "RUN stays selected for follow-ups. File changes still require approval."
        });
        true
    }
    fn enqueue(&mut self, text: String, plan: bool) -> bool {
        if self.queued.len() >= 8 {
            self.notice("Queue is full (8 requests). Your draft is retained; /queue lets you edit pending requests.");
            return false;
        }
        self.queued.push_back((text, plan));
        self.status.queued = self.queued.len();
        self.notice(&format!(
            "Queued request {} ({}). /queue inspects or edits pending work.",
            self.queued.len(),
            if plan { "PLAN" } else { "RUN" }
        ));
        true
    }
    fn queue_command(&mut self, argument: &str) {
        if argument.trim() == "resume" {
            self.queue_paused = false;
            self.notice("Queued work will continue after the current request.");
            return;
        }
        let mut rows = vec![crate::dialog::Choice::new(
            "Resume queued work",
            "Continue in order; every request retains its selected mode.",
            "queue:resume",
        )];
        rows.extend(self.queued.iter().enumerate().map(|(i, (text, plan))| {
            crate::dialog::Choice::new(
                format!("{} · {}", i + 1, if *plan { "PLAN" } else { "RUN" }),
                text.lines().next().unwrap_or_default(),
                format!("queue:edit:{i}"),
            )
        }));
        self.dialog = Some(crate::dialog::Dialog::choices("Queued requests", "Enter on a request moves it back to the composer without submitting it. Cancellation pauses the queue; nothing is discarded.", rows));
    }
    fn search_form(&mut self) {
        self.dialog = Some(crate::dialog::Dialog::form(
            "Search conversation",
            "Find literal text in current and archived screen output.",
            vec![crate::dialog::Field::new("Search", "", false)],
            "transcript:search",
        ));
    }
    fn search(&mut self, query: &str) -> Result<()> {
        if query.trim().is_empty() {
            self.search_form();
            return Ok(());
        }
        let mut matches = match &self.log.archive {
            Some(archive) => archive.search(query, 50)?,
            None => Vec::new(),
        };
        matches.extend(
            self.log
                .messages
                .iter()
                .filter(|m| {
                    m.plain_text()
                        .to_lowercase()
                        .contains(&query.to_lowercase())
                })
                .cloned(),
        );
        let text = if matches.is_empty() {
            "No matches. Your conversation and draft are unchanged.".into()
        } else {
            matches
                .iter()
                .map(Message::plain_text)
                .collect::<Vec<_>>()
                .join("\n\n────────\n\n")
        };
        self.dialog = Some(crate::dialog::Dialog::text("Conversation search", &format!("Search: {query}. Latest matching archived entries plus current output. Scroll to inspect; Esc returns to your draft."), text, vec![]));
        Ok(())
    }
    fn tools_view(&mut self) {
        self.tool_views = self
            .history_view
            .as_ref()
            .unwrap_or(&self.log)
            .messages
            .iter()
            .filter(|m| matches!(m, Message::ToolCall { .. }))
            .cloned()
            .collect();
        let rows = self
            .tool_views
            .iter()
            .enumerate()
            .filter_map(|(i, m)| {
                let Message::ToolCall {
                    name, arguments, ..
                } = m
                else {
                    return None;
                };
                Some(crate::dialog::Choice::new(
                    format!("{} · {name}", i + 1),
                    arguments,
                    format!("transcript:tool:{i}"),
                ))
            })
            .collect();
        self.dialog = Some(crate::dialog::Dialog::choices("Inspect tool results", "Choose any tool in this transcript page. /load-older shows earlier pages; /latest returns to live output.", rows));
    }
    fn load_older(&mut self) -> Result<()> {
        if let Some(error) = &self.log.archive_error {
            self.notice(&format!("Scrollback could not be fully preserved: {error}. Saved conversation records remain available through /sessions."));
        }
        let messages = match &self.log.archive {
            Some(archive) => archive.page(self.history_page, 100)?,
            None => Vec::new(),
        };
        if messages.is_empty() {
            self.notice("No older screen output remains. /latest returns to live output.");
            return Ok(());
        }
        self.history_page += 1;
        let mut view = MessageLog::new(101);
        view.push(Message::System { text: format!("Earlier transcript · page {} · /load-older continues · /latest returns to live work",self.history_page), kind: SystemKind::Info });
        for message in messages {
            view.push(message);
        }
        let height = chat::message_area(
            ratatui::layout::Rect::new(0, 0, self.width as u16, self.height as u16),
            false,
        )
        .height as usize;
        view.scroll_offset = view
            .to_lines(&self.styles, self.width)
            .len()
            .saturating_sub(height);
        view.follow = false;
        self.history_view = Some(view);
        Ok(())
    }
    fn transcript_action(&mut self, action: &str, values: &[String]) -> Result<()> {
        if action == "transcript:search" {
            self.search(values.first().map(String::as_str).unwrap_or_default())?;
        } else if action == "queue:resume" {
            self.queue_paused = false;
            self.dialog = None;
        } else if let Some(index) = action
            .strip_prefix("queue:edit:")
            .and_then(|s| s.parse::<usize>().ok())
        {
            if !self.input.is_empty() {
                self.notice(
                    "Keep or submit your current draft before moving a queued request into it.",
                );
                self.dialog = None;
            } else if let Some((text, plan)) = self.queued.remove(index) {
                self.plan_mode = plan;
                if !self.status.running {
                    self.status.mode = if plan { "PLAN" } else { "RUN" }.into();
                }
                self.input.set_buffer(&text);
                self.status.queued = self.queued.len();
                self.dialog = None;
            }
        } else if let Some(index) = action
            .strip_prefix("transcript:tool:")
            .and_then(|s| s.parse::<usize>().ok())
        {
            if let Some(Message::ToolCall {
                name,
                arguments,
                result,
                ..
            }) = self.tool_views.get(index)
            {
                self.dialog = Some(crate::dialog::Dialog::text(&format!("Tool result: {name}"), "Complete stored arguments and result. Scroll with arrows/Page keys; End reaches the end. Esc returns to chat.", format!("Arguments:\n{arguments}\n\nResult:\n{}", result.as_deref().unwrap_or("Still running; reopen after completion.")), vec![]));
            }
        }
        Ok(())
    }
    fn open_jobs(&mut self, jobs: &mut crate::jobs::Jobs, root: &std::path::Path) {
        self.dialog = Some(jobs.open(root.to_path_buf()));
    }
}

async fn new_conversation(
    root: &std::path::Path,
    tui: &mut Tui,
    session: &mut Option<SavedSession>,
) -> Result<()> {
    if !tui.queued.is_empty() {
        tui.notice("Move queued requests back to the composer or finish them before starting a fresh conversation.");
        return Ok(());
    }
    let fresh = SavedSession::create(root.to_path_buf(), "Interactive conversation").await?;
    tui.reset_log(root);
    tui.status.session_id = fresh.id.to_string();
    tui.status.prompt_tokens = 0;
    tui.status.completion_tokens = 0;
    tui.status.context_tokens = 0;
    tui.status.tool_calls = 0;
    tui.status.iteration = 0;
    tui.status.usage_available = false;
    tui.objective.clear();
    *session = Some(fresh);
    tui.notice("New conversation: previous messages are saved in /sessions. Your unsent draft is preserved.");
    Ok(())
}

async fn git_branch(root: &std::path::Path) -> String {
    let mut command = tokio::process::Command::new("git");
    command
        .args([
            "-c",
            &format!("safe.directory={}", root.display()),
            "symbolic-ref",
            "--short",
            "HEAD",
        ])
        .current_dir(root)
        .kill_on_drop(true);
    match tokio::time::timeout(Duration::from_secs(1), command.output()).await {
        Ok(Ok(output)) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().into()
        }
        _ => "detached / no Git".into(),
    }
}

fn estimate_context(messages: &[chimera_llm::Message], objective: &str) -> u64 {
    let bytes = serde_json::to_vec(messages)
        .map(|v| v.len())
        .unwrap_or_default();
    (bytes + objective.len() + chimera_core::agent::DEFAULT_SYSTEM_PROMPT.len()).div_ceil(4) as u64
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
    tui.styles = crate::theme::styles_for_theme(&config.ui.theme);
    if let Some(dir) = chimera_config::config_dir() {
        tui.input.attach_history(dir.join("history"));
    }
    tui.status.session_id = session.id.to_string();
    tui.status.max_iterations = config.agent.max_iterations;
    tui.attach_scrollback(&root);
    tui.restore_messages(&session.messages);
    if !settings.ready(&config) {
        show_setup_needed(&mut tui);
    }
    tui.plan_mode = profile.as_deref() == Some("read-only");
    tui.read_only = tui.plan_mode;
    tui.status.mode = if !settings.ready(&config) {
        "SETUP"
    } else if tui.plan_mode {
        "PLAN"
    } else {
        "RUN"
    }
    .into();
    tui.status.project = root.display().to_string();
    tui.status.branch = git_branch(&root).await;
    let mut session = Some(session);
    let mut active: Option<
        tokio::task::JoinHandle<(SavedSession, Result<chimera_core::agent::AgentRunResult>)>,
    > = None;
    let (cancel, _) = tokio::sync::watch::channel(false);
    let (events, mut updates) = chimera_core::runtime::event_channel();
    let mut pending: Option<(
        uuid::Uuid,
        tokio::sync::oneshot::Sender<bool>,
        chimera_tools::ToolCall,
    )> = None;
    let mut connections = crate::connect::Connections::default();
    let mut models = crate::models::Models::default();
    let mut sessions = crate::sessions::Sessions::default();
    let mut rewind = crate::rewind::Rewind::default();
    let mut permissions = crate::permissions::Permissions::default();
    let mut memory = crate::memory::Memory::default();
    let mut skills = crate::skills::Skills::default();
    let mut external = crate::external::External::default();
    let mut jobs = crate::jobs::Jobs::default();
    let mut review = crate::review::GitReview::default();
    let mut branch_checked = tokio::time::Instant::now();
    let restore = TerminalGuard::new();
    let mut terminal = init_terminal()?;
    if let Some(objective) = initial.filter(|s| !s.trim().is_empty()) {
        // Setup commands must not erase a request supplied on the command line.
        tui.input.record(&objective);
        tui.tx.try_send(TuiEvent::Submit(objective))?;
    }
    let result: Result<()> = async {
        loop {
            connections.poll(&mut tui.dialog).await;
            models.poll(&mut tui.dialog).await;
            sessions.poll(&mut tui.dialog).await;
            rewind.poll(&mut tui.dialog).await;
            permissions.poll(&mut tui.dialog).await;
            memory.poll(&mut tui.dialog).await;
            skills.poll(&mut tui.dialog).await;
            external.poll(&mut tui.dialog).await;
            jobs.poll(&mut tui.dialog).await;
            review.poll(&mut tui.dialog).await;
            // A busy stream must still leave time for keys and drawing.
            for _ in 0..128 {
                let Ok(update) = updates.try_recv() else { break };
                match update {
                    RunEvent::Approval { id, call, answer } => {
                        let preview_root = root.clone();
                        let preview_call = call.clone();
                        let dialog = tokio::task::spawn_blocking(move || crate::review::approval(&preview_root, &preview_call, id)).await?;
                        tui.dialog = Some(dialog);
                        tui.log.push(Message::System { text: format!("Action waiting for review ({id}). Use the review panel; /approve reopens it and /deny rejects. Expires in five minutes."), kind: SystemKind::Warning });
                        pending = Some((id, answer, call));
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
                tui.status.context_tokens = estimate_context(&saved.messages, "");
                session = Some(saved);
                pending = None;
                tui.apply_agent_update(AgentUpdate::TextDone);
                tui.status.running = false;
                tui.status.run_started = None;
                tui.status.active_tool = None;
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
                if kind != SystemKind::Success { tui.queue_paused = true; }
                tui.status.mode = if tui.plan_mode { "PLAN" } else { "RUN" }.into();
                tui.log.push(Message::System { text, kind });
            }
            while let Ok(input) = tui.rx.try_recv() {
                let mut queued_mode = None;
                let text = match input {
                    TuiEvent::Queued(text, plan) => { queued_mode = Some(plan); text },
                    TuiEvent::DialogAction(action, values) if action.starts_with("transcript:") || action.starts_with("queue:") => {
                        if let Err(error) = tui.transcript_action(&action, &values) { tui.notice(&format!("Could not open transcript: {error}")); } continue;
                    },
                    TuiEvent::DialogAction(action, _) if action.starts_with("approval:") => {
                        let parts: Vec<_> = action.split(':').collect();
                        if parts.len() == 3 && pending.as_ref().is_some_and(|(id, _, _)| id.to_string() == parts[2]) {
                            let (_, answer, _) = pending.take().context("pending approval disappeared")?;
                            let allowed = parts[1] == "approve";
                            let delivered = answer.send(allowed).is_ok();
                            tui.dialog = None;
                            tui.log.push(Message::System { text: if delivered { if allowed {"Approved once."} else {"Action denied."} } else {"Action expired; no approval was applied."}.into(), kind: SystemKind::Info });
                        } else { tui.dialog = None; tui.notice("That action expired. No approval was applied."); }
                        continue;
                    },
                    TuiEvent::DialogAction(action, _) if action.starts_with("review:") => {
                        review.action(&action, &mut tui.dialog).await?; continue;
                    },
                    TuiEvent::DialogClosed => {
                        connections.cancel();
                        models.cancel();
                        sessions.cancel();
                        rewind.cancel();
                        permissions.cancel();
                        memory.cancel();
                        skills.cancel();
                        external.cancel();
                        jobs.cancel();
                        review.cancel();
                        continue;
                    },
                    TuiEvent::DialogAction(action, _values) if action.starts_with("models:") => {
                        match models.action(&action, &config, &mut tui.dialog).await {
                            Ok(Some(saved)) => {
                                let chosen = saved.provider.model.clone();
                                let provider = saved.provider.name.clone();
                                match settings.load() {
                                    Ok(loaded) => {
                                        config = loaded;
                                        if tui.status.model != config.provider.model { tui.status.context_window = None; }
                                tui.status.model = config.provider.model.clone();
                                        tui.status.provider = config.provider.name.clone();
                                        tui.status.mode = if tui.plan_mode { "PLAN" } else { "RUN" }.into();
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
                        let archive_id = action.strip_prefix("sessions:delete-confirm:").or_else(|| action.strip_prefix("sessions:delete:"));
                        if archive_id.is_some_and(|id| session.as_ref().is_some_and(|current| current.id.to_string() == id)) {
                            tui.dialog = None;
                            tui.notice("This conversation is open. Use /new before archiving it.");
                            continue;
                        }
                        match sessions.action(&action, &mut tui.dialog).await {
                            Ok(Some(resumed)) => {
                                tui.reset_log(&root);
                                tui.restore_messages(&resumed.messages);
                                tui.objective.clear();
                                tui.status.prompt_tokens = 0;
                                tui.status.completion_tokens = 0;
                                tui.status.tool_calls = 0;
                                tui.status.iteration = 0;
                                tui.status.usage_available = false;
                                tui.status.context_tokens = estimate_context(&resumed.messages, "");
                                tui.status.session_id = resumed.id.to_string();
                                session = Some(resumed);
                                tui.log.push(Message::System { text: "Conversation opened in this screen. Your unsent text is ready to send.".into(), kind: SystemKind::Success });
                            },
                            Ok(None) => {},
                            Err(error) => if let Some(dialog) = &mut tui.dialog { dialog.note = error.to_string(); }
                        }
                        continue;
                    },
                    TuiEvent::DialogAction(action, _values) if action.starts_with("rewind:") => {
                        match rewind.action(&action, &mut tui.dialog).await {
                            Ok(Some(messages)) => {
                                match session.as_mut() {
                                    Some(current) => {
                                        current.messages = messages.clone();
                                        tui.reset_log(&root);
                                        tui.restore_messages(&messages);
                                        tui.status.context_tokens = estimate_context(&messages, "");
                                        tui.log.push(Message::System { text: "Files restored from the snapshot. The displaced versions were preserved in a private archive, and the conversation returned to the point right after that turn.".into(), kind: SystemKind::Success });
                                    }
                                    None => {
                                        tui.log.push(Message::System { text: "Files were restored from the snapshot, but no conversation is open in this screen; it will show the restored history when you reopen it.".into(), kind: SystemKind::Warning });
                                    }
                                }
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
                    TuiEvent::DialogAction(action, _values) if action.starts_with("permissions:") => {
                        match permissions.action(&action, &mut tui.dialog).await {
                            Ok(Some(report)) => tui.log.push(report),
                            Ok(None) => {},
                            Err(error) => if let Some(dialog) = &mut tui.dialog { dialog.note = error.to_string(); }
                        }
                        continue;
                    },
                    TuiEvent::DialogAction(action, values) if action.starts_with("skills:") => {
                        match skills.action(&action, &values, &mut tui.dialog).await {
                            Ok(Some(objective)) => { tui.tx.try_send(TuiEvent::Submit(objective))?; },
                            Ok(None) => {},
                            Err(error) => if let Some(dialog) = &mut tui.dialog { dialog.note = error.to_string(); }
                        }
                        continue;
                    },
                    TuiEvent::DialogAction(action, values) if action.starts_with("external:") => {
                        match external.action(&action, &values, &root, profile.as_deref() == Some("read-only"), &mut tui.dialog).await {
                            Ok(Some(message)) => tui.log.push(Message::System { text: message, kind: SystemKind::Success }),
                            Ok(None) => {},
                            Err(error) => if let Some(dialog) = &mut tui.dialog { dialog.note = error.to_string(); }
                        }
                        continue;
                    },
                    TuiEvent::DialogAction(action, values) if action.starts_with("jobs:") => {
                        match jobs.action(&action, &values, &mut tui.dialog).await {
                            Ok(Some(report)) => tui.log.push(Message::System { text: report, kind: SystemKind::Success }),
                            Ok(None) => {},
                            Err(error) => if let Some(dialog) = &mut tui.dialog { dialog.note = error.to_string(); }
                        }
                        continue;
                    },
                    TuiEvent::DialogAction(action, values) => {
                        match connections.action(&action, &values, &config, &mut tui.dialog).await {
                            Ok(Some(saved)) => {
                                config = settings.accept(Ok(saved))?;
                                if tui.status.model != config.provider.model { tui.status.context_window = None; }
                                tui.status.model = config.provider.model.clone();
                                tui.status.provider = config.provider.name.clone();
                                tui.status.mode = if tui.plan_mode { "PLAN" } else { "RUN" }.into();
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
                let mut plan = queued_mode.unwrap_or(tui.plan_mode);
                let objective = match command {
                    "/quit" | "/exit" => { tui.should_quit = true; break; }
                    "/clear" => { tui.clear_view(); continue; }
                    "/help" | "/?" => { tui.show_help = true; tui.help_scroll = 0; continue; }
                    "/theme" => { tui.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL)); continue; }
                    "/cancel" => {
                        tui.queue_paused = true;
                        let _ = cancel.send(true);
                        tui.dialog = None;
                        connections.cancel(); models.cancel(); sessions.cancel(); rewind.cancel(); permissions.cancel(); memory.cancel(); skills.cancel(); jobs.cancel(); review.cancel();
                        tui.notice(if active.is_some() { "Stopping current work. Queued requests are paused; /queue resumes or edits them." } else { "No request is running. Queued requests stay paused. Use /queue or /quit." });
                        continue;
                    }
                    "/new" => {
                        if active.is_some() { tui.notice("Stop the current request before starting a new conversation."); }
                        else { if let Err(error) = new_conversation(&root, &mut tui, &mut session).await { tui.notice(&format!("Could not start conversation: {error}")); } }
                        continue;
                    }
                    "/diff" => { tui.dialog = Some(review.open(root.clone())); continue; }
                    "/search" => { if argument.is_empty() { tui.search_form(); } else { if let Err(error) = tui.search(argument) { tui.notice(&format!("Could not search: {error}")); } } continue; }
                    "/tools-view" => { tui.tools_view(); continue; }
                    "/load-older" => { if let Err(error) = tui.load_older() { tui.notice(&format!("Could not open earlier output: {error}")); } continue; }
                    "/latest" => { tui.history_view = None; tui.history_page = 0; continue; }
                    "/queue" => { tui.queue_command(argument); continue; }
                    "/context" => {
                        if argument.trim() == "unknown" { tui.status.context_window = None; }
                        else if !argument.trim().is_empty() {
                            match argument.trim().parse::<u64>() { Ok(value) if value > 0 => tui.status.context_window = Some(value), _ => { tui.notice("Use /context TOKEN_LIMIT or /context unknown. The limit must come from your model's settings."); continue; } }
                        }
                        tui.dialog = Some(crate::dialog::Dialog::text("Context budget", "Capacity is a per-chat user setting, never guessed from a model name. /context N sets the known model limit; /context unknown clears it.", format!("{}\nApproximate next-request context from encoded conversation bytes (4 bytes/token), not a tokenizer measurement. Tool schemas and project facts may add context. Treat headroom as an estimate.\nUse /new for fresh context.", tui.status.context_label()), vec![]));
                        continue;
                    }
                    "/approve" => {
                        if let Some((id, _, call)) = &pending {
                            let (id, call, preview_root) = (*id, call.clone(), root.clone());
                            tui.dialog = Some(tokio::task::spawn_blocking(move || crate::review::approval(&preview_root, &call, id)).await?);
                        } else { tui.notice("No action is waiting for approval."); }
                        continue;
                    }
                    "/deny" => {
                        if let Some((_, answer, _)) = pending.take() {
                            let _ = answer.send(false);
                            tui.dialog = None;
                            tui.notice("Action denied.");
                        } else { tui.notice("No action is waiting for approval."); }
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
                                    tui.styles = crate::theme::styles_for_theme(&config.ui.theme);
                                    if tui.status.model != config.provider.model { tui.status.context_window = None; }
                                tui.status.model = config.provider.model.clone();
                                    tui.status.provider = config.provider.name.clone();
                                    tui.status.max_iterations = config.agent.max_iterations;
                                    tui.status.mode = if tui.plan_mode { "PLAN" } else { "RUN" }.into();
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
                                if tui.status.model != config.provider.model { tui.status.context_window = None; }
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
                    "/rewind" => {
                        if active.is_some() {
                            tui.log.push(Message::System { text: "Cancel or finish the current request before rewinding files.".into(), kind: SystemKind::Warning });
                        } else {
                            match session.as_ref() {
                                Some(current) => tui.dialog = Some(rewind.open(root.clone(), current.id, profile.as_deref() == Some("read-only"))),
                                None => tui.log.push(Message::System { text: "No conversation is open in this screen; use /sessions to open one first.".into(), kind: SystemKind::Warning }),
                            }
                        }
                        continue;
                    }
                    "/editor" => {
                        if active.is_some() {
                            tui.log.push(Message::System { text: "Cancel or finish the current request before editing the draft in your editor.".into(), kind: SystemKind::Warning });
                        } else {
                            match crate::editor::suspend_and_edit(&mut terminal, &config, &tui.input.buffer) {
                                Ok(outcome) => {
                                    if let Some(draft) = outcome.draft {
                                        tui.input.set_buffer(&draft);
                                    }
                                    tui.log.push(Message::System { text: outcome.message, kind: outcome.kind });
                                }
                                Err(error) => tui.log.push(Message::System { text: error.to_string(), kind: SystemKind::Warning }),
                            }
                        }
                        continue;
                    }
                    "/permissions" => {
                        if active.is_some() { tui.log.push(Message::System { text: "Cancel or finish the current request before changing permissions.".into(), kind: SystemKind::Warning }); }
                        else { tui.dialog = Some(permissions.open(root.clone(), plan)); }
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
                    "/skills" => {
                        let requested = argument.trim();
                        if active.is_some() {
                            tui.log.push(Message::System { text: "Cancel or finish the current request before using saved procedures.".into(), kind: SystemKind::Warning });
                            continue;
                        }
                        if requested.is_empty() {
                            tui.dialog = Some(skills.open(root.clone(), session.as_ref().map(|saved| saved.id)));
                            continue;
                        }
                        match crate::skills::Skills::objective(root.clone(), requested).await {
                            Ok(objective) => objective,
                            Err(error) => {
                                tui.log.push(Message::System { text: format!("Could not run the saved procedure '{requested}': {error:#}"), kind: SystemKind::Warning });
                                continue;
                            }
                        }
                    }
                    "/mcp" => {
                        if active.is_some() { tui.log.push(Message::System { text: "Finish or cancel the current request before managing external tools.".into(), kind: SystemKind::Warning }); }
                        else { tui.dialog = Some(external.open(chimera_mcp::default_config_path())); }
                        continue;
                    }
                    "/jobs" => { tui.open_jobs(&mut jobs, &root); continue; }
                    "/run" | "/plan" => {
                        if !tui.select_mode(command) { continue; }
                        plan = tui.plan_mode;
                        argument.to_string()
                    }
                    _ if command.starts_with('/') => { tui.log.push(Message::System { text: format!("Unknown command {command}. Use /help."), kind: SystemKind::Warning }); continue; }
                    _ => text,
                };
                if objective.trim().is_empty() { continue; }
                if active.is_some() || (queued_mode.is_none() && !tui.queued.is_empty() && !tui.queue_paused) {
                    if !tui.enqueue(objective.clone(), plan) { tui.input.set_buffer(&unsent); }
                    continue;
                }
                if !settings.ready(&config) {
                    tui.input.set_buffer(&unsent);
                    show_setup_needed(&mut tui);
                    continue;
                }
                tui.objective = objective.clone();
                tui.status.mode = if plan { "PLAN" } else { "RUN" }.into();
                tui.log.push(Message::User { text: objective.clone() });
                tui.status.running = true;
                // Clock the spinner and elapsed readout from submit, not from
                // the first agent event, so a slow provider still shows time.
                tui.status.run_started = Some(std::time::Instant::now());
                tui.status.active_tool = None;
                cancel.send_replace(false);
                let cancellation = cancel.subscribe();
                let provider = config.build_provider();
                let model = config.provider.model.clone();
                let agent_config = AgentConfig { max_iterations: config.agent.max_iterations, temperature: Some(config.provider.temperature), max_tokens: Some(config.provider.max_tokens), ..Default::default() };
                let policy = if plan { chimera_tools::ToolPolicy::read_only() } else { chimera_tools::ToolPolicy::supervised() };
                let tools: Arc<dyn chimera_tools::ToolRouter> = match external.attached() {
                    Some(attached) if !plan => attached,
                    Some(_) => {
                        tui.log.push(Message::System { text: "External tools are not used in plan mode; run without /plan to use them.".into(), kind: SystemKind::Warning });
                        Arc::new(chimera_tools::DefaultToolRouter::with_policy(Some(root.clone()), policy))
                    }
                    None => Arc::new(chimera_tools::DefaultToolRouter::with_policy(Some(root.clone()), policy)),
                };
                let events = events.clone();
                let mut saved = session.take().context("saved conversation unavailable")?;
                tui.status.context_tokens = estimate_context(&saved.messages, &objective);
                tui.history_view = None;
                tui.history_page = 0;
                active = Some(tokio::spawn(async move {
                    let result = saved.run(&provider, tools, &model, &objective, &agent_config, chimera_core::runtime::RunChannels { events, cancellation }).await;
                    (saved, result)
                }));
            }
            if active.is_none() && !tui.queue_paused && tui.dialog.is_none() {
                if let Some((text, plan)) = tui.queued.pop_front() {
                    tui.status.queued = tui.queued.len();
                    tui.tx.try_send(TuiEvent::Queued(text, plan))?;
                }
            }
            if tui.should_quit { break; }
            if let Ok(size) = terminal.size() {
                tui.width = size.width as usize;
                tui.log.width = tui.width;
                tui.height = size.height as usize;
                if tui.show_help {
                    tui.help_scroll = tui.help_scroll.min(crate::help::scroll_limit(
                        ratatui::layout::Rect::new(0, 0, size.width, size.height), &tui.styles,
                    ));
                }
            }
            if branch_checked.elapsed() >= Duration::from_secs(5) {
                tui.status.branch = git_branch(&root).await;
                branch_checked = tokio::time::Instant::now();
            }
            terminal.draw(|frame| tui.render(frame))?;
            if event::poll(Duration::from_millis(1))? {
                match event::read()? {
                    Event::Key(key) => tui.handle_key(key),
                    Event::Paste(text) => if let Some(dialog) = &mut tui.dialog { dialog.paste(&text); } else { for character in text.chars() { tui.input.insert_char(character); } },
                    Event::Mouse(mouse) if !tui.show_help && tui.dialog.is_none() => {
                        match mouse.kind {
                            MouseEventKind::ScrollUp => {
                                let limit = tui.scroll_limit();
                                tui.view_log_mut().scroll_up(limit);
                            }
                            MouseEventKind::ScrollDown => {
                                let limit = tui.scroll_limit();
                                let log = tui.view_log_mut();
                                log.scroll_offset = log.scroll_offset.min(limit);
                                log.scroll_down();
                            }
                            _ => {}
                        }
                    }
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
    external.shutdown().await;
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
mod developer_acceptance;

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
    fn ctrl_e_opens_the_editor_without_touching_the_draft() {
        let mut tui = Tui::new(String::new(), "test".into(), "test".into());
        for c in "long draft 世界".chars() {
            tui.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        tui.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
        assert!(matches!(
            tui.rx.try_recv().unwrap(),
            TuiEvent::Submit(text) if text == "/editor"
        ));
        assert_eq!(tui.input.text(), "long draft 世界");
        assert!(!tui.should_quit);
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
    fn slash_suggestions_complete_with_tab_and_enter() {
        let mut tui = Tui::new(String::new(), "test".into(), "test".into());
        for c in "/ru".chars() {
            tui.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert!(!tui.input.suggestions.is_empty());
        tui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(tui.input.text(), "/run ");

        tui.input.buffer.clear();
        tui.input.cursor = 0;
        for c in "/se".chars() {
            tui.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        tui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(tui.input.suggestion_sel, Some(0));
        tui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(tui.input.text(), "/setup ");
        assert!(
            tui.rx.try_recv().is_err(),
            "Enter with a selection must complete, not submit"
        );
        tui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(tui.rx.try_recv().unwrap(), TuiEvent::Submit(_)));
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
    fn shift_arrows_scroll_the_transcript_without_touching_the_draft() {
        let mut tui = Tui::new("scroll".into(), "test".into(), "test".into());
        tui.width = 40;
        tui.height = 12;
        let text = (1..=60)
            .map(|n| format!("row {n:03}"))
            .collect::<Vec<_>>()
            .join("\n");
        tui.log.push(Message::Assistant {
            text,
            streaming: false,
        });
        for c in "draft".chars() {
            tui.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        for _ in 0..5 {
            tui.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
        }
        assert!(tui.log.scroll_offset > 0);
        assert_eq!(tui.input.text(), "draft");
        tui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
        assert!(tui.log.scroll_offset < 5);
        while tui.log.scroll_offset > 0 {
            tui.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
        }
        assert!(tui.log.follow && tui.log.at_bottom());
    }

    #[test]
    fn streaming_grows_the_offset_while_detached() {
        let mut tui = Tui::new(String::new(), "test".into(), "test".into());
        tui.log.push(Message::Assistant {
            text: "one".into(),
            streaming: true,
        });
        tui.log.scroll_up(50);
        tui.apply_agent_update(AgentUpdate::TextDelta("\ntwo".into()));
        assert_eq!(
            tui.log.scroll_offset, 2,
            "streamed lines must not yank a detached view"
        );
        assert!(!tui.log.follow);
    }

    #[test]
    fn backtab_folds_every_tool_card() {
        let mut tui = Tui::new("fold".into(), "test".into(), "test".into());
        tui.apply_agent_update(AgentUpdate::ToolCallStart {
            name: "one".into(),
            arguments: "{}".into(),
        });
        tui.apply_agent_update(AgentUpdate::ToolCallStart {
            name: "two".into(),
            arguments: "{}".into(),
        });
        tui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(tui
            .log
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::ToolCall { expanded, .. } => Some(*expanded),
                _ => None,
            })
            .any(|expanded| expanded));
        tui.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert!(tui.log.messages.iter().all(|m| match m {
            Message::ToolCall { expanded, .. } => !*expanded,
            _ => true,
        }));
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
