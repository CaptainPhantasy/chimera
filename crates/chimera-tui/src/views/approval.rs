use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::App;

/// Render the approval prompt view.
pub fn render(f: &mut Frame, app: &App) {
    let prompt = match &app.approval {
        Some(p) => p,
        None => {
            let p = Paragraph::new("No pending approvals.")
                .block(Block::default().title(" Approval ").borders(Borders::ALL));
            f.render_widget(p, f.area());
            return;
        }
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),  // header
            Constraint::Length(6),  // resources
            Constraint::Length(6),  // verification
            Constraint::Length(3),  // rollback
            Constraint::Min(1),    // spacer
            Constraint::Length(1), // keys
        ])
        .split(f.area());

    // Header
    let header_lines = vec![
        Line::from(vec![
            Span::styled("Request: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&prompt.request_id),
        ]),
        Line::from(vec![
            Span::styled("Actor:   ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&prompt.actor),
        ]),
        Line::from(vec![
            Span::styled("Class:   ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(&prompt.class, Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled("Action:  ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&prompt.action),
        ]),
        Line::from(vec![
            Span::styled("Reason:  ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&prompt.reason),
        ]),
    ];
    let header_block = Block::default()
        .title(" Approval Required ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    f.render_widget(Paragraph::new(header_lines).block(header_block), chunks[0]);

    // Touched resources
    let resource_lines: Vec<Line> = prompt
        .touched_resources
        .iter()
        .map(|r| Line::from(format!("  - {}", r)))
        .collect();
    let res_block = Block::default()
        .title(" Touched Resources ")
        .borders(Borders::ALL);
    f.render_widget(Paragraph::new(resource_lines).block(res_block), chunks[1]);

    // Verification steps
    let verify_lines: Vec<Line> = prompt
        .verification_steps
        .iter()
        .map(|v| Line::from(format!("  - {}", v)))
        .collect();
    let verify_block = Block::default()
        .title(" Prepared Verification ")
        .borders(Borders::ALL);
    f.render_widget(Paragraph::new(verify_lines).block(verify_block), chunks[2]);

    // Rollback
    let rollback_block = Block::default()
        .title(" Rollback ")
        .borders(Borders::ALL);
    let rollback = Paragraph::new(format!("  {}", prompt.rollback_info)).block(rollback_block);
    f.render_widget(rollback, chunks[3]);

    // Keys
    let keys = Line::from(vec![
        Span::styled("[y]", Style::default().fg(Color::Green)),
        Span::raw(" approve once  "),
        Span::styled("[Y]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        Span::raw(" approve for task  "),
        Span::styled("[n]", Style::default().fg(Color::Red)),
        Span::raw(" deny  "),
        Span::styled("[o]", Style::default().fg(Color::Yellow)),
        Span::raw(" reduce scope"),
    ]);
    f.render_widget(Paragraph::new(keys), chunks[5]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::test_helpers::{app_with_approval, test_terminal};

    #[test]
    fn approval_renders_with_prompt() {
        let app = app_with_approval();
        let mut terminal = test_terminal(80, 30);
        terminal.draw(|f| render(f, &app)).unwrap();
    }

    #[test]
    fn approval_renders_no_prompt() {
        let app = App::new("s".into(), "o".into());
        let mut terminal = test_terminal(80, 30);
        terminal.draw(|f| render(f, &app)).unwrap();
    }
}
