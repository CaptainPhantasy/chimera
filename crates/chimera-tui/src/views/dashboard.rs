use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use ratatui::Frame;

use crate::app::{App, TaskDisplayStatus};

/// Render the main dashboard view.
pub fn render(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // header
            Constraint::Length(3),  // objective
            Constraint::Min(8),    // creatures
            Constraint::Length(10), // task graph
            Constraint::Length(8),  // feed
            Constraint::Length(1),  // key hints
        ])
        .split(f.area());

    render_header(f, app, chunks[0]);
    render_objective(f, app, chunks[1]);
    render_creatures(f, app, chunks[2]);
    render_task_graph(f, app, chunks[3]);
    render_feed(f, app, chunks[4]);
    render_keys(f, chunks[5]);
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let text = vec![Line::from(vec![
        Span::styled("chimera", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw(" :: "),
        Span::raw(&app.session_label),
        Span::raw("    Mode: "),
        Span::styled(&app.mode, Style::default().fg(Color::Yellow)),
        Span::raw("    Autonomy: "),
        Span::styled(&app.autonomy, Style::default().fg(Color::Green)),
    ])];
    let block = Block::default().borders(Borders::BOTTOM);
    let p = Paragraph::new(text).block(block);
    f.render_widget(p, area);
}

fn render_objective(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title(" Objective ")
        .borders(Borders::ALL);
    let p = Paragraph::new(app.objective.as_str()).block(block);
    f.render_widget(p, area);
}

fn render_creatures(f: &mut Frame, app: &App, area: Rect) {
    let header = Row::new(vec!["ID", "Role", "State", "World", "Conf", "Tokens", "Last Action"])
        .style(Style::default().add_modifier(Modifier::BOLD))
        .bottom_margin(1);

    let rows: Vec<Row> = app
        .creatures
        .iter()
        .map(|c| {
            let state_color = match c.state.as_str() {
                "running" => Color::Green,
                "waiting" => Color::Yellow,
                "idle" => Color::DarkGray,
                _ => Color::White,
            };
            Row::new(vec![
                Cell::from(c.id.clone()),
                Cell::from(c.role.clone()),
                Cell::from(c.state.clone()).style(Style::default().fg(state_color)),
                Cell::from(c.world.clone()),
                Cell::from(format!("{:.2}", c.confidence)),
                Cell::from(format!("{}k", c.tokens_used / 1000)),
                Cell::from(c.last_action.clone()),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(14),
            Constraint::Length(6),
            Constraint::Length(8),
            Constraint::Fill(1),
        ],
    )
    .header(header)
    .block(Block::default().title(" Creatures ").borders(Borders::ALL));

    f.render_widget(table, area);
}

fn render_task_graph(f: &mut Frame, app: &App, area: Rect) {
    let lines: Vec<Line> = app
        .tasks
        .iter()
        .map(|t| {
            let icon = match t.status {
                TaskDisplayStatus::Complete => "[x]",
                TaskDisplayStatus::InProgress => "[~]",
                TaskDisplayStatus::Pending => "[ ]",
                TaskDisplayStatus::Failed => "[!]",
            };
            let color = match t.status {
                TaskDisplayStatus::Complete => Color::Green,
                TaskDisplayStatus::InProgress => Color::Yellow,
                TaskDisplayStatus::Pending => Color::DarkGray,
                TaskDisplayStatus::Failed => Color::Red,
            };
            Line::from(Span::styled(
                format!(" {} {}", icon, t.description),
                Style::default().fg(color),
            ))
        })
        .collect();

    let block = Block::default()
        .title(" Task Graph ")
        .borders(Borders::ALL);
    let p = Paragraph::new(lines).block(block);
    f.render_widget(p, area);
}

fn render_feed(f: &mut Frame, app: &App, area: Rect) {
    let visible = if app.feed.len() > 6 {
        &app.feed[app.feed.len() - 6..]
    } else {
        &app.feed
    };

    let lines: Vec<Line> = visible
        .iter()
        .map(|l| {
            Line::from(vec![
                Span::styled(
                    l.timestamp.format(" %H:%M:%S ").to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(&l.creature, Style::default().fg(Color::Cyan)),
                Span::raw(" "),
                Span::raw(&l.message),
            ])
        })
        .collect();

    let block = Block::default()
        .title(" Live Feed ")
        .borders(Borders::ALL);
    let p = Paragraph::new(lines).block(block);
    f.render_widget(p, area);
}

fn render_keys(f: &mut Frame, area: Rect) {
    let keys = Line::from(vec![
        Span::styled("[a]", Style::default().fg(Color::Green)),
        Span::raw(" approve  "),
        Span::styled("[d]", Style::default().fg(Color::Red)),
        Span::raw(" deny  "),
        Span::styled("[f]", Style::default().fg(Color::Yellow)),
        Span::raw(" fork  "),
        Span::styled("[c]", Style::default().fg(Color::Blue)),
        Span::raw(" checkpoint  "),
        Span::styled("[r]", Style::default().fg(Color::Magenta)),
        Span::raw(" replay  "),
        Span::styled("[l]", Style::default().fg(Color::Cyan)),
        Span::raw(" leash  "),
        Span::styled("[q]", Style::default().fg(Color::DarkGray)),
        Span::raw(" quit"),
    ]);
    f.render_widget(Paragraph::new(keys), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::test_helpers::{sample_app, test_terminal};

    #[test]
    fn dashboard_renders_without_panic() {
        let app = sample_app();
        let mut terminal = test_terminal(120, 40);
        terminal
            .draw(|f| render(f, &app))
            .unwrap();
    }

    #[test]
    fn dashboard_renders_empty_app() {
        let app = App::new("empty".into(), "nothing".into());
        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|f| render(f, &app))
            .unwrap();
    }

    #[test]
    fn dashboard_renders_with_long_feed() {
        let mut app = sample_app();
        for i in 0..10 {
            app.push_feed("C", &format!("msg {}", i));
        }
        let mut terminal = test_terminal(120, 40);
        terminal.draw(|f| render(f, &app)).unwrap();
    }
}
