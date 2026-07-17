//! chimera-tui::input — Multi-line text input with slash-command support.
//!
//! A lightweight, self-contained text editor for the TUI bottom bar. Supports
//! cursor movement, history, and slash-command autocomplete suggestions.

use std::collections::VecDeque;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::theme::Styles;

// ---------------------------------------------------------------------------
// Input state
// ---------------------------------------------------------------------------

/// The text input editor state.
#[derive(Debug, Clone)]
pub struct InputBar {
    /// Buffer content (single line for now; multi-line on Enter with Shift).
    pub buffer: String,
    /// Cursor position (byte offset into buffer).
    pub cursor: usize,
    /// Command history (most recent last).
    pub history: VecDeque<String>,
    /// Current position in history navigation (None = composing new).
    pub history_pos: Option<usize>,
    /// Whether the input is focused/active.
    pub focused: bool,
    /// Slash-command suggestions for the current buffer.
    pub suggestions: Vec<String>,
}

impl Default for InputBar {
    fn default() -> Self {
        Self::new()
    }
}

impl InputBar {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            history: VecDeque::with_capacity(100),
            history_pos: None,
            focused: true,
            suggestions: Vec::new(),
        }
    }

    /// Current input text.
    pub fn text(&self) -> &str {
        &self.buffer
    }

    /// Is the buffer empty?
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Insert a character at the cursor.
    pub fn insert_char(&mut self, c: char) {
        self.buffer.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.update_suggestions();
    }

    /// Delete the character before the cursor (backspace).
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            // Find the char boundary before cursor.
            let prev = self.buffer[..self.cursor].chars().last().unwrap();
            let char_start = self.cursor - prev.len_utf8();
            self.buffer.replace_range(char_start..self.cursor, "");
            self.cursor = char_start;
            self.update_suggestions();
        }
    }

    /// Delete the character at the cursor (delete).
    pub fn delete_char(&mut self) {
        if self.cursor < self.buffer.len() {
            let next = self.buffer[self.cursor..].chars().next().unwrap();
            let end = self.cursor + next.len_utf8();
            self.buffer.replace_range(self.cursor..end, "");
            self.update_suggestions();
        }
    }

    /// Move cursor left one char.
    pub fn cursor_left(&mut self) {
        if self.cursor > 0 {
            let prev = self.buffer[..self.cursor].chars().last().unwrap();
            self.cursor -= prev.len_utf8();
        }
    }

    /// Move cursor right one char.
    pub fn cursor_right(&mut self) {
        if self.cursor < self.buffer.len() {
            let next = self.buffer[self.cursor..].chars().next().unwrap();
            self.cursor += next.len_utf8();
        }
    }

    /// Move cursor to start.
    pub fn cursor_home(&mut self) {
        self.cursor = 0;
    }

    /// Move cursor to end.
    pub fn cursor_end(&mut self) {
        self.cursor = self.buffer.len();
    }

    /// Move one word left.
    pub fn cursor_word_left(&mut self) {
        let mut pos = self.cursor;
        let bytes = self.buffer.as_bytes();
        // Skip trailing whitespace.
        while pos > 0 && bytes[pos - 1].is_ascii_whitespace() {
            pos -= 1;
        }
        // Skip word chars.
        while pos > 0 && !bytes[pos - 1].is_ascii_whitespace() {
            pos -= 1;
        }
        self.cursor = pos;
    }

    /// Move one word right.
    pub fn cursor_word_right(&mut self) {
        let mut pos = self.cursor;
        let bytes = self.buffer.as_bytes();
        // Skip word chars.
        while pos < bytes.len() && !bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        // Skip whitespace.
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        self.cursor = pos;
    }

    /// Clear the buffer and return the submitted text, recording to history.
    pub fn submit(&mut self) -> String {
        let text = std::mem::take(&mut self.buffer);
        self.cursor = 0;
        self.history_pos = None;
        if !text.trim().is_empty() {
            if self.history.len() >= 100 {
                self.history.pop_front();
            }
            self.history.push_back(text.clone());
        }
        self.suggestions.clear();
        text
    }

    /// Navigate to previous history entry (older).
    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let pos = match self.history_pos {
            None => self.history.len() - 1,
            Some(p) if p > 0 => p - 1,
            Some(p) => p,
        };
        self.history_pos = Some(pos);
        self.buffer = self.history[pos].clone();
        self.cursor = self.buffer.len();
    }

    /// Navigate to next history entry (newer).
    pub fn history_next(&mut self) {
        let len = self.history.len();
        match self.history_pos {
            None => {}
            Some(p) => {
                if p + 1 >= len {
                    // Past newest: clear to new composition.
                    self.history_pos = None;
                    self.buffer.clear();
                    self.cursor = 0;
                } else {
                    self.history_pos = Some(p + 1);
                    self.buffer = self.history[p + 1].clone();
                    self.cursor = self.buffer.len();
                }
            }
        }
    }

    /// Update slash-command suggestions based on current buffer.
    fn update_suggestions(&mut self) {
        if self.buffer.starts_with('/') {
            let prefix = &self.buffer[1..];
            self.suggestions = SLASH_COMMANDS
                .iter()
                .filter(|(name, _)| name.starts_with(prefix))
                .take(5)
                .map(|(name, desc)| format!("/{name} — {desc}"))
                .collect();
        } else {
            self.suggestions.clear();
        }
    }

    /// Is the current input a slash command?
    pub fn is_command(&self) -> bool {
        self.buffer.starts_with('/')
    }

    /// Parse the current input as a command (name, args).
    pub fn parse_command(&self) -> Option<(&str, &str)> {
        let trimmed = self.buffer.trim_start_matches('/');
        let (name, rest) = trimmed.split_once(' ').unwrap_or((trimmed, ""));
        if name.is_empty() {
            None
        } else {
            Some((name, rest.trim()))
        }
    }

    /// Render the input bar into the given area.
    pub fn render(&self, f: &mut Frame, area: Rect, styles: &Styles, placeholder: &str) {
        let border_style = if self.focused {
            styles.border_focus()
        } else {
            styles.border()
        };

        let display_text = if self.buffer.is_empty() {
            placeholder.to_string()
        } else {
            self.buffer.clone()
        };

        let text_style = if self.buffer.is_empty() {
            styles.text_faint()
        } else {
            styles.text()
        };

        // Build the paragraph with cursor indicator.
        let mut spans = vec![Span::styled(display_text.clone(), text_style)];
        let _ = &mut spans; // suppress unused warning

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(Span::styled(" input ", styles.title()));

        let p = Paragraph::new(Line::from(Span::styled(display_text, text_style))).block(block);
        f.render_widget(p, area);

        // Render cursor if focused.
        if self.focused {
            let cursor_x = area.x + 1 + self.cursor_display_offset() as u16;
            let cursor_y = area.y + 1;
            f.set_cursor_position((cursor_x, cursor_y));
        }
    }

    /// Render the slash-command suggestions popup (if any).
    pub fn render_suggestions(&self, f: &mut Frame, area: Rect, styles: &Styles) {
        if self.suggestions.is_empty() {
            return;
        }
        let lines: Vec<Line> = self
            .suggestions
            .iter()
            .map(|s| {
                Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(s, styles.info().add_modifier(Modifier::BOLD)),
                ])
            })
            .collect();

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(styles.border_focus())
            .title(Span::styled(" commands ", styles.title()));

        let p = Paragraph::new(lines).block(block);
        f.render_widget(p, area);
    }

    fn cursor_display_offset(&self) -> usize {
        // Approximate display column of cursor (assuming ASCII/monospace).
        self.buffer[..self.cursor.min(self.buffer.len())]
            .chars()
            .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0))
            .sum()
    }
}

