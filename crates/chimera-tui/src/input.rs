//! chimera-tui::input — Multi-line text input with slash-command support.
//!
//! A lightweight, self-contained text editor for the TUI bottom bar. Supports
//! cursor movement, history, and slash-command autocomplete suggestions.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
    /// The unsent composition and insertion point saved while browsing history.
    history_draft: Option<(String, usize)>,
    /// Desired terminal-cell column retained across short intervening lines.
    vertical_column: Option<usize>,
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
            history_draft: None,
            vertical_column: None,
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
        self.history_pos = None;
        self.history_draft = None;
        self.vertical_column = None;
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
        self.vertical_column = None;
        self.update_suggestions();
    }

    /// Delete the character before the cursor (backspace).
    pub fn backspace(&mut self) {
        self.vertical_column = None;
        if let Some(prev) = self.buffer[..self.cursor].chars().next_back() {
            let char_start = self.cursor - prev.len_utf8();
            self.buffer.replace_range(char_start..self.cursor, "");
            self.cursor = char_start;
            self.update_suggestions();
        }
    }

    /// Delete the character at the cursor (delete).
    pub fn delete_char(&mut self) {
        self.vertical_column = None;
        if let Some(next) = self.buffer[self.cursor..].chars().next() {
            let end = self.cursor + next.len_utf8();
            self.buffer.replace_range(self.cursor..end, "");
            self.update_suggestions();
        }
    }

    /// Move cursor left one char.
    pub fn cursor_left(&mut self) {
        self.vertical_column = None;
        if let Some(prev) = self.buffer[..self.cursor].chars().next_back() {
            self.cursor -= prev.len_utf8();
        }
    }

    /// Move cursor right one char.
    pub fn cursor_right(&mut self) {
        self.vertical_column = None;
        if let Some(next) = self.buffer[self.cursor..].chars().next() {
            self.cursor += next.len_utf8();
        }
    }

    /// Move cursor to start.
    pub fn cursor_home(&mut self) {
        self.cursor = 0;
        self.vertical_column = None;
    }

    /// Move cursor to end.
    pub fn cursor_end(&mut self) {
        self.cursor = self.buffer.len();
        self.vertical_column = None;
    }

    /// Move one word left.
    pub fn cursor_word_left(&mut self) {
        self.vertical_column = None;
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
        self.vertical_column = None;
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

    /// Move within the multiline draft, retaining the desired display column.
    /// False means the first logical row was reached; the caller may browse history.
    pub fn cursor_up(&mut self) -> bool {
        let start = self.line_start();
        if start == 0 {
            return false;
        }
        let column = self
            .vertical_column
            .unwrap_or_else(|| self.cursor_display_offset());
        let end = start - 1;
        let previous = self.buffer[..end].rfind('\n').map_or(0, |i| i + 1);
        self.cursor = self.column_position(previous, end, column);
        self.vertical_column = Some(column);
        true
    }

    /// Move within the multiline draft. False means the last logical row was reached.
    pub fn cursor_down(&mut self) -> bool {
        let end = self.line_end();
        if end == self.buffer.len() {
            return false;
        }
        let column = self
            .vertical_column
            .unwrap_or_else(|| self.cursor_display_offset());
        let next = end + 1;
        let next_end = self.buffer[next..]
            .find('\n')
            .map_or(self.buffer.len(), |i| next + i);
        self.cursor = self.column_position(next, next_end, column);
        self.vertical_column = Some(column);
        true
    }

    fn line_start(&self) -> usize {
        self.buffer[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.buffer[self.cursor..]
            .find('\n')
            .map_or(self.buffer.len(), |i| self.cursor + i)
    }

    fn column_position(&self, start: usize, end: usize, column: usize) -> usize {
        let mut cells = 0;
        let mut position = start;
        for (offset, character) in self.buffer[start..end].char_indices() {
            let width = unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
            if cells + width > column {
                break;
            }
            cells += width;
            position = start + offset + character.len_utf8();
        }
        position
    }

    /// Familiar terminal composition shortcuts operate on the current logical line.
    /// Returns false for controls owned by the surrounding application.
    pub fn handle_edit_key(&mut self, key: KeyEvent) -> bool {
        if !key.modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }
        match key.code {
            KeyCode::Char('a') => self.cursor_line_home(),
            KeyCode::Char('e') => self.cursor_line_end(),
            KeyCode::Char('k') => self.kill_to_line_end(),
            KeyCode::Char('u') => self.kill_to_line_start(),
            KeyCode::Char('w') => self.delete_word_left(),
            _ => return false,
        }
        true
    }

    pub fn cursor_line_home(&mut self) {
        self.cursor = self.line_start();
        self.vertical_column = None;
    }

    pub fn cursor_line_end(&mut self) {
        self.cursor = self.line_end();
        self.vertical_column = None;
    }

    /// Delete through the end of this line; at its end, join the following line.
    pub fn kill_to_line_end(&mut self) {
        let mut end = self.line_end();
        if end == self.cursor && end < self.buffer.len() {
            end += 1;
        }
        self.buffer.replace_range(self.cursor..end, "");
        self.vertical_column = None;
        self.update_suggestions();
    }

    /// Delete back to the start of this line; at its start, join the previous line.
    pub fn kill_to_line_start(&mut self) {
        let mut start = self.line_start();
        if start == self.cursor && start > 0 {
            start -= 1;
        }
        self.buffer.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.vertical_column = None;
        self.update_suggestions();
    }

    /// Delete a preceding word without splitting international text or crossing a line.
    pub fn delete_word_left(&mut self) {
        let start = self.line_start();
        let mut boundary = self.cursor;
        let mut word_started = false;
        for (offset, character) in self.buffer[start..self.cursor].char_indices().rev() {
            if character.is_whitespace() {
                if word_started {
                    break;
                }
            } else {
                word_started = true;
            }
            boundary = start + offset;
        }
        self.buffer.replace_range(boundary..self.cursor, "");
        self.cursor = boundary;
        self.vertical_column = None;
        self.update_suggestions();
    }

    /// Clear the buffer and return the submitted text, recording to history.
    pub fn submit(&mut self) -> String {
        let text = std::mem::take(&mut self.buffer);
        self.cursor = 0;
        self.history_pos = None;
        self.history_draft = None;
        self.vertical_column = None;
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
            None => {
                self.history_draft = Some((self.buffer.clone(), self.cursor));
                self.history.len() - 1
            }
            Some(p) if p > 0 => p - 1,
            Some(p) => p,
        };
        self.history_pos = Some(pos);
        self.buffer = self.history[pos].clone();
        self.cursor = self.buffer.len();
        self.vertical_column = None;
        self.update_suggestions();
    }

    /// Navigate to next history entry (newer).
    pub fn history_next(&mut self) {
        let len = self.history.len();
        match self.history_pos {
            None => {}
            Some(p) => {
                if p + 1 >= len {
                    // Restore the exact unsent composition, including its insertion point.
                    self.history_pos = None;
                    let (draft, cursor) = self.history_draft.take().unwrap_or_default();
                    self.buffer = draft;
                    self.cursor = cursor;
                } else {
                    self.history_pos = Some(p + 1);
                    self.buffer = self.history[p + 1].clone();
                    self.cursor = self.buffer.len();
                }
            }
        }
        self.vertical_column = None;
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
        self.history_draft = None;
        self.vertical_column = None;
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
    ("run", "switch to run mode; optional text starts a task"),
    ("plan", "stay read-only; optional text starts planning"),
    ("new", "start a fresh conversation and context"),
    ("search", "find text in the conversation"),
    ("diff", "review project changes"),
    ("tools-view", "inspect earlier tool cards"),
    ("context", "set known context-window tokens, or unknown"),
    ("load-older", "load earlier saved conversation messages"),
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

    /// WHAT: recall an old request, then return to and keep editing the draft.
    /// WHY: browsing history must never discard an engineer's unsent instruction.
    /// PASS: the complete international draft returns with its original insertion point.
    #[test]
    fn ux_16_history_returns_the_unsent_draft_and_insertion_point() {
        let mut input = InputBar::new();
        input.record("an earlier request");
        input.set_buffer("Please fix café 世界");
        input.cursor_left();
        input.cursor_left();
        input.history_prev();
        input.history_next();
        input.insert_char('!');
        assert_eq!(input.text(), "Please fix café !世界");
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 5)).unwrap();
        terminal
            .draw(|frame| input.render(frame, frame.area(), &Styles::dark(), "Message"))
            .unwrap();
        // A wide glyph owns its following continuation cell. Reading every
        // stored cell as another character would insert spaces inside 世界.
        let buffer = terminal.backend().buffer();
        let mut visible = String::new();
        for y in buffer.area.y..buffer.area.bottom() {
            let mut x = buffer.area.x;
            while x < buffer.area.right() {
                let symbol = buffer[(x, y)].symbol();
                visible.push_str(symbol);
                x += unicode_width::UnicodeWidthStr::width(symbol).max(1) as u16;
            }
            visible.push('\n');
        }
        assert!(
            visible.contains("Please fix café !世界"),
            "the restored draft must be visible: {visible}"
        );
    }

    /// WHAT: move vertically through a short international line in a long draft.
    /// WHY: arrow keys should edit multiline instructions instead of recalling history.
    /// PASS: the displayed caret returns to its original column without changing the draft.
    #[test]
    fn ux_17_vertical_arrows_preserve_the_multiline_draft_and_column() {
        let mut input = InputBar::new();
        input.record("previous request");
        input.set_buffer("abcdef\né\nuvwxyz");
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 5)).unwrap();
        let mut positions = Vec::new();
        for up in [true, true, false, false] {
            assert!(
                if up {
                    input.cursor_up()
                } else {
                    input.cursor_down()
                },
                "a draft row should be available"
            );
            terminal
                .draw(|frame| input.render(frame, frame.area(), &Styles::dark(), "Message"))
                .unwrap();
            let point = terminal.get_cursor_position().unwrap();
            positions.push((point.x, point.y));
        }
        assert_eq!(
            positions,
            [(2, 2), (7, 1), (2, 2), (7, 3)],
            "the visible caret should retain its preferred column"
        );
        assert_eq!(input.text(), "abcdef\né\nuvwxyz");
    }

    /// WHAT: use Ctrl+A/E/W/U/K while composing a multiline request.
    /// WHY: familiar terminal editing must work without opening another application.
    /// PASS: each shortcut produces the expected visible text or insertion point,
    /// while preserving the preceding draft line.
    #[test]
    fn ux_18_terminal_editing_shortcuts_edit_the_current_draft_line() {
        let mut input = InputBar::new();
        input.set_buffer("prefix\nalpha beta gamma");
        let mut outcomes = Vec::new();
        for key in ['a', 'e', 'w', 'u'] {
            assert!(input.handle_edit_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL)));
            outcomes.push((key, input.text().to_owned(), input.cursor));
        }
        input.set_buffer("prefix\nalpha tail");
        input.handle_edit_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        for _ in 0..5 {
            input.cursor_right();
        }
        input.handle_edit_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        outcomes.push(('k', input.text().to_owned(), input.cursor));
        assert_eq!(
            outcomes,
            vec![
                ('a', "prefix\nalpha beta gamma".into(), 7),
                ('e', "prefix\nalpha beta gamma".into(), 23),
                ('w', "prefix\nalpha beta ".into(), 18),
                ('u', "prefix\n".into(), 7),
                ('k', "prefix\nalpha".into(), 12),
            ]
        );
    }

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
