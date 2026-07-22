//! chimera-tui::runner — The main event loop that drives the terminal.
//!
//! Enters raw mode, creates a ratatui terminal on stdout, polls keyboard
//! events, draws frames at the frame budget, and integrates with the agent
//! loop via a channel-based streaming model.

use std::io::{self, Stdout};
use std::time::{Duration, Instant};

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

/// Initialize the terminal for full-screen TUI rendering.
pub fn init_terminal() -> Result<TuiTerminal> {
    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)
        .context("failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend).context("failed to create terminal")?;
    Ok(terminal)
}

/// Restore the terminal to its original state.
pub fn restore_terminal() -> Result<()> {
    disable_raw_mode().context("failed to disable raw mode")?;
    execute!(io::stdout(), LeaveAlternateScreen)
        .context("failed to leave alternate screen")?;
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
    System { text: String, kind: crate::message::SystemKind },
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
    /// Receiver for user-submit events.
    pub rx: mpsc::UnboundedReceiver<TuiEvent>,
    /// Sender for user-submit events (cloned for input handling).
    pub tx: mpsc::UnboundedSender<TuiEvent>,
    /// Width hint for text wrapping (updated on resize).
    pub width: usize,
}

impl Tui {
    /// Create a new TUI session with the given objective and model info.
    pub fn new(objective: String, model: String, provider: String) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            styles: Styles::dark(),
            log: MessageLog::new(500),
            input: InputBar::new(),
            status: StatusInfo {
                provider,
                model,
                mode: "RUN".into(),
                prompt_tokens: 0,
                completion_tokens: 0,
                iteration: 0,
                max_iterations: 40,
                session_id: short_id(),
                tool_calls: 0,
                running: false,
            },
            objective: objective.clone(),
            should_quit: false,
            show_help: false,
            rx,
            tx,
            width: 80,
        }
    }

    /// Render a single frame.
    pub fn render(&self, f: &mut ratatui::Frame) {
        if self.show_help {
            crate::help::render(f, &self.styles);
            return;
        }
        chat::render(
            f,
            &self.styles,
            &self.log,
            &self.status,
            &self.input,
            &self.objective,
            self.width,
        );
    }

    /// Handle a keyboard event.
    fn handle_key(&mut self, key: KeyEvent) {
        // Only respond to press events (not repeat/release).
        if key.kind != KeyEventKind::Press {
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

        // Help overlay.
        if self.show_help {
            match key.code {
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => {
                    self.show_help = false;
                }
                _ => {}
            }
            return;
        }

        // Input bar handling.
        match key.code {
            KeyCode::Enter => {
                let text = self.input.submit();
                if !text.trim().is_empty() {
                    let _ = self.tx.send(TuiEvent::Submit(text));
                }
            }
            KeyCode::Char('?') => {
                self.show_help = true;
            }
            KeyCode::Up => {
                self.log.scroll_up();
            }
            KeyCode::Down => {
                self.log.scroll_down();
            }
            KeyCode::PageUp => {
                for _ in 0..10 {
                    self.log.scroll_up();
                }
            }
            KeyCode::PageDown => {
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
            KeyCode::Char(c) => {
                self.input.insert_char(c);
            }
            _ => {}
        }
    }

    /// Apply an agent update to the message log.
    pub fn apply_agent_update(&mut self, update: AgentUpdate) {
        self.status.running = true;
        match update {
            AgentUpdate::TextDelta(text) => {
                // Append to the last assistant message if streaming, else create one.
                if let Some(Message::Assistant { text: existing, streaming: true }) =
                    self.log.last_mut()
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
                // Update the last tool call if it matches.
                if let Some(Message::ToolCall { name: tn, result: r, ok: o, duration_ms: d, .. }) =
                    self.log.last_mut()
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

/// Run the TUI. Blocks until the user quits or the agent finishes.
///
/// The `objective` is auto-submitted on start if non-empty.
pub async fn run_interactive(initial_objective: Option<String>) -> Result<()> {
    run_interactive_with_config(initial_objective, "model-unset".into(), "unknown".into()).await
}

/// Run the TUI with explicit model/provider info.
pub async fn run_interactive_with_config(
    initial_objective: Option<String>,
    model: String,
    provider: String,
) -> Result<()> {
    let mut terminal = init_terminal()?;
    let restore = ScopeGuard::new(|| {
        let _ = restore_terminal();
    });

    let mut tui = Tui::new(initial_objective.clone().unwrap_or_default(), model, provider);

    // Auto-submit the initial objective if provided.
    if let Some(obj) = initial_objective {
        if !obj.trim().is_empty() {
            tui.log.push(Message::User { text: obj.clone() });
        }
    }

    let tick_rate = Duration::from_millis(50);
    let mut last_render = Instant::now();

    loop {
        // Update width from terminal size.
        if let Ok(size) = terminal.size() {
            tui.width = size.width as usize;
        }

        // Render.
        terminal.draw(|f| tui.render(f))?;

        // Poll for events with timeout (non-blocking tick).
        let timeout = tick_rate
            .checked_sub(last_render.elapsed())
            .unwrap_or(Duration::ZERO);

        // Check TUI events first (non-blocking).
        while let Ok(event) = tui.rx.try_recv() {
            match event {
                TuiEvent::Key(k) => tui.handle_key(k),
                TuiEvent::Submit(text) => {
                    // Echo user message.
                    tui.log.push(Message::User { text: text.clone() });
                    // Handle slash commands locally.
                    if text.trim() == "/quit" || text.trim() == "/exit" {
                        break;
                    }
                    if text.trim() == "/clear" {
                        tui.log = MessageLog::new(500);
                        continue;
                    }
                    if text.trim() == "/help" || text.trim() == "/?" {
                        tui.show_help = true;
                        continue;
                    }
                    // Otherwise: the caller's agent task picks this up via the channel.
                    // (For the standalone TUI without an agent task, we echo a system note.)
                    if !text.starts_with('/') {
                        // No agent backend in standalone mode; note it.
                        tui.log.push(Message::System {
                            text: "No agent backend connected. Run via `chimera run --interactive` to connect the agent loop.".into(),
                            kind: crate::message::SystemKind::Warning,
                        });
                    }
                }
                TuiEvent::AgentMessage(u) => tui.apply_agent_update(u),
                TuiEvent::AgentDone => {
                    tui.status.running = false;
                    tui.log.push(Message::System {
                        text: "Agent run complete.".into(),
                        kind: crate::message::SystemKind::Success,
                    });
                }
                TuiEvent::Tick => {}
            }
        }

        // Poll terminal keyboard events.
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => {
                    tui.handle_key(key);
                }
                Event::Resize(_, _) => {
                    // Will be handled next frame.
                }
                _ => {}
            }
        }

        if tui.should_quit {
            break;
        }

        last_render = Instant::now();
    }

    drop(restore);
    Ok(())
}

/// RAII guard to ensure terminal restoration.
struct ScopeGuard<F: FnOnce()> {
    f: Option<F>,
}

impl<F: FnOnce()> ScopeGuard<F> {
    fn new(f: F) -> Self {
        Self { f: Some(f) }
    }
}

impl<F: FnOnce()> Drop for ScopeGuard<F> {
    fn drop(&mut self) {
        if let Some(f) = self.f.take() {
            f();
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
}
