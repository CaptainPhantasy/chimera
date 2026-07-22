//! chimera-tui::status — Contextual status bar with model, tokens, cost.

use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

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
    /// Estimated cost in USD (rough heuristic for OpenAI pricing tiers).
    /// This is a placeholder — real cost tracking needs provider pricing.
    pub fn est_cost(&self) -> String {
        // Very rough: assume ~$3/M input, ~$15/M output (GPT-4o tier).
        let cost = (self.prompt_tokens as f64 * 3.0 / 1_000_000.0)
            + (self.completion_tokens as f64 * 15.0 / 1_000_000.0);
        if cost < 0.01 {
            format!("${cost:.4}")
        } else {
            format!("${cost:.2}")
        }
    }

    /// Token usage as a compact string.
    pub fn token_str(&self) -> String {
        let total = self.prompt_tokens + self.completion_tokens;
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

/// Render the status bar.
pub fn render(f: &mut Frame, area: Rect, info: &StatusInfo, styles: &Styles) {
    let dot = if info.running { "●" } else { "○" };
    let state_style = if info.running {
        styles.success()
    } else {
        styles.text_faint()
    };

    let model_display = if info.model.is_empty() {
        "no model".to_string()
    } else {
        info.model.clone()
    };
    let iter_str = info.iter_str();
    let cost_str = info.est_cost();

    let line = Line::from(vec![
        Span::styled(format!(" {dot} "), state_style),
        Span::styled(&info.mode, styles.status_pill_info()),
        Span::raw(" "),
        Span::styled(&model_display, styles.brand()),
        Span::raw("  "),
        Span::styled("tokens:", styles.text_subtle()),
        Span::styled(format!(" {} ", info.token_str()), styles.text_muted()),
        Span::styled(&iter_str, styles.text_subtle()),
        Span::raw("  "),
        Span::styled("tools:", styles.text_subtle()),
        Span::styled(format!(" {} ", info.tool_calls), styles.text_muted()),
        Span::raw("  "),
        Span::styled(&cost_str, styles.warning()),
        Span::raw("  "),
        Span::styled(&info.session_id, styles.text_faint()),
    ]);

    let p = Paragraph::new(line).alignment(Alignment::Left);
    f.render_widget(p, area);
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
            prompt_tokens: 500,
            completion_tokens: 300,
            ..StatusInfo::default()
        };
        assert_eq!(info.token_str(), "800");
        info.prompt_tokens = 500_000;
        assert_eq!(info.token_str(), "500.3k");
        info.prompt_tokens = 2_000_000;
        assert!(info.token_str().contains("M"));
    }

    #[test]
    fn est_cost_is_positive() {
        let info = StatusInfo {
            prompt_tokens: 100_000,
            completion_tokens: 20_000,
            ..StatusInfo::default()
        };
        let cost = info.est_cost();
        assert!(cost.contains("$"));
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
}
