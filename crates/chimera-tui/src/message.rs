//! chimera-tui::message — Conversation message model for the streaming chat.
//!
//! These types model the agent conversation for display: user messages,
//! assistant messages with streaming text, tool calls with results, and
use std::collections::VecDeque;

use ratatui::text::{Line, Span};

use crate::theme::Styles;

// ---------------------------------------------------------------------------
// Message types
// ---------------------------------------------------------------------------

/// A single entry in the conversation stream.
#[derive(Debug, Clone)]
pub enum Message {
    /// A user-submitted objective or follow-up.
    User { text: String },
    /// Assistant text (may be streaming — accumulates over time).
    Assistant {
        text: String,
        /// True until the model finishes this turn.
        streaming: bool,
    },
    /// A tool call executed by the agent.
    ToolCall {
        id: String,
        name: String,
        arguments: String,
        result: Option<String>,
        ok: Option<bool>,
        duration_ms: Option<u64>,
        /// Whether the card is expanded in the UI.
        expanded: bool,
    },
    /// A system/status line (errors, info, checkpoints).
    System { text: String, kind: SystemKind },
    /// A section divider (e.g. "Iteration 3").
    Divider { label: String },
}

/// Severity/kind for system messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemKind {
    Info,
    Success,
    Warning,
    Error,
}

impl Message {
    /// How many terminal lines this message occupies when rendered at the
    /// given width (approximate — used for scroll math).
    pub fn estimated_lines(&self, width: usize) -> usize {
        let text_len = match self {
            Message::User { text } | Message::Assistant { text, .. } => text.len(),
            Message::System { text, .. } => text.len(),
            Message::ToolCall { name, arguments, result, .. } => {
                let r = result.as_deref().unwrap_or("");
                name.len() + arguments.len() + r.len() + 20
            }
            Message::Divider { label } => label.len() + 6,
        };
        let chars_per_line = width.max(1);
        (text_len / chars_per_line).max(1) + 1
    }

    /// Render this message into styled lines for the terminal.
    pub fn to_lines(&self, styles: &Styles, width: usize) -> Vec<Line<'static>> {
        match self {
            Message::User { text } => {
                let mut lines = vec![Line::from(vec![
                    Span::styled("▸ ", styles.role_user()),
                    Span::styled("you", styles.role_user()),
                ])];
                for wrap in wrap_text(text, width.saturating_sub(2)) {
                    lines.push(Line::from(Span::styled(
                        format!("  {wrap}"),
                        styles.text(),
                    )));
                }
                lines
            }
            Message::Assistant { text, streaming } => {
                let mut lines = vec![Line::from(vec![
                    Span::styled("◆ ", styles.role_assistant()),
                    Span::styled("chimera", styles.role_assistant()),
                    if *streaming {
                        Span::styled(" ▌", styles.brand())
                    } else {
                        Span::raw("")
                    },
                ])];
                for wrap in wrap_text(text, width.saturating_sub(2)) {
                    lines.push(Line::from(Span::styled(
                        format!("  {wrap}"),
                        styles.text(),
                    )));
                }
                lines
            }
            Message::ToolCall {
                id: _,
                name,
                arguments,
                result,
                ok,
                duration_ms,
                expanded,
            } => {
                let icon = match ok {
                    Some(true) => "✓",
                    Some(false) => "✗",
                    None => "⟳",
                };
                let status_style = match ok {
                    Some(true) => styles.success(),
                    Some(false) => styles.error(),
                    None => styles.warning(),
                };
                let dur_str = duration_ms
                    .map(|d| format!(" {d}ms"))
                    .unwrap_or_default();

                let header = Line::from(vec![
                    Span::styled(
                        format!("  {icon} ", ),
                        status_style,
                    ),
                    Span::styled(name.clone(), styles.info()),
                    Span::styled(dur_str, styles.text_subtle()),
                    Span::styled(" [tab to expand]", styles.text_faint()),
                ]);

                let mut lines = vec![header];

                if *expanded {
                    // Arguments
                    lines.push(Line::from(Span::styled(
                        "    args:".to_string(),
                        styles.text_subtle(),
                    )));
                    for w in wrap_text(arguments, width.saturating_sub(8)) {
                        lines.push(Line::from(Span::styled(
                            format!("      {w}"),
                            styles.syntax_string(),
                        )));
                    }
                    // Result
                    if let Some(r) = result {
                        lines.push(Line::from(Span::styled(
                            "    result:".to_string(),
                            styles.text_subtle(),
                        )));
                        for w in wrap_text(r, width.saturating_sub(8)) {
                            lines.push(Line::from(Span::styled(
                                format!("      {w}"),
                                styles.text_muted(),
                            )));
                        }
                    }
                }
                lines
            }
            Message::System { text, kind } => {
                let (icon, s) = match kind {
                    SystemKind::Info => ("ℹ", styles.info()),
                    SystemKind::Success => ("✓", styles.success()),
                    SystemKind::Warning => ("⚠", styles.warning()),
                    SystemKind::Error => ("✗", styles.error()),
                };
                let mut lines: Vec<Line<'static>> = Vec::new();
                for wrap in wrap_text(text, width.saturating_sub(4)).into_iter().take(1) {
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {icon} "), s),
                        Span::styled(wrap, s),
                    ]));
                }
                if lines.is_empty() {
                    lines.push(Line::from(Span::styled(
                        format!("  {icon} "),
                        s,
                    )));
                }
                for wrap in wrap_text(text, width.saturating_sub(4)).into_iter().skip(1) {
                    lines.push(Line::from(Span::styled(
                        format!("    {wrap}"),
                        s,
                    )));
                }
                lines
            }
            Message::Divider { label } => {
                vec![Line::from(vec![
                    Span::styled("─".repeat(3), styles.text_faint()),
                    Span::styled(format!(" {label} "), styles.text_subtle()),
                    Span::styled(
                        "─".repeat(3),
                        styles.text_faint(),
                    ),
                ])]
            }
        }
    }
}

