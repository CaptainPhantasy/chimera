//! The primary streaming chat view, with space reserved for editable input.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::input::InputBar;
use crate::message::MessageLog;
use crate::status::{self, StatusInfo};
use crate::theme::Styles;

/// Render the full chat view. The frame, rather than an earlier size hint, owns geometry.
pub fn render(
    f: &mut Frame,
    styles: &Styles,
    log: &MessageLog,
    status_info: &StatusInfo,
    input: &InputBar,
    objective: &str,
    _width_hint: usize,
) {
    let area = f.area();
    let regions = chat_areas(area, !input.suggestions.is_empty());
    f.render_widget(
        Block::default().style(Style::default().bg(styles.palette.bg)),
        area,
    );

    render_header(f, styles, regions[0], objective, status_info);
    if log.is_empty() {
        render_welcome(f, styles, regions[1]);
    } else {
        render_messages(f, styles, regions[1], log);
    }
    status::render(f, regions[2], status_info, styles);
    input.render(
        f,
        regions[3],
        styles,
        "Describe a task, or / for commands...",
    );
    if regions[4].height > 0 {
        input.render_suggestions(f, regions[4], styles);
    }
    if log.archive_error.is_some() {
        f.render_widget(Paragraph::new("History storage unavailable: older screen entries may be missing; /sessions retains saved conversation.").style(styles.text_subtle()), regions[5]);
    } else {
        render_footer(f, styles, regions[5]);
    }
}

/// Header, conversation, status, input, suggestions, and keyboard hint.
fn chat_areas(area: Rect, show_suggestions: bool) -> [Rect; 6] {
    let input = if area.height >= 16 {
        5
    } else {
        area.height.min(3)
    };
    let status = u16::from(area.height > input);
    let footer = u16::from(area.height >= 8);
    let header = if area.height >= 10 {
        2
    } else {
        u16::from(area.height >= 6)
    };
    let spare = area.height.saturating_sub(input + status + footer + header);
    // Suggestions get a border and at least one entry only when three conversation rows remain.
    let suggestions = if show_suggestions && spare >= 6 {
        spare.saturating_sub(3).min(7)
    } else {
        0
    };
    Layout::vertical([
        Constraint::Length(header),
        Constraint::Min(0),
        Constraint::Length(status),
        Constraint::Length(input),
        Constraint::Length(suggestions),
        Constraint::Length(footer),
    ])
    .areas(area)
}

/// Share the exact conversation rectangle with scroll calculations.
pub(crate) fn message_area(area: Rect, show_suggestions: bool) -> Rect {
    chat_areas(area, show_suggestions)[1]
}

fn render_header(f: &mut Frame, styles: &Styles, area: Rect, objective: &str, info: &StatusInfo) {
    if area.is_empty() {
        return;
    }
    let width = usize::from(area.width);
    let identity = if info.project.is_empty() {
        "◆ CHIMERA".to_string()
    } else {
        format!(
            "◆ {} [{}]",
            std::path::Path::new(&info.project)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            info.branch
        )
    };
    let brand = status::fit_text(&identity, width);
    let mut used = brand.width();
    let mut spans = vec![Span::styled(brand, styles.brand())];
    let version = concat!("  v", env!("CARGO_PKG_VERSION"));
    if used + version.width() <= width {
        spans.push(Span::styled(version, styles.text_subtle()));
        used += version.width();
    }
    let objective = objective.lines().next().unwrap_or_default().trim();
    if !objective.is_empty() && width.saturating_sub(used) > 5 {
        spans.push(Span::styled("  ·  ", styles.text_faint()));
        spans.push(Span::styled(
            status::fit_text(objective, width - used - 5),
            styles.text_muted(),
        ));
    }
    let block = Block::default()
        .borders(if area.height > 1 {
            Borders::BOTTOM
        } else {
            Borders::NONE
        })
        .border_style(styles.border());
    f.render_widget(Paragraph::new(Line::from(spans)).block(block), area);
}

