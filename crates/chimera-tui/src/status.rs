//! chimera-tui::status — Contextual status bar with model, tokens, cost.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::Styles;

// ---------------------------------------------------------------------------
// Status info
// ---------------------------------------------------------------------------

/// The contextual data displayed in the status bar.
#[derive(Debug, Clone, Default)]
pub struct StatusInfo {
    /// Provider name (e.g. "openai", "ollama").
    pub provider: String,
    /// Model id (e.g. "gpt-4o-mini").
    pub model: String,
    /// Mode label (RUN, PLAN, SWARM).
    pub mode: String,
    /// Total prompt tokens used this session.
    pub prompt_tokens: u64,
    /// Total completion tokens used this session.
    pub completion_tokens: u64,
    pub usage_available: bool,
    /// Current iteration count.
    pub iteration: usize,
    /// Max iterations.
    pub max_iterations: usize,
    /// Session ID (short form).
    pub session_id: String,
    /// Number of tool calls made.
    pub tool_calls: u64,
    /// Whether the agent is actively running.
    pub running: bool,
}

impl StatusInfo {
    /// Cost cannot be inferred without the actual provider's price schedule.
    pub fn est_cost(&self) -> String {
        "cost unavailable".into()
    }

    /// Token usage as a compact string.
    pub fn token_str(&self) -> String {
        if !self.usage_available {
            return "unavailable".into();
        }
        let total = self.prompt_tokens.saturating_add(self.completion_tokens);
        if total < 1000 {
            format!("{total}")
        } else if total < 1_000_000 {
            format!("{:.1}k", total as f64 / 1000.0)
        } else {
            format!("{:.1}M", total as f64 / 1_000_000.0)
        }
    }

    /// Iteration progress as "iter 3/40".
    pub fn iter_str(&self) -> String {
        format!("iter {}/{}", self.iteration, self.max_iterations)
    }
}

/// Fit text in terminal cells, with an ellipsis only when it must be shortened.
pub(crate) fn fit_text(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut result = String::new();
    let mut used = 0;
    for character in text.chars() {
        let cells = character.width().unwrap_or(0);
        if used + cells > width - 1 {
            break;
        }
        result.push(character);
        used += cells;
    }
    result.push('…');
    result
}

fn append_group(
    spans: &mut Vec<Span<'static>>,
    used: &mut usize,
    width: usize,
    text: String,
    style: Style,
    separator_style: Style,
) {
    let separator = if spans.is_empty() { "" } else { " · " };
    let needed = separator.width() + text.width();
    if !text.is_empty() && *used + needed <= width {
        if !separator.is_empty() {
            spans.push(Span::styled(separator, separator_style));
        }
        spans.push(Span::styled(text, style));
        *used += needed;
    }
}

/// Assemble complete groups, reserving room for mode before optional measurements.
fn status_line(info: &StatusInfo, styles: &Styles, width: usize) -> Line<'static> {
    let state = if info.running {
        "● Working"
    } else {
        "○ Ready"
    };
    let state_style = if info.running {
        styles.brand()
    } else {
        styles.text_muted()
    };
    let state = if state.width() <= width {
        state.to_owned()
    } else {
        fit_text(if info.running { "Working" } else { "Ready" }, width)
    };
    let mut used = state.width();
    let mut spans = vec![Span::styled(state, state_style)];
    let model = if info.model.trim().is_empty() {
        "no model".to_owned()
    } else {
        info.model.lines().collect::<Vec<_>>().join(" ")
    };
    let mode = info.mode.trim();
    let mode_reserve = if !mode.is_empty() && used + 3 + 1 + 3 + mode.width() <= width {
        3 + mode.width()
    } else {
        0
    };
    let model_width = width.saturating_sub(used + 3 + mode_reserve).min(32);
    append_group(
        &mut spans,
        &mut used,
        width,
        fit_text(&model, model_width),
        styles.brand(),
        styles.text_subtle(),
    );
    append_group(
        &mut spans,
        &mut used,
        width,
        mode.to_owned(),
        styles.status_pill_info(),
        styles.text_subtle(),
    );
    for (text, style) in [
        (format!("tokens: {}", info.token_str()), styles.text_muted()),
        (format!("tools: {}", info.tool_calls), styles.text_muted()),
        (info.est_cost(), styles.text_subtle()),
        (info.iter_str(), styles.text_subtle()),
        (
            if info.session_id.is_empty() {
                String::new()
            } else {
                format!("session {}", fit_text(&info.session_id, 8))
            },
            styles.text_subtle(),
        ),
    ] {
        append_group(
            &mut spans,
            &mut used,
            width,
            text,
            style,
            styles.text_subtle(),
        );
    }
    Line::from(spans)
}

