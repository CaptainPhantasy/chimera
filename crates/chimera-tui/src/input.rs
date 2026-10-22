//! chimera-tui::input — Multi-line text input with slash-command support.
//!
//! A lightweight, self-contained text editor for the TUI bottom bar. Supports
//! cursor movement, history, and slash-command autocomplete suggestions.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use ratatui::layout::Rect;
use ratatui::style::Modifier;
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
    /// Buffer content, including newlines entered with Alt/Shift+Enter or paste.
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
    /// Command names parallel to `suggestions` (what completion inserts).
    suggestion_commands: Vec<String>,
    /// Selected suggestion index (None = none highlighted).
    pub suggestion_sel: Option<usize>,
    /// File the history persists to (one escaped entry per line; None = memory only).
    pub history_path: Option<PathBuf>,
}

/// Maximum history entries kept in memory and loaded from disk.
const HISTORY_LIMIT: usize = 100;

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
            suggestion_commands: Vec::new(),
            suggestion_sel: None,
            history_path: None,
        }
    }

    /// Attach persistent history storage at `path`, loading existing entries.
    /// Missing or unreadable files are ignored: history still works in memory.
    pub fn attach_history(&mut self, path: PathBuf) {
        self.load_history(&path);
        self.history_path = Some(path);
    }

    /// Load persisted history (newest last). Only the newest entries are kept;
    /// when the file has grown far past the limit it is compacted in place.
    fn load_history(&mut self, path: &Path) {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return;
        };
        let total = raw.lines().count();
        let newest: Vec<&str> = raw.lines().rev().take(HISTORY_LIMIT).collect();
        for line in newest.iter().rev() {
            let entry = unescape_history(line);
            if entry.trim().is_empty() {
                continue;
            }
            self.history.push_back(entry);
        }
        // The file is append-only, so it needs a periodic trim to stay small.
        if total > HISTORY_LIMIT * 2 {
            let contents: String = self
                .history
                .iter()
                .map(|entry| format!("{}\n", escape_history(entry)))
                .collect();
            let _ = std::fs::write(path, contents);
        }
    }

    /// Assign the buffer and refresh command hints, so a popup can never go
    /// stale against text that was replaced behind its back.
    pub fn set_buffer(&mut self, text: &str) {
        self.buffer = text.to_string();
        self.cursor = self.buffer.len();
        self.update_suggestions();
    }

    /// Record an entry into history and the history file (best-effort).
    pub fn record(&mut self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if self.history.back().map(String::as_str) == Some(text) {
            return;
        }
        if self.history.len() >= HISTORY_LIMIT {
            self.history.pop_front();
        }
        self.history.push_back(text.to_string());
        let Some(path) = &self.history_path else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            use std::io::Write as _;
            let _ = writeln!(file, "{}", escape_history(text));
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
        if self.buffer.len() + c.len_utf8() > 65_536 {
            return;
        }
        self.buffer.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.update_suggestions();
    }

    /// Delete the character before the cursor (backspace).
    pub fn backspace(&mut self) {
        if let Some(prev) = self.buffer[..self.cursor].chars().next_back() {
            let char_start = self.cursor - prev.len_utf8();
            self.buffer.replace_range(char_start..self.cursor, "");
            self.cursor = char_start;
            self.update_suggestions();
        }
    }

    /// Delete the character at the cursor (delete).
    pub fn delete_char(&mut self) {
        if let Some(next) = self.buffer[self.cursor..].chars().next() {
            let end = self.cursor + next.len_utf8();
            self.buffer.replace_range(self.cursor..end, "");
            self.update_suggestions();
        }
    }

    /// Move cursor left one char.
    pub fn cursor_left(&mut self) {
        if let Some(prev) = self.buffer[..self.cursor].chars().next_back() {
            self.cursor -= prev.len_utf8();
        }
    }

    /// Move cursor right one char.
    pub fn cursor_right(&mut self) {
        if let Some(next) = self.buffer[self.cursor..].chars().next() {
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
        self.record(&text);
        self.suggestions.clear();
        self.suggestion_sel = None;
        self.suggestion_commands.clear();
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
        self.update_suggestions();
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
        self.update_suggestions();
    }

    /// Move the suggestion highlight down (wraps).
    pub fn suggestion_next(&mut self) {
        if self.suggestions.is_empty() {
            return;
        }
        let len = self.suggestions.len();
        self.suggestion_sel = Some(match self.suggestion_sel {
            Some(i) => (i + 1) % len,
            None => 0,
        });
    }

    /// Move the suggestion highlight up (wraps).
    pub fn suggestion_prev(&mut self) {
        if self.suggestions.is_empty() {
            return;
        }
        let len = self.suggestions.len();
        self.suggestion_sel = Some(match self.suggestion_sel {
            Some(0) | None => len - 1,
            Some(i) => i - 1,
        });
    }

    /// Complete the buffer with the selected suggestion (or the first one).
    /// Returns true when the buffer was completed.
    pub fn accept_suggestion(&mut self) -> bool {
        if self.suggestions.is_empty() {
            return false;
        }
        let i = self.suggestion_sel.unwrap_or(0);
        let Some(name) = self.suggestion_commands.get(i) else {
            self.suggestion_sel = None;
            return false;
        };
        self.buffer = format!("/{name} ");
        self.cursor = self.buffer.len();
        self.history_pos = None;
        self.update_suggestions();
        true
    }

    /// Update slash-command suggestions based on current buffer.
    fn update_suggestions(&mut self) {
        self.suggestion_sel = None;
        self.suggestions.clear();
        self.suggestion_commands.clear();
        if !self.buffer.starts_with('/') {
            return;
        }
        // Once arguments start, the command name is chosen; stop hinting.
        if self.buffer[1..].contains(' ') {
            return;
        }
        let prefix = &self.buffer[1..];
        for (name, desc) in SLASH_COMMANDS
            .iter()
            .filter(|(name, _)| name.starts_with(prefix))
            .take(5)
        {
            self.suggestion_commands.push((*name).to_string());
            self.suggestions.push(format!("/{name} — {desc}"));
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
            styles.text_muted()
        } else {
            styles.text()
        };

        let title = if area.width >= 38 {
            " Message · Alt+Enter new line "
        } else {
            " Message "
        };
        let mut block = Block::default().border_style(border_style);
        if area.height >= 3 {
            block = block
                .borders(Borders::ALL)
                .title(Span::styled(title, styles.title()));
        }
        let inner = block.inner(area);
        let before = &self.buffer[..self.cursor];
        let row = before.chars().filter(|c| *c == '\n').count();
        let col = self.cursor_display_offset();
        let scroll_y = row.saturating_sub(inner.height.saturating_sub(1) as usize);
        let scroll_x = col.saturating_sub(inner.width.saturating_sub(1) as usize);
        let p = Paragraph::new(display_text)
            .style(text_style)
            .block(block)
            .scroll((
                scroll_y.min(u16::MAX as usize) as u16,
                scroll_x.min(u16::MAX as usize) as u16,
            ));
        f.render_widget(p, area);
        if self.focused && inner.width > 0 && inner.height > 0 {
            f.set_cursor_position((
                inner.x + (col - scroll_x) as u16,
                inner.y + (row - scroll_y) as u16,
            ));
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
            .enumerate()
            .map(|(i, s)| {
                let selected = self.suggestion_sel == Some(i);
                let marker = if selected { "▶ " } else { "  " };
                let style = if selected {
                    styles
                        .info()
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    styles.info().add_modifier(Modifier::BOLD)
                };
                Line::from(Span::styled(format!("{marker}{s}"), style))
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
        unicode_width::UnicodeWidthStr::width(
            self.buffer[..self.cursor].rsplit('\n').next().unwrap_or(""),
        )
    }
}

/// Escape a history entry for one-line file storage.
fn escape_history(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            other => out.push(other),
        }
    }
    out
}

/// Inverse of `escape_history`; unknown escapes keep their characters.
fn unescape_history(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// The known slash commands (name, short description).
pub const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("connect", "choose and connect a provider"),
    ("help", "show keyboard shortcuts and commands"),
    ("run", "execute an objective"),
    ("plan", "plan without write side effects"),
    ("setup", "choose and connect a provider"),
    ("reload", "reload saved model settings"),
    ("model", "show or change the active model"),
    ("models", "choose a model and save it as your default"),
    ("tools", "list available tools"),
    ("sessions", "browse and switch sessions"),
    ("rewind", "restore files edited by earlier turns"),
    ("permissions", "review and change permissions in one place"),
    ("memory", "browse, edit and archive saved project facts"),
    ("remember", "save a project fact after you confirm it"),
    ("skills", "browse, capture and run saved procedures"),
    ("mcp", "connect external tool servers for chat"),
    ("jobs", "list and control background work"),
    ("editor", "compose the draft in your editor"),
    ("clear", "clear the screen; keep saved context"),
    ("theme", "switch between dark and light themes"),
    ("approve", "allow the displayed action once"),
    ("deny", "reject the displayed action"),
    ("cancel", "stop current work"),
    ("quit", "exit chimera"),
    ("exit", "exit chimera"),
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

    #[test]
    fn multiline_cursor_uses_current_line_and_stays_in_bounds() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut input = InputBar::new();
        for c in "a very long first line\n世界é".chars() {
            input.insert_char(c);
        }
        assert_eq!(input.cursor_display_offset(), 5);
        input.cursor_left();
        assert_eq!(input.cursor_display_offset(), 4);
        input.backspace();
        assert_eq!(input.text(), "a very long first line\n世é");
        for (width, height) in [(1, 1), (12, 2), (40, 3), (80, 5)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|f| input.render(f, f.area(), &Styles::dark(), "Message"))
                .unwrap();
            let cursor = terminal.get_cursor_position().unwrap();
            assert!(cursor.x < width && cursor.y < height);
        }
    }

    #[test]
    fn one_row_input_displays_typing_without_a_title_row() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut input = InputBar::new();
        input.insert_char('x');
        let mut terminal = Terminal::new(TestBackend::new(2, 1)).unwrap();
        terminal
            .draw(|f| input.render(f, f.area(), &Styles::dark(), "Message"))
            .unwrap();
        assert_eq!(terminal.backend().buffer()[(0, 0)].symbol(), "x");
        assert_eq!(
            terminal.get_cursor_position().unwrap(),
            ratatui::layout::Position::new(1, 0)
        );
    }

    #[test]
    fn history_roundtrips_through_file() {
        let path = std::env::temp_dir().join(format!(
            "chimera-hist-{}-{}.txt",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_file(&path);
        let mut first = InputBar::new();
        first.attach_history(path.clone());
        first.insert_str_and_reset("plain entry");
        first.submit();
        first.insert_str_and_reset("multi\nline");
        first.submit();
        drop(first);

        let mut second = InputBar::new();
        second.attach_history(path.clone());
        assert_eq!(second.history.len(), 2, "file entries must reload");
        assert_eq!(second.history[0], "plain entry");
        assert_eq!(second.history[1], "multi\nline", "newlines must survive");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn record_skips_empty_and_consecutive_duplicates() {
        let mut input = InputBar::new();
        input.record("   ");
        assert!(input.history.is_empty());
        input.record("same");
        input.record("same");
        assert_eq!(input.history.len(), 1);
    }

    #[test]
    fn suggestion_selection_moves_and_accepts() {
        let mut input = InputBar::new();
        input.insert_char('/');
        assert!(input.suggestion_sel.is_none());
        input.suggestion_next();
        assert_eq!(input.suggestion_sel, Some(0));
        input.suggestion_next();
        assert_eq!(input.suggestion_sel, Some(1));
        input.suggestion_prev();
        assert_eq!(input.suggestion_sel, Some(0));
        assert!(input.accept_suggestion());
        assert_eq!(input.text(), "/connect ");
        assert!(input.suggestions.is_empty());
        // Editing the buffer clears any active selection.
        input.suggestion_next();
        input.backspace();
        assert!(input.suggestion_sel.is_none());
    }

    #[test]
    fn history_refreshes_command_hints() {
        let mut input = InputBar::new();
        for c in "/model".chars() {
            input.insert_char(c);
        }
        input.submit();
        input.history_prev();
        assert!(input.suggestions.iter().any(|s| s.starts_with("/model")));
        input.history_next();
        assert!(input.suggestions.is_empty());
    }

    impl InputBar {
        fn insert_str_and_reset(&mut self, s: &str) {
            self.buffer = s.to_string();
            self.cursor = s.len();
        }
    }
}
