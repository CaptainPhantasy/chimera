//! chimera-tui::help — Keyboard shortcut help overlay.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::theme::Styles;

/// Render the help overlay as a centered modal.
pub fn render(f: &mut Frame, styles: &Styles) {
    let area = centered_rect(60, 70, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(styles.border_focus())
        .title(Span::styled(" Help — press ? or Esc to close ", styles.title()));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1)])
        .split(block.inner(area));

    let lines = help_lines(styles);

    let p = Paragraph::new(lines).alignment(Alignment::Left);
    f.render_widget(p, chunks[0]);
    f.render_widget(block, area);
}

fn help_lines(styles: &Styles) -> Vec<Line<'static>> {
    let section = |title: &'static str| -> Line<'static> {
        Line::from(vec![
            Span::styled(title, styles.brand()),
            Span::raw(""),
        ])
    };
    let key = |k: &'static str, desc: &'static str| -> Line<'static> {
        Line::from(vec![
            Span::styled(format!("  {k:<20}", ), styles.info()),
            Span::styled(desc, styles.text_muted()),
        ])
    };

    vec![
        section("Input"),
        key("Enter", "submit message / objective"),
        key("Backspace / Delete", "edit input"),
        key("← → / Home / End", "move cursor"),
        key("Alt+← / Alt+→", "move by word"),
        key("/ ", "start a slash command"),
        Span::raw("").into(),
        section("Navigation"),
        key("↑ / ↓", "scroll message log"),
        key("PageUp / PageDown", "scroll fast"),
        key("Tab", "(in tool card) expand/collapse"),
        Span::raw("").into(),
        section("Global"),
        key("? ", "toggle this help"),
        key("Ctrl+L", "switch dark/light theme"),
        key("Ctrl+Space", "clear conversation"),
        key("Ctrl+C / q", "quit"),
        Span::raw("").into(),
        section("Slash Commands"),
        key("/help", "show this help"),
        key("/run <objective>", "execute an objective"),
        key("/model <name>", "change model"),
        key("/tools", "list available tools"),
        key("/clear", "clear conversation"),
        key("/theme", "switch theme"),
        key("/quit", "exit chimera"),
    ]
}

/// Helper: produce a centered rect of given width/height percentages.
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let pop_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(pop_layout[1])[1]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn help_overlay_renders() {
        let styles = Styles::dark();
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &styles)).unwrap();
    }
}