/// Word-wrap text to a max character width.
fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }
    let mut result = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            result.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if current.is_empty() {
                current = word.to_string();
            } else if current.len() + 1 + word.len() <= max_width {
                current.push(' ');
                current.push_str(word);
            } else {
                result.push(std::mem::take(&mut current));
                current = word.to_string();
            }
        }
        if !current.is_empty() {
            result.push(current);
        }
    }
    if result.is_empty() {
        result.push(String::new());
    }
    result
}

// ---------------------------------------------------------------------------
// Message log — bounded, scrollable
// ---------------------------------------------------------------------------

/// A bounded, scrollable log of conversation messages.
#[derive(Debug, Clone, Default)]
pub struct MessageLog {
    pub messages: VecDeque<Message>,
    /// Maximum messages retained.
    pub capacity: usize,
    /// Scroll offset from the bottom (0 = pinned to latest).
    pub scroll_offset: usize,
}

impl MessageLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            messages: VecDeque::with_capacity(capacity),
            capacity,
            scroll_offset: 0,
        }
    }

    /// Push a message, evicting the oldest if over capacity.
    pub fn push(&mut self, msg: Message) {
        if self.messages.len() >= self.capacity {
            self.messages.pop_front();
        }
        self.messages.push_back(msg);
        // Reset scroll to follow new messages.
        self.scroll_offset = 0;
    }

    /// Scroll up (toward older messages).
    pub fn scroll_up(&mut self) {
        let len = self.messages.len();
        if self.scroll_offset < len.saturating_sub(1) {
            self.scroll_offset += 1;
        }
    }

    /// Scroll down (toward newer messages).
    pub fn scroll_down(&mut self) {
        if self.scroll_offset > 0 {
            self.scroll_offset -= 1;
        }
    }

    /// Jump to the latest message.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    /// Whether the view is pinned to the latest message.
    pub fn at_bottom(&self) -> bool {
        self.scroll_offset == 0
    }

    /// Render all visible messages into lines.
    pub fn to_lines(&self, styles: &Styles, width: usize) -> Vec<Line<'static>> {
        let msgs: Vec<&Message> = self.messages.iter().collect();
        let mut lines: Vec<Line<'static>> = Vec::new();
        for msg in &msgs {
            lines.extend(msg.to_lines(styles, width));
        }
        lines
    }

    /// Get a mutable reference to the last message, if any.
    pub fn last_mut(&mut self) -> Option<&mut Message> {
        self.messages.back_mut()
    }

    /// Number of messages.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Whether the log is empty.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

