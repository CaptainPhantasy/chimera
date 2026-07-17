//! chimera-tui::chat — The primary streaming chat view.
//!
//! Layout:
//!   ┌────────────────────────────┐
//!   │ header (brand + objective) │
//!   │ message stream (scroll)    │
//!   │ status bar                 │
//!   │ input bar                  │
//!   └────────────────────────────┘

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::input::InputBar;
use crate::message::MessageLog;
use crate::status::{self, StatusInfo};
use crate::theme::Styles;

/// Render the full chat view.
pub fn render(
    f: &mut Frame,
    styles: &Styles,
    log: &MessageLog,
    status_info: &StatusInfo,
    input: &InputBar,
    _objective: &str,
    width_hint: usize,
) {
    let area = f.area();
    let show_suggestions = !input.suggestions.is_empty();

    let constraints = if show_suggestions {
        vec![
            Constraint::Length(2), // header
            Constraint::Min(5),    // messages
            Constraint::Length(1), // status
            Constraint::Length(3), // input
            Constraint::Length(8), // suggestions
        ]
    } else {
        vec![
            Constraint::Length(2), // header
            Constraint::Min(5),    // messages
            Constraint::Length(1), // status
            Constraint::Length(3), // input
        ]
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    render_header(f, styles, chunks[0], status_info);
    render_messages(f, styles, chunks[1], log, width_hint);
    status::render(f, chunks[2], status_info, styles);
    input.render(f, chunks[3], styles, "Type your objective, or / for commands...");

    if show_suggestions {
        input.render_suggestions(f, chunks[4], styles);
    }
}

fn render_header(f: &mut Frame, styles: &Styles, area: Rect, info: &StatusInfo) {
    let obj = objective_short(info);
    let objective_preview = if obj.len() > 60 {
        format!("{}…", &obj[..60])
    } else {
        obj.to_string()
    };

    let line = Line::from(vec![
        Span::styled("◆ ", styles.brand()),
        Span::styled("CHIMERA", styles.brand()),
        Span::styled("  v0.2.0", styles.text_faint()),
        Span::raw("   "),
        Span::styled("objective: ", styles.text_subtle()),
        Span::styled(objective_preview, styles.text_muted()),
    ]);

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(styles.border());

    let p = Paragraph::new(line).block(block);
    f.render_widget(p, area);
}

fn objective_short(info: &StatusInfo) -> &str {
    // We pass the objective via a static-ish channel; for now use the mode label.
    info.mode.as_str()
}

fn render_messages(f: &mut Frame, styles: &Styles, area: Rect, log: &MessageLog, width: usize) {
    let width = width.max(20);
    let all_lines = log.to_lines(styles, width);

    let inner_height = area.height.saturating_sub(2) as usize; // borders
    let total_lines = all_lines.len();

    // Determine which lines to show based on scroll offset.
    // scroll_offset = 0 means pinned to bottom.
    let visible: Vec<&Line> = if total_lines <= inner_height {
        all_lines.iter().collect()
    } else {
        let end = total_lines.saturating_sub(log.scroll_offset);
        let start = end.saturating_sub(inner_height);
        all_lines[start..end.min(total_lines)].iter().collect()
    };

    let block = Block::default()
        .borders(Borders::NONE)
        .border_style(styles.border());

    // We need owned lines for Paragraph.
    let owned: Vec<Line> = visible.into_iter().cloned().collect();
    let p = Paragraph::new(owned).block(block);
    f.render_widget(p, area);
}

// Silence unused warning.
#[allow(unused_variables)]
fn _suppress(objective: &str) {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Message, MessageLog};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn chat_renders_with_messages() {
        let styles = Styles::dark();
        let mut log = MessageLog::new(100);
        log.push(Message::User { text: "hello world".into() });
        log.push(Message::Assistant { text: "Hi there!".into(), streaming: false });
        let status_info = StatusInfo {
            mode: "RUN".into(),
            model: "test".into(),
            ..Default::default()
        };
        let input = InputBar::default();
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render(f, &styles, &log, &status_info, &input, "test objective", 100))
            .unwrap();
    }

    #[test]
    fn chat_renders_empty() {
        let styles = Styles::dark();
        let log = MessageLog::new(100);
        let status_info = StatusInfo::default();
        let input = InputBar::default();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render(f, &styles, &log, &status_info, &input, "test", 80))
            .unwrap();
    }

    #[test]
    fn chat_renders_with_tool_calls() {
        let styles = Styles::dark();
        let mut log = MessageLog::new(100);
        log.push(Message::User { text: "run tests".into() });
        log.push(Message::ToolCall {
            id: "1".into(),
            name: "bash".into(),
            arguments: r#"{"command":"cargo test"}"#.into(),
            result: Some(r#"{"exit_code":0,"stdout":"168 passed"}"#.into()),
            ok: Some(true),
            duration_ms: Some(5000),
            expanded: false,
        });
        log.push(Message::Assistant { text: "All tests pass.".into(), streaming: false });
        let status_info = StatusInfo {
            mode: "RUN".into(),
            model: "test".into(),
            running: true,
            ..Default::default()
        };
        let input = InputBar::default();
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render(f, &styles, &log, &status_info, &input, "test", 100))
            .unwrap();
    }
}
