//! chimera-tui::message — Conversation message model for the streaming chat.
//!
//! These types model the agent conversation for display: user messages,
//! assistant messages with streaming text, tool calls with results, and
use std::collections::VecDeque;

use ratatui::style::Modifier;
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
            Message::ToolCall {
                name,
                arguments,
                result,
                ..
            } => {
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
                    lines.push(Line::from(Span::styled(format!("  {wrap}"), styles.text())));
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
                lines.extend(markdown_lines(text, styles, width.saturating_sub(2)));
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
                let dur_str = duration_ms.map(|d| format!(" {d}ms")).unwrap_or_default();

                let header = Line::from(vec![
                    Span::styled(format!("  {icon} ",), status_style),
                    Span::styled(name.clone(), styles.info()),
                    Span::styled(dur_str, styles.text_subtle()),
                    Span::styled(
                        if *expanded {
                            " [tab to collapse]"
                        } else {
                            " [tab to expand]"
                        },
                        styles.text_faint(),
                    ),
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
                        if looks_like_diff(r) {
                            lines.extend(diff_lines(r, styles));
                        } else {
                            for w in wrap_text(r, width.saturating_sub(8)) {
                                lines.push(Line::from(Span::styled(
                                    format!("      {w}"),
                                    styles.text_muted(),
                                )));
                            }
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
                for wrap in wrap_text(text, width.saturating_sub(4)).into_iter() {
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {icon} "), s),
                        Span::styled(wrap, s),
                    ]));
                }
                if lines.is_empty() {
                    lines.push(Line::from(Span::styled(format!("  {icon} "), s)));
                }
                lines
            }
            Message::Divider { label } => {
                vec![Line::from(vec![
                    Span::styled("─".repeat(3), styles.text_faint()),
                    Span::styled(format!(" {label} "), styles.text_subtle()),
                    Span::styled("─".repeat(3), styles.text_faint()),
                ])]
            }
        }
    }
}

/// Render assistant markdown: fenced code blocks, "- "/* " bullets,
/// whole-line **bold** and `code` lines. Styling stays per source line so
/// word wrap keeps working; richer inline spans are a later upgrade.
fn markdown_lines(text: &str, styles: &Styles, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut in_code = false;
    for raw in text.split('\n') {
        let trimmed = raw.trim_start();
        if let Some(lang) = trimmed.strip_prefix("```") {
            in_code = !in_code;
            if in_code && !lang.trim().is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("  ── {lang} "),
                    styles.text_faint(),
                )));
            }
            continue;
        }
        if in_code {
            lines.push(Line::from(Span::styled(
                format!("  │ {raw}"),
                styles.code_block(),
            )));
            continue;
        }
        if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            for wrap in wrap_text(item, width.saturating_sub(4)) {
                lines.push(Line::from(Span::styled(
                    format!("  • {wrap}"),
                    styles.text(),
                )));
            }
            continue;
        }
        if let Some(inner) = trimmed
            .strip_prefix("**")
            .and_then(|s| s.strip_suffix("**"))
        {
            if !inner.contains("**") {
                for wrap in wrap_text(inner, width.saturating_sub(2)) {
                    lines.push(Line::from(Span::styled(
                        format!("  {wrap}"),
                        styles.text().add_modifier(Modifier::BOLD),
                    )));
                }
                continue;
            }
        }
        if let Some(inner) = trimmed.strip_prefix('`').and_then(|s| s.strip_suffix('`')) {
            if !inner.is_empty() && !inner.contains('`') {
                lines.push(Line::from(Span::styled(
                    format!("  {inner}"),
                    styles.code_block(),
                )));
                continue;
            }
        }
        for wrap in wrap_text(raw, width.saturating_sub(2)) {
            lines.push(Line::from(Span::styled(format!("  {wrap}"), styles.text())));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled("", styles.text())));
    }
    lines
}

/// True when a tool result looks like a unified diff worth coloring.
fn looks_like_diff(result: &str) -> bool {
    let mut plus = 0;
    let mut minus = 0;
    let mut headers = 0;
    for line in result.lines() {
        if line.starts_with("diff --git") || line.starts_with("Index: ") || line.starts_with("@@") {
            headers += 1;
        } else if line.starts_with('+') && !line.starts_with("+++") {
            plus += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            minus += 1;
        }
    }
    headers > 0 || (plus > 0 && minus > 0)
}

/// Cap on rendered diff lines so a huge patch cannot flood the screen.
const MAX_DIFF_LINES: usize = 400;

