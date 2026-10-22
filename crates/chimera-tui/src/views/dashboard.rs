use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use ratatui::Frame;

use crate::app::{App, TaskDisplayStatus};
use crate::theme::Styles;

/// Render the main dashboard view.
pub fn render(f: &mut Frame, app: &App, styles: &Styles) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // header
            Constraint::Length(3),  // objective
            Constraint::Min(8),     // creatures
            Constraint::Length(10), // task graph
            Constraint::Length(8),  // feed
            Constraint::Length(1),  // key hints
        ])
        .split(f.area());

    render_header(f, app, styles, chunks[0]);
    render_objective(f, app, chunks[1]);
    render_creatures(f, app, styles, chunks[2]);
    render_task_graph(f, app, styles, chunks[3]);
    render_feed(f, app, styles, chunks[4]);
    render_keys(f, styles, chunks[5]);
}

fn render_header(f: &mut Frame, app: &App, styles: &Styles, area: Rect) {
    let text = vec![Line::from(vec![
        Span::styled("chimera", styles.info().add_modifier(Modifier::BOLD)),
        Span::raw(" :: "),
        Span::raw(&app.session_label),
        Span::raw("    Mode: "),
        Span::styled(&app.mode, styles.warning()),
        Span::raw("    Autonomy: "),
        Span::styled(&app.autonomy, styles.success()),
    ])];
    let block = Block::default().borders(Borders::BOTTOM);
    let p = Paragraph::new(text).block(block);
    f.render_widget(p, area);
}

fn render_objective(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default().title(" Objective ").borders(Borders::ALL);
    let p = Paragraph::new(app.objective.as_str()).block(block);
    f.render_widget(p, area);
}

fn render_creatures(f: &mut Frame, app: &App, styles: &Styles, area: Rect) {
    let header = Row::new(vec![
        "ID",
        "Role",
        "State",
        "World",
        "Conf",
        "Tokens",
        "Last Action",
    ])
    .style(Style::default().add_modifier(Modifier::BOLD))
    .bottom_margin(1);

    let rows: Vec<Row> = app
        .creatures
        .iter()
        .map(|c| {
            let state_style = match c.state.as_str() {
                "running" => styles.success(),
                "waiting" => styles.warning(),
                "idle" => styles.text_faint(),
                _ => styles.text(),
            };
            Row::new(vec![
                Cell::from(c.id.clone()),
                Cell::from(c.role.clone()),
                Cell::from(c.state.clone()).style(state_style),
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

fn render_task_graph(f: &mut Frame, app: &App, styles: &Styles, area: Rect) {
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
            let style = match t.status {
                TaskDisplayStatus::Complete => styles.success(),
                TaskDisplayStatus::InProgress => styles.warning(),
                TaskDisplayStatus::Pending => styles.text_faint(),
                TaskDisplayStatus::Failed => styles.error(),
            };
            Line::from(Span::styled(format!(" {} {}", icon, t.description), style))
        })
        .collect();

    let block = Block::default().title(" Task Graph ").borders(Borders::ALL);
    let p = Paragraph::new(lines).block(block);
    f.render_widget(p, area);
}

fn render_feed(f: &mut Frame, app: &App, styles: &Styles, area: Rect) {
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
                    styles.text_faint(),
                ),
                Span::styled(&l.creature, styles.info()),
                Span::raw(" "),
                Span::raw(&l.message),
            ])
        })
        .collect();

    let block = Block::default().title(" Live Feed ").borders(Borders::ALL);
    let p = Paragraph::new(lines).block(block);
    f.render_widget(p, area);
}

fn render_keys(f: &mut Frame, styles: &Styles, area: Rect) {
    let keys = Line::from(vec![
        Span::styled("[a]", styles.success()),
        Span::raw(" approve  "),
        Span::styled("[d]", styles.error()),
        Span::raw(" deny  "),
        Span::styled("[f]", styles.warning()),
        Span::raw(" fork  "),
        Span::styled("[c]", styles.info()),
        Span::raw(" checkpoint  "),
        Span::styled("[r]", styles.brand_dim()),
        Span::raw(" replay  "),
        Span::styled("[l]", styles.info()),
        Span::raw(" leash  "),
        Span::styled("[q]", styles.text_faint()),
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
        terminal.draw(|f| render(f, &app, &Styles::dark())).unwrap();
    }

    #[test]
    fn dashboard_renders_empty_app() {
        let app = App::new("empty".into(), "nothing".into());
        let mut terminal = test_terminal(80, 24);
        terminal.draw(|f| render(f, &app, &Styles::dark())).unwrap();
    }

    #[test]
    fn dashboard_renders_with_long_feed() {
        let mut app = sample_app();
        for i in 0..10 {
            app.push_feed("C", &format!("msg {}", i));
        }
        let mut terminal = test_terminal(120, 40);
        terminal.draw(|f| render(f, &app, &Styles::dark())).unwrap();
    }
}