/// Ready means idle; it makes no claim about provider connectivity.
pub fn render(f: &mut Frame, area: Rect, info: &StatusInfo, styles: &Styles) {
    if area.is_empty() {
        return;
    }
    let line = status_line(info, styles, usize::from(area.width));
    f.render_widget(
        Paragraph::new(line).style(styles.text().bg(styles.palette.bg)),
        area,
    );
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
    fn token_str_formats_compactly() {
        let mut info = StatusInfo {
            usage_available: true,
            prompt_tokens: 500,
            completion_tokens: 300,
            ..Default::default()
        };
        assert_eq!(info.token_str(), "800");
        info.prompt_tokens = 500_000;
        assert_eq!(info.token_str(), "500.3k");
        info.prompt_tokens = 2_000_000;
        assert!(info.token_str().contains("M"));
    }

    #[test]
    fn unknown_price_is_not_fabricated() {
        let info = StatusInfo {
            prompt_tokens: 100_000,
            completion_tokens: 20_000,
            ..Default::default()
        };
        let cost = info.est_cost();
        assert_eq!(cost, "cost unavailable");
    }

    #[test]
    fn status_bar_renders() {
        let styles = Styles::dark();
        let info = StatusInfo {
            provider: "ollama".into(),
            model: "test-model".into(),
            mode: "RUN".into(),
            prompt_tokens: 1000,
            completion_tokens: 500,
            usage_available: true,
            iteration: 3,
            max_iterations: 40,
            session_id: "abc123".into(),
            tool_calls: 5,
            running: true,
        };
        let backend = TestBackend::new(120, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render(f, f.area(), &info, &styles))
            .unwrap();
    }

    #[test]
    fn status_bar_renders_idle() {
        let styles = Styles::dark();
        let info = StatusInfo {
            running: false,
            ..Default::default()
        };
        let backend = TestBackend::new(120, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render(f, f.area(), &info, &styles))
            .unwrap();
    }

    fn plain(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn compact_status_retains_state_model_and_mode() {
        let info = StatusInfo {
            model: "local-model".into(),
            mode: "PLAN".into(),
            ..Default::default()
        };
        let line = status_line(&info, &Styles::dark(), 35);
        let text = plain(&line);
        assert!(text.contains("Ready · local-model · PLAN"));
        assert!(!text.contains("tokens:"));
        assert!(line.width() <= 35);
    }

    #[test]
    fn optional_measurements_never_leave_partial_labels() {
        let info = StatusInfo {
            model: "model".into(),
            mode: "RUN".into(),
            ..Default::default()
        };
        for width in 1..160 {
            let line = status_line(&info, &Styles::dark(), width);
            let text = plain(&line);
            assert!(line.width() <= width, "{width}: {text}");
            if text.contains("tokens:") {
                assert!(text.contains("tokens: unavailable"));
            }
            if text.contains("cost") {
                assert!(text.contains("cost unavailable"));
            }
            assert!(!text.ends_with(" · "));
        }
        let text = plain(&status_line(&info, &Styles::dark(), 140));
        assert!(text.contains("tokens: unavailable"));
        assert!(text.contains("cost unavailable"));
    }

    #[test]
    fn long_unicode_model_keeps_mode_and_fits_cell_width() {
        let info = StatusInfo {
            model: "界面🦋".repeat(15),
            mode: "RUN".into(),
            running: true,
            ..Default::default()
        };
        let line = status_line(&info, &Styles::dark(), 32);
        let text = plain(&line);
        assert!(text.contains("Working"));
        assert!(text.contains('…'));
        assert!(text.ends_with("RUN"));
        assert!(line.width() <= 32);
        assert_eq!(fit_text("界a", 2), "…");
        assert_eq!(fit_text("界a", 3), "界a");
    }
}