/// Render a unified diff with add/remove/hunk colors. Long patches are
/// truncated with a marker instead of flooding the conversation.
fn diff_lines(result: &str, styles: &Styles) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let total = result.lines().count();
    for line in result.lines().take(MAX_DIFF_LINES) {
        let style = if line.starts_with("diff --git")
            || line.starts_with("Index: ")
            || line.starts_with("+++")
            || line.starts_with("---")
        {
            styles.text_subtle()
        } else if line.starts_with("@@") {
            styles.info()
        } else if line.starts_with('+') {
            styles.success()
        } else if line.starts_with('-') {
            styles.error()
        } else {
            styles.text_muted()
        };
        lines.push(Line::from(Span::styled(format!("      {line}"), style)));
    }
    if total > MAX_DIFF_LINES {
        lines.push(Line::from(Span::styled(
            format!("      … {} more diff lines", total - MAX_DIFF_LINES),
            styles.text_faint(),
        )));
    }
    lines
}

/// Word-wrap text to a max character width.
fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    let max_width = max_width.max(1);
    let mut result = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut columns = 0;
        for c in paragraph.chars() {
            let width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            if columns + width > max_width && !line.is_empty() {
                result.push(std::mem::take(&mut line));
                columns = 0;
            }
            line.push(c);
            columns += width;
        }
        result.push(line);
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
    pub fn scroll_up(&mut self, max_offset: usize) {
        self.scroll_offset = self.scroll_offset.min(max_offset);
        if self.scroll_offset < max_offset {
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
            log.push(Message::User {
                text: format!("msg {i}"),
            });
        }
        assert!(log.at_bottom());
        log.scroll_up(9);
        assert!(!log.at_bottom());
        assert_eq!(log.scroll_offset, 1);
        log.scroll_down();
        assert!(log.at_bottom());
    }

    #[test]
    fn fenced_code_blocks_bullets_and_bold_render_styled() {
        let styles = Styles::dark();
        let msg = Message::Assistant {
            text: "Intro:\n```rust\nlet x = 1;\n```\n**Heading**\n- first item\nplain tail".into(),
            streaming: false,
        };
        let lines = msg.to_lines(&styles, 80);
        let text_of = |l: &Line| -> String { l.spans.iter().map(|s| s.content.as_ref()).collect() };
        let texts: Vec<String> = lines.iter().map(text_of).collect();
        assert!(
            texts.iter().any(|t| t.contains("│ let x = 1;")),
            "{texts:?}"
        );
        assert!(!texts.iter().any(|t| t.contains("```")), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains("• first item")));
        assert!(texts.iter().any(|t| t.contains("Heading")));
        let bold_any = lines.iter().any(|l| {
            l.spans.iter().any(|s| {
                s.content.contains("Heading") && s.style.add_modifier.contains(Modifier::BOLD)
            })
        });
        assert!(bold_any, "whole-line bold must carry the BOLD modifier");
    }

    #[test]
    fn tool_results_that_look_like_diffs_get_diff_colors() {
        let styles = Styles::dark();
        let msg = Message::ToolCall {
            id: "1".into(),
            name: "edit".into(),
            arguments: "{}".into(),
            result: Some(
                "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n-old\n+new\n ctx".into(),
            ),
            ok: Some(true),
            duration_ms: Some(5),
            expanded: true,
        };
        let lines = msg.to_lines(&styles, 80);
        let mut saw_add = false;
        let mut saw_del = false;
        let mut saw_hunk = false;
        for line in &lines {
            for span in &line.spans {
                if span.content.contains("+new") {
                    saw_add = span.style == styles.success();
                }
                if span.content.contains("-old") {
                    saw_del = span.style == styles.error();
                }
                if span.content.contains("@@") {
                    saw_hunk = true;
                }
            }
        }
        assert!(saw_add && saw_del && saw_hunk);
    }

    #[test]
    fn plain_tool_results_are_not_colored_like_diffs() {
        assert!(!looks_like_diff("plain output\nwith - hyphen"));
        assert!(!looks_like_diff("key: value\nanother: line"));
        assert!(!looks_like_diff(""));
    }

    #[test]
    fn user_message_renders_with_label() {
        let styles = Styles::dark();
        let msg = Message::User {
            text: "hello".into(),
        };
        let lines = msg.to_lines(&styles, 80);
        assert!(lines.len() >= 2);
    }

    #[test]
    fn assistant_streaming_shows_cursor() {
        let styles = Styles::dark();
        let msg = Message::Assistant {
            text: "thinking".into(),
            streaming: true,
        };
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
        let msg = Message::Divider {
            label: "Iteration 2".into(),
        };
        let lines = msg.to_lines(&styles, 80);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn system_message_renders_icon() {
        let styles = Styles::dark();
        let msg = Message::System {
            text: "done".into(),
            kind: SystemKind::Success,
        };
        let lines = msg.to_lines(&styles, 80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn long_error_continuations_are_rendered_once() {
        let message = Message::System {
            text: "first error line\nsecond error line\nthird error line".into(),
            kind: SystemKind::Error,
        };
        let lines = message.to_lines(&Styles::dark(), 80);
        let text = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(text.matches("second error line").count(), 1, "{text}");
        assert_eq!(text.matches("third error line").count(), 1, "{text}");
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn estimated_lines_is_positive() {
        let msg = Message::User { text: "x".into() };
        assert!(msg.estimated_lines(80) >= 1);
    }
}
