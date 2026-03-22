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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::test_helpers::{sample_app, test_terminal};

    #[test]
    fn shell_grid_renders_with_creatures() {
        let app = sample_app();
        let mut terminal = test_terminal(100, 20);
        terminal.draw(|f| render(f, &app)).unwrap();
    }

    #[test]
    fn shell_grid_renders_empty() {
        let app = App::new("s".into(), "o".into());
        let mut terminal = test_terminal(80, 20);
        terminal.draw(|f| render(f, &app)).unwrap();
    }

    #[test]
    fn shell_grid_renders_idle_status() {
        use crate::app::CreatureRow;
        let mut app = App::new("s".into(), "o".into());
        app.creatures = vec![CreatureRow {
            id: "OWL-1".into(),
            role: "evaluator".into(),
            state: "idle".into(),
            world: "local".into(),
            confidence: 0.93,
            tokens_used: 7000,
            last_action: "ready".into(),
        }];
        let mut terminal = test_terminal(100, 20);
        terminal.draw(|f| render(f, &app)).unwrap();
    }
}