fn render_welcome(f: &mut Frame, styles: &Styles, area: Rect) {
    if area.is_empty() {
        return;
    }
    if area.height < 4 || area.width < 24 {
        let text = status::fit_text("Type a request · F1 help", usize::from(area.width));
        f.render_widget(Paragraph::new(text).style(styles.text_muted()), area);
        return;
    }
    let margin = if area.width >= 40 { 2 } else { 1 };
    let content = Rect::new(
        area.x + margin,
        area.y + u16::from(area.height >= 10),
        area.width.saturating_sub(margin * 2),
        area.height.saturating_sub(u16::from(area.height >= 10)),
    );
    let mut lines = vec![
        Line::from(Span::styled("What are we working on?", styles.brand())),
        Line::from(Span::styled(
            if content.width >= 38 {
                "Describe a task or ask a question below."
            } else {
                "Type a request below."
            },
            styles.text_muted(),
        )),
    ];
    if content.height >= 6 {
        lines.extend([
            Line::default(),
            Line::from(vec![
                Span::styled("/plan  ", styles.role_user()),
                Span::styled(
                    if content.width >= 38 {
                        "read and explore without changes"
                    } else {
                        "read only"
                    },
                    styles.text_muted(),
                ),
            ]),
            Line::from(vec![
                Span::styled("/      ", styles.role_user()),
                Span::styled("browse commands", styles.text_muted()),
            ]),
            Line::from(vec![
                Span::styled("F1     ", styles.role_user()),
                Span::styled("keyboard help", styles.text_muted()),
            ]),
        ]);
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), content);
}

fn render_messages(f: &mut Frame, styles: &Styles, area: Rect, log: &MessageLog) {
    if area.is_empty() {
        return;
    }
    let all_lines = log.to_lines(styles, usize::from(area.width));
    let height = usize::from(area.height);
    let offset = log
        .scroll_offset
        .min(all_lines.len().saturating_sub(height));
    let end = all_lines.len().saturating_sub(offset);
    let start = end.saturating_sub(height);
    // This area has no border: every allocated row is available for conversation.
    f.render_widget(Paragraph::new(all_lines[start..end].to_vec()), area);
}

