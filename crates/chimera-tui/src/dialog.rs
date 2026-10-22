//! Reusable in-chat choices and forms. Secrets never enter chat or input history.
use crate::{input::InputBar, theme::Styles};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

pub struct Choice {
    pub label: String,
    pub detail: String,
    pub action: String,
    /// Optional destructive secondary action offered by the `d` key.
    pub alt_action: Option<String>,
}
impl Choice {
    pub fn new(
        label: impl Into<String>,
        detail: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            detail: detail.into(),
            action: action.into(),
            alt_action: None,
        }
    }
    /// Offer a destructive `d` action for this row (e.g. delete a session).
    pub fn with_alt_action(mut self, alt_action: impl Into<String>) -> Self {
        self.alt_action = Some(alt_action.into());
        self
    }
}
pub struct Field {
    pub label: String,
    pub input: InputBar,
    pub secret: bool,
}
impl Field {
    pub fn new(label: &str, value: &str, secret: bool) -> Self {
        let mut input = InputBar::new();
        input.buffer = value.into();
        input.cursor = value.len();
        Self {
            label: label.into(),
            input,
            secret,
        }
    }
}
pub enum Body {
    Choices {
        rows: Vec<Choice>,
        search: String,
        selected: usize,
    },
    Form {
        fields: Vec<Field>,
        selected: usize,
        action: String,
    },
    Busy,
}
pub struct Dialog {
    pub title: String,
    pub note: String,
    pub body: Body,
}
#[derive(Debug)]
pub enum Outcome {
    Stay,
    Close,
    Action(String, Vec<String>),
}
impl Dialog {
    pub fn choices(title: &str, note: &str, rows: Vec<Choice>) -> Self {
        Self {
            title: title.into(),
            note: note.into(),
            body: Body::Choices {
                rows,
                search: String::new(),
                selected: 0,
            },
        }
    }
    pub fn form(title: &str, note: &str, fields: Vec<Field>, action: &str) -> Self {
        Self {
            title: title.into(),
            note: note.into(),
            body: Body::Form {
                fields,
                selected: 0,
                action: action.into(),
            },
        }
    }
    pub fn busy(title: &str, note: &str) -> Self {
        Self {
            title: title.into(),
            note: note.into(),
            body: Body::Busy,
        }
    }
    pub fn paste(&mut self, text: &str) {
        match &mut self.body {
            Body::Form {
                fields, selected, ..
            } => {
                if let Some(field) = fields.get_mut(*selected) {
                    for c in text.chars().filter(|c| !c.is_control()) {
                        field.input.insert_char(c);
                    }
                }
            }
            Body::Choices {
                search, selected, ..
            } => {
                search.extend(text.chars().filter(|c| !c.is_control()).take(256));
                *selected = 0;
            }
            Body::Busy => {}
        }
    }
    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        if key.code == KeyCode::Esc {
            return Outcome::Close;
        }
        match &mut self.body {
            Body::Choices {
                rows,
                search,
                selected,
            } => {
                let filtered = matching(rows, search);
                match key.code {
                    KeyCode::Up => *selected = selected.saturating_sub(1),
                    KeyCode::Down => {
                        *selected = (*selected + 1).min(filtered.len().saturating_sub(1))
                    }
                    KeyCode::PageDown => {
                        *selected = (*selected + 10).min(filtered.len().saturating_sub(1))
                    }
                    KeyCode::PageUp => *selected = selected.saturating_sub(10),
                    KeyCode::Enter => {
                        if let Some(row) = filtered.get(*selected) {
                            return Outcome::Action(row.action.clone(), vec![]);
                        }
                    }
                    KeyCode::Char('d') => {
                        if let Some(row) = filtered.get(*selected) {
                            if let Some(alt) = &row.alt_action {
                                return Outcome::Action(alt.clone(), vec![]);
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        search.pop();
                        *selected = 0;
                    }
                    KeyCode::Char(c)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                            && search.len() < 256 =>
                    {
                        search.push(c);
                        *selected = 0;
                    }
                    _ => {}
                }
            }
            Body::Form {
                fields,
                selected,
                action,
            } => match key.code {
                KeyCode::Tab | KeyCode::Down => {
                    *selected = (*selected + 1).min(fields.len().saturating_sub(1))
                }
                KeyCode::BackTab | KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Enter if *selected + 1 < fields.len() => *selected += 1,
                KeyCode::Enter => {
                    return Outcome::Action(
                        action.clone(),
                        fields.iter().map(|f| f.input.buffer.clone()).collect(),
                    )
                }
                _ => {
                    if let Some(field) = fields.get_mut(*selected) {
                        match key.code {
                            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                field.input.buffer.clear();
                                field.input.cursor = 0;
                            }
                            KeyCode::Char(c)
                                if !key
                                    .modifiers
                                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                            {
                                field.input.insert_char(c)
                            }
                            KeyCode::Backspace => field.input.backspace(),
                            KeyCode::Delete => field.input.delete_char(),
                            KeyCode::Left => field.input.cursor_left(),
                            KeyCode::Right => field.input.cursor_right(),
                            KeyCode::Home => field.input.cursor_home(),
                            KeyCode::End => field.input.cursor_end(),
                            _ => {}
                        }
                    }
                }
            },
            Body::Busy => {}
        }
        Outcome::Stay
    }
    pub fn render(&self, frame: &mut Frame, styles: &Styles) {
        let full = frame.area();
        let width = full.width.min(96);
        let height = full.height.min(25);
        let area = Rect::new(
            full.x + (full.width - width) / 2,
            full.y + (full.height - height) / 2,
            width,
            height,
        );
        let p = &styles.palette;
        frame.render_widget(Clear, area);
        let block = Block::default()
            .title(format!(" {} ", self.title))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(p.border_focus))
            .style(Style::default().bg(p.bg_panel).fg(p.fg));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let note_rows = ((self.note.chars().count() / usize::from(inner.width.max(1))) + 1)
            .min(usize::from(inner.height.saturating_sub(6).max(1))) as u16;
        let parts = Layout::vertical([
            Constraint::Length(note_rows),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(inner);
        frame.render_widget(
            Paragraph::new(self.note.clone())
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(p.fg_muted)),
            parts[0],
        );
        let hint = match &self.body {
            Body::Choices {
                rows,
                search,
                selected,
            } => {
                let region =
                    Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(parts[1]);
                frame.render_widget(Paragraph::new(format!("Search: {search}")), region[0]);
                let matches = matching(rows, search);
                let items = matches
                    .iter()
                    .map(|row| {
                        ListItem::new(vec![
                            Line::from(row.label.clone()),
                            Line::styled(row.detail.clone(), Style::default().fg(p.fg_muted)),
                        ])
                    })
                    .collect::<Vec<_>>();
                if items.is_empty() {
                    frame.render_widget(
                        Paragraph::new("No matches. Backspace to change the search."),
                        region[1],
                    );
                } else {
                    let mut state =
                        ListState::default().with_selected(Some((*selected).min(items.len() - 1)));
                    frame.render_stateful_widget(
                        List::new(items)
                            .highlight_style(
                                Style::default()
                                    .bg(p.bg_hover)
                                    .fg(p.teal)
                                    .add_modifier(Modifier::BOLD),
                            )
                            .highlight_symbol("› "),
                        region[1],
                        &mut state,
                    );
                }
                if matches.iter().any(|row| row.alt_action.is_some()) {
                    "Type to search · ↑↓ choose · Enter select · d delete · Esc back"
                } else {
                    "Type to search · ↑↓ choose · Enter select · Esc back"
                }
            }
            Body::Form {
                fields, selected, ..
            } => {
                let items = fields
                    .iter()
                    .map(|field| {
                        let (value, _) =
                            field_view(field, parts[1].width.saturating_sub(5) as usize);
                        ListItem::new(vec![
                            Line::from(field.label.clone()),
                            Line::from(format!("  {value}")),
                            Line::from(""),
                        ])
                    })
                    .collect::<Vec<_>>();
                let mut state = ListState::default().with_selected(Some(*selected));
                frame.render_stateful_widget(
                    List::new(items)
                        .highlight_style(Style::default().bg(p.bg_hover).fg(p.teal))
                        .highlight_symbol("› "),
                    parts[1],
                    &mut state,
                );
                if let Some(field) = fields.get(*selected) {
                    let (_, cursor) = field_view(field, parts[1].width.saturating_sub(5) as usize);
                    let y = parts[1]
                        .y
                        .saturating_add(selected.saturating_sub(state.offset()) as u16 * 3 + 1);
                    let x = parts[1].x.saturating_add(4 + cursor as u16);
                    if x < parts[1].right() && y < parts[1].bottom() {
                        frame.set_cursor_position((x, y));
                    }
                }
                "Tab/↑↓ fields · Ctrl+U clear · Enter next/save · Esc cancel"
            }
            Body::Busy => {
                frame.render_widget(Paragraph::new("Working…"), parts[1]);
                "Esc cancel and return to chat"
            }
        };
        frame.render_widget(
            Paragraph::new(hint)
                .wrap(Wrap { trim: true })
                .style(Style::default().fg(p.fg_muted)),
            parts[2],
        );
    }
}
fn field_view(field: &Field, width: usize) -> (String, usize) {
    use unicode_width::UnicodeWidthChar;
    let text = if field.secret {
        "•".repeat(field.input.buffer.chars().count())
    } else {
        field.input.buffer.clone()
    };
    let chars = text.chars().collect::<Vec<_>>();
    let cursor = field.input.buffer[..field.input.cursor].chars().count();
    let mut start = 0;
    let mut cursor_width: usize = chars[..cursor].iter().map(|c| c.width().unwrap_or(0)).sum();
    while cursor_width >= width.max(1) && start < cursor {
        cursor_width = cursor_width.saturating_sub(chars[start].width().unwrap_or(0));
        start += 1;
    }
    let mut shown = String::new();
    let mut used = 0;
    for c in &chars[start..] {
        used += c.width().unwrap_or(0);
        if used > width {
            break;
        }
        shown.push(*c);
    }
    (shown, cursor_width)
}
fn matching<'a>(rows: &'a [Choice], search: &str) -> Vec<&'a Choice> {
    let search = search.to_lowercase();
    rows.iter()
        .filter(|r| {
            format!("{} {}", r.label, r.detail)
                .to_lowercase()
                .contains(&search)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chooser_filters_and_form_masks_secrets() {
        let mut dialog = Dialog::choices(
            "Choose",
            "",
            vec![
                Choice::new("Alpha", "first", "a"),
                Choice::new("Beta", "second", "b"),
            ],
        );
        dialog.paste("beta");
        assert!(
            matches!(dialog.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), Outcome::Action(a, _) if a == "b")
        );
        let dialog = Dialog::form(
            "Connect",
            "Enter your key",
            vec![Field::new("Key", "private-sentinel", true)],
            "save",
        );
        for (w, h) in [(40, 15), (80, 24), (120, 32), (8, 4)] {
            for styles in [Styles::dark(), Styles::new(crate::theme::Palette::light())] {
                let mut terminal =
                    ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
                terminal.draw(|f| dialog.render(f, &styles)).unwrap();
                let text = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|c| c.symbol())
                    .collect::<String>();
                assert!(!text.contains("private-sentinel"));
                if w >= 40 {
                    assert!(text.contains("Connect"));
                    assert!(text.contains("•••"));
                }
            }
        }
    }
}
