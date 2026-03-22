use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::App;

/// Render the creature inspector panel.
pub fn render(f: &mut Frame, app: &App) {
    let creature = match app.creatures.get(app.selected_creature) {
        Some(c) => c,
        None => {
            let p = Paragraph::new("No creature selected.")
                .block(Block::default().title(" Creature Inspector ").borders(Borders::ALL));
            f.render_widget(p, f.area());
            return;
        }
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),  // info
            Constraint::Length(6),  // plan placeholder
            Constraint::Min(4),    // evidence
            Constraint::Length(1), // keys
        ])
        .split(f.area());

    // Info block
    let info_lines = vec![
        Line::from(vec![
            Span::styled("ID:    ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&creature.id),
        ]),
        Line::from(vec![
            Span::styled("Role:  ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&creature.role),
        ]),
        Line::from(vec![
            Span::styled("World: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&creature.world),
        ]),
        Line::from(vec![
            Span::styled("State: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(&creature.state, Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled("Conf:  ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!("{:.2}", creature.confidence)),
        ]),
    ];
    let info_block = Block::default()
        .title(format!(" Creature: {} ", creature.id))
        .borders(Borders::ALL);
    f.render_widget(Paragraph::new(info_lines).block(info_block), chunks[0]);

    // Plan placeholder
    let plan_block = Block::default()
        .title(" Current Plan ")
        .borders(Borders::ALL);
    let plan = Paragraph::new(" (plan details populated at runtime)")
        .block(plan_block);
    f.render_widget(plan, chunks[1]);

    // Evidence placeholder
    let evidence_block = Block::default()
        .title(" Evidence ")
        .borders(Borders::ALL);
    let evidence = Paragraph::new(" (evidence details populated at runtime)")
        .block(evidence_block);
    f.render_widget(evidence, chunks[2]);

    // Keys
    let keys = Line::from(vec![
        Span::styled("[enter]", Style::default().fg(Color::Green)),
        Span::raw(" approve step  "),
        Span::styled("[b]", Style::default().fg(Color::Blue)),
        Span::raw(" open shell  "),
        Span::styled("[t]", Style::default().fg(Color::Magenta)),
        Span::raw(" traces  "),
        Span::styled("[Tab]", Style::default().fg(Color::DarkGray)),
        Span::raw(" back"),
    ]);
    f.render_widget(Paragraph::new(keys), chunks[3]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::test_helpers::{sample_app, test_terminal};

    #[test]
    fn inspector_renders_with_creature() {
        let app = sample_app();
        let mut terminal = test_terminal(80, 30);
        terminal.draw(|f| render(f, &app)).unwrap();
    }

    #[test]
    fn inspector_renders_no_creatures() {
        let app = App::new("s".into(), "o".into());
        let mut terminal = test_terminal(80, 30);
        terminal.draw(|f| render(f, &app)).unwrap();
    }
}