fn render_footer(f: &mut Frame, styles: &Styles, area: Rect) {
    if area.is_empty() {
        return;
    }
    let width = usize::from(area.width);
    let text = [
        "Enter send · Alt+Enter new line · F1 help · Ctrl+C stop · Ctrl+Q quit",
        "Enter send · F1 help · Ctrl+C stop · Ctrl+Q quit",
        "F1 help · Ctrl+C stop",
        "F1 help",
    ]
    .into_iter()
    .find(|text| text.width() <= width)
    .unwrap_or_default();
    f.render_widget(Paragraph::new(text).style(styles.text_subtle()), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;
    use crate::theme::Palette;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn row(terminal: &Terminal<TestBackend>, y: u16) -> String {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    fn screen(terminal: &Terminal<TestBackend>) -> String {
        (0..terminal.backend().buffer().area.height)
            .map(|y| row(terminal, y))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn opening_screen_explains_the_next_step_without_claiming_connection() {
        for palette in [Palette::dark(), Palette::light()] {
            let styles = Styles::new(palette);
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            terminal
                .draw(|f| {
                    render(
                        f,
                        &styles,
                        &MessageLog::new(100),
                        &StatusInfo::default(),
                        &InputBar::default(),
                        "",
                        80,
                    )
                })
                .unwrap();
            let text = screen(&terminal);
            assert!(text.contains("◆ CHIMERA"));
            assert!(text.contains("What are we working on?"));
            assert!(text.contains("/plan"));
            assert!(text.contains("no model"));
            assert!(text.contains("F1 help"));
            assert!(!text.contains("Connected"));
            assert_eq!(terminal.backend().buffer()[(79, 8)].bg, palette.bg);
        }
    }

    #[test]
    fn compact_welcome_keeps_its_last_hint_visible() {
        for (width, height) in [(24, 16), (40, 16)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|f| {
                    render(
                        f,
                        &Styles::dark(),
                        &MessageLog::new(100),
                        &StatusInfo::default(),
                        &InputBar::default(),
                        "",
                        width as usize,
                    )
                })
                .unwrap();
            assert!(screen(&terminal).contains("keyboard help"));
        }
    }

    #[test]
    fn conversation_uses_its_last_row_and_scrolls_to_its_first_row() {
        let styles = Styles::dark();
        let mut log = MessageLog::new(100);
        let text = format!(
            "FIRST ROW\n{}\nLAST ROW",
            (0..35).map(|n| format!("row {n}\n")).collect::<String>()
        );
        log.push(Message::Assistant {
            text,
            streaming: false,
        });
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let area = message_area(Rect::new(0, 0, 60, 20), false);
        terminal
            .draw(|f| {
                render(
                    f,
                    &styles,
                    &log,
                    &StatusInfo::default(),
                    &InputBar::default(),
                    "",
                    60,
                )
            })
            .unwrap();
        assert!(row(&terminal, area.bottom() - 1).contains("LAST ROW"));
        log.scroll_offset = usize::MAX;
        terminal
            .draw(|f| {
                render(
                    f,
                    &styles,
                    &log,
                    &StatusInfo::default(),
                    &InputBar::default(),
                    "",
                    60,
                )
            })
            .unwrap();
        assert!(row(&terminal, area.y + 1).contains("FIRST ROW"));
    }

    #[test]
    fn narrow_wrapping_uses_the_frame_instead_of_a_stale_width_hint() {
        let mut log = MessageLog::new(100);
        log.push(Message::User {
            text: format!("{} END", "abcdefghij".repeat(6)),
        });
        let mut terminal = Terminal::new(TestBackend::new(12, 16)).unwrap();
        terminal
            .draw(|f| {
                render(
                    f,
                    &Styles::dark(),
                    &log,
                    &StatusInfo::default(),
                    &InputBar::default(),
                    "",
                    1000,
                )
            })
            .unwrap();
        assert!(screen(&terminal).contains("END"));
    }

    #[test]
    fn suggestions_yield_to_input_in_a_short_terminal() {
        let mut input = InputBar::default();
        input.insert_char('/');
        for (width, height) in [(1, 1), (2, 2), (12, 5), (24, 8), (40, 12), (80, 24)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|f| {
                    render(
                        f,
                        &Styles::dark(),
                        &MessageLog::new(100),
                        &StatusInfo::default(),
                        &input,
                        "界面 🦋",
                        80,
                    )
                })
                .unwrap();
            let regions = chat_areas(Rect::new(0, 0, width, height), true);
            assert!(regions[3].height >= height.min(3));
            if height < 10 {
                assert_eq!(regions[4].height, 0);
            }
            if width >= 12 && height >= 5 {
                assert!(screen(&terminal).contains('/'));
            }
            if width > 2 && height >= 3 {
                let cursor = terminal.get_cursor_position().unwrap();
                assert!(cursor.x < width && cursor.y < height);
            }
        }
    }

    #[test]
    fn conversation_retains_tool_results_and_answer_content() {
        let mut log = MessageLog::new(100);
        log.push(Message::ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: "notes.txt".into(),
            result: Some("saved evidence".into()),
            ok: Some(true),
            duration_ms: Some(5),
            expanded: true,
        });
        log.push(Message::Assistant {
            text: "Answer content".into(),
            streaming: false,
        });
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|f| {
                render(
                    f,
                    &Styles::dark(),
                    &log,
                    &StatusInfo::default(),
                    &InputBar::default(),
                    "read notes",
                    80,
                )
            })
            .unwrap();
        let text = screen(&terminal);
        assert!(text.contains("read_file"));
        assert!(text.contains("saved evidence"));
        assert!(text.contains("Answer content"));
        assert!(!text.contains("What are we working on?"));
    }
}
