use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::App;

/// A row in the shell grid display.
pub struct ShellGridRow {
    pub label: String,
    pub command: String,
    pub status: String,
}

/// Render the parallel shell grid view.
pub fn render(f: &mut Frame, app: &App) {
    // Build rows from creature data (placeholder mapping).
    let rows: Vec<ShellGridRow> = app
        .creatures
        .iter()
        .map(|c| ShellGridRow {
            label: format!("{}:{}", c.world, c.id),
            command: c.last_action.clone(),
            status: c.state.clone(),
        })
        .collect();

    let lines: Vec<Line> = rows
        .iter()
        .map(|r| {
            let status_color = match r.status.as_str() {
                "running" => Color::Green,
                "waiting" => Color::Yellow,
                "idle" => Color::DarkGray,
                _ => Color::White,
            };
            Line::from(vec![
                Span::styled(
                    format!(" {:20}", r.label),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(format!("{:40}", r.command)),
                Span::styled(&r.status, Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
            ])
        })
        .collect();

    let block = Block::default()
        .title(" Shell Grid ")
        .borders(Borders::ALL);
    let p = Paragraph::new(lines).block(block);
    f.render_widget(p, f.area());
}