/// The known slash commands (name, short description).
pub const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("help", "show keyboard shortcuts and commands"),
    ("run", "execute an objective"),
    ("plan", "plan without write side effects"),
    ("model", "show or change the active model"),
    ("tools", "list available tools"),
    ("sessions", "list sessions"),
    ("clear", "clear the conversation"),
    ("theme", "switch between dark and light themes"),
    ("quit", "exit chimera"),
];

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_backspace_roundtrip() {
        let mut input = InputBar::new();
        for c in "hello world".chars() {
            input.insert_char(c);
        }
        assert_eq!(input.text(), "hello world");
        assert_eq!(input.cursor, 11);
        input.cursor_end();
        for _ in 0..6 {
            input.backspace();
        }
        assert_eq!(input.text(), "hello");
    }

    #[test]
    fn cursor_movement_word() {
        let mut input = InputBar::new();
        for c in "hello world test".chars() {
            input.insert_char(c);
        }
        input.cursor_home();
        input.cursor_word_right();
        assert_eq!(input.cursor, 6); // "hello "
        input.cursor_word_right();
        assert_eq!(input.cursor, 12); // "world "
    }

    #[test]
    fn history_navigation() {
        let mut input = InputBar::new();
        input.insert_str_and_reset("first");
        input.submit();
        input.insert_str_and_reset("second");
        input.submit();

        input.history_prev();
        assert_eq!(input.text(), "second");
        input.history_prev();
        assert_eq!(input.text(), "first");
        input.history_next();
        assert_eq!(input.text(), "second");
    }

    #[test]
    fn slash_command_suggestions() {
        let mut input = InputBar::new();
        input.insert_char('/');
        input.insert_char('r');
        assert!(!input.suggestions.is_empty());
        assert!(input.suggestions.iter().any(|s| s.contains("run")));
    }

    #[test]
    fn parse_command_extracts_name_and_args() {
        let mut input = InputBar::new();
        input.insert_str_and_reset("/model gpt-4o");
        let (name, args) = input.parse_command().unwrap();
        assert_eq!(name, "model");
        assert_eq!(args, "gpt-4o");
    }

    impl InputBar {
        fn insert_str_and_reset(&mut self, s: &str) {
            self.buffer = s.to_string();
            self.cursor = s.len();
        }
    }
}