// Silence unused import warning for Style in some build configs.
#[allow(unused_imports)]
use ratatui::style::Color;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_text_respects_max_width() {
        let lines = wrap_text("hello world this is a test", 10);
        for l in &lines {
            assert!(l.len() <= 10, "line '{l}' is {} chars", l.len());
        }
        assert!(lines.len() > 1);
    }

    #[test]
    fn wrap_text_handles_empty_string() {
        let lines = wrap_text("", 10);
        assert_eq!(lines, vec!["".to_string()]);
    }

    #[test]
    fn wrap_text_handles_newlines() {
        let lines = wrap_text("line one\nline two", 80);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn message_log_pushes_and_evicts() {
        let mut log = MessageLog::new(3);
        log.push(Message::User { text: "a".into() });
        log.push(Message::User { text: "b".into() });
        log.push(Message::User { text: "c".into() });
        log.push(Message::User { text: "d".into() });
        assert_eq!(log.len(), 3);
    }

    #[test]
    fn message_log_scrolls() {
        let mut log = MessageLog::new(100);
        for i in 0..10 {
            log.push(Message::User { text: format!("msg {i}") });
        }
        assert!(log.at_bottom());
        log.scroll_up();
        assert!(!log.at_bottom());
        assert_eq!(log.scroll_offset, 1);
        log.scroll_down();
        assert!(log.at_bottom());
    }

    #[test]
    fn user_message_renders_with_label() {
        let styles = Styles::dark();
        let msg = Message::User { text: "hello".into() };
        let lines = msg.to_lines(&styles, 80);
        assert!(lines.len() >= 2);
    }

    #[test]
    fn assistant_streaming_shows_cursor() {
        let styles = Styles::dark();
        let msg = Message::Assistant { text: "thinking".into(), streaming: true };
        let lines = msg.to_lines(&styles, 80);
        // Header line should contain the cursor block
        let header_str = format!("{:?}", lines[0]);
        assert!(header_str.contains("▌"));
    }

    #[test]
    fn tool_call_renders_status_icon() {
        let styles = Styles::dark();
        let msg = Message::ToolCall {
            id: "1".into(),
            name: "bash".into(),
            arguments: r#"{"command":"echo hi"}"#.into(),
            result: Some(r#"{"exit_code":0}"#.into()),
            ok: Some(true),
            duration_ms: Some(42),
            expanded: false,
        };
        let lines = msg.to_lines(&styles, 80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn tool_call_expanded_shows_args_and_result() {
        let styles = Styles::dark();
        let msg = Message::ToolCall {
            id: "1".into(),
            name: "bash".into(),
            arguments: r#"{"command":"echo hi"}"#.into(),
            result: Some(r#"{"exit_code":0}"#.into()),
            ok: Some(true),
            duration_ms: Some(42),
            expanded: true,
        };
        let lines = msg.to_lines(&styles, 80);
        let joined = format!("{:?}", lines);
        assert!(joined.contains("args"));
        assert!(joined.contains("result"));
    }

    #[test]
    fn divider_renders_dashes() {
        let styles = Styles::dark();
        let msg = Message::Divider { label: "Iteration 2".into() };
        let lines = msg.to_lines(&styles, 80);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn system_message_renders_icon() {
        let styles = Styles::dark();
        let msg = Message::System { text: "done".into(), kind: SystemKind::Success };
        let lines = msg.to_lines(&styles, 80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn estimated_lines_is_positive() {
        let msg = Message::User { text: "x".into() };
        assert!(msg.estimated_lines(80) >= 1);
    }
}
