//! Keyboard help with explicit wrapping and scrolling for small terminals.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::theme::Styles;

const CONTROLS: &[(&str, &str)] = &[
    ("Enter", "send request"),
    ("Alt/Shift+Enter", "new line; paste keeps new lines"),
    ("Up / Down", "previous / next input"),
    ("Left/Right/Home/End", "move within input"),
    ("Alt+Left/Right", "move by word"),
    ("PageUp/PageDown", "scroll conversation"),
    ("Tab", "expand latest tool result"),
    ("F1 / Escape", "show / close help"),
    ("Ctrl+L / /theme", "dark or light display"),
    ("/run or /plan TEXT", "act, or read only"),
    ("/connect /setup F2", "choose and connect a provider"),
    ("/reload", "reload saved settings"),
    ("/model [NAME]", "show, choose or change model"),
    (
        "/tools /sessions",
        "show tools or browse and switch sessions",
    ),
    ("/rewind", "restore files edited by earlier turns"),
    ("/permissions", "review and change project permissions"),
    ("/memory /remember", "browse, edit or save project facts"),
    ("/skills [NAME]", "list, capture or run saved procedures"),
    ("/approve /deny", "decide the displayed action once"),
    ("/clear", "clear screen; retain saved context"),
    ("/cancel", "stop current work"),
    ("/quit /exit Ctrl+C", "stop work and leave"),
];

pub fn render(f: &mut Frame, styles: &Styles) {
    render_scrolled(f, styles, 0);
}

fn areas(area: Rect) -> (Rect, Rect, Rect) {
    let margin = u16::from(area.width >= 60 && area.height >= 20);
    let panel = Rect::new(
        area.x + margin,
        area.y + margin,
        area.width.saturating_sub(margin * 2),
        area.height.saturating_sub(margin * 2),
    );
    let inner = if panel.height >= 3 && panel.width >= 3 {
        Block::bordered().inner(panel)
    } else {
        panel
    };
    let body = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        inner.height.saturating_sub(1),
    );
    let footer = Rect::new(
        inner.x,
        inner.y + body.height,
        inner.width,
        u16::from(inner.height > 0),
    );
    (panel, body, footer)
}

pub(crate) fn scroll_limit(area: Rect, styles: &Styles) -> usize {
    let (_, body, _) = areas(area);
    help_lines(styles, body.width)
        .len()
        .saturating_sub(body.height as usize)
}

pub fn render_scrolled(f: &mut Frame, styles: &Styles, offset: usize) {
    let (area, body, footer) = areas(f.area());
    f.render_widget(
        Block::default().style(Style::default().bg(styles.palette.bg)),
        f.area(),
    );
    let borders = if area.height >= 3 && area.width >= 3 {
        Borders::ALL
    } else {
        Borders::NONE
    };
    f.render_widget(
        Block::default()
            .borders(borders)
            .border_style(styles.border_focus())
            .title(Span::styled(" Help ", styles.title())),
        area,
    );
    let lines = help_lines(styles, body.width);
    let offset = offset.min(lines.len().saturating_sub(body.height as usize));
    f.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(offset)
                .take(body.height as usize)
                .collect::<Vec<_>>(),
        ),
        body,
    );
    let hint = if footer.width >= 25 {
        "↑↓ scroll · Esc close"
    } else {
        "↑↓ scroll  Esc close"
    };
    f.render_widget(Paragraph::new(hint).style(styles.info()), footer);
}

fn help_lines(styles: &Styles, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::styled("CHIMERA controls", styles.brand()),
        Line::raw(""),
    ];
    for &(key, description) in CONTROLS {
        if width >= 62 {
            lines.push(Line::from(vec![
                Span::styled(format!("  {key:<20}"), styles.info()),
                Span::styled(description, styles.text_muted()),
            ]));
        } else {
            for text in wrap(key, width as usize) {
                lines.push(Line::styled(text, styles.info()));
            }
            for text in wrap(description, width as usize) {
                lines.push(Line::styled(text, styles.text_muted()));
            }
            lines.push(Line::raw(""));
        }
    }
    lines
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut result = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.width() + 1 + word.width() > width {
            result.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        for character in word.chars() {
            let cells = unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
            if !line.is_empty() && line.width() + cells > width {
                result.push(std::mem::take(&mut line));
            }
            line.push(character);
        }
    }
    if !line.is_empty() {
        result.push(line);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn narrow_help_reaches_final_control_and_keeps_close_hint() {
        for (width, height) in [(40, 15), (66, 15), (80, 24), (160, 45)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let styles = Styles::dark();
            terminal
                .draw(|f| render_scrolled(f, &styles, usize::MAX))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect();
            assert!(
                text.contains("stop work and leave"),
                "{width}x{height}: {text}"
            );
            assert!(text.contains("Esc close"));
        }
    }

    #[test]
    fn narrow_help_does_not_drop_long_keys_or_descriptions() {
        let lines = help_lines(&Styles::dark(), 20);
        assert!(lines.iter().all(|line| line.width() <= 20));
        let text = lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("Left/Right/Home/End"));
        assert!(text.contains("retain saved context"));
    }
}
