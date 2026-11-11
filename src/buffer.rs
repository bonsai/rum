use unicode_width::UnicodeWidthChar;

/// 1枚の紙はたて24行固定
pub const MAX_SHEET_LINES: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub row: usize,
    pub col: usize, // character index in line
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardContent {
    Lines(Vec<String>),
    Chars(String),
}

#[derive(Debug, Clone)]
struct HistoryState {
    lines: Vec<String>,
    cursor: Position,
}

#[derive(Debug, Clone)]
pub struct TextBuffer {
    pub lines: Vec<String>,
    pub cursor: Position,
    history: Vec<HistoryState>,
    redo_stack: Vec<HistoryState>,
    pub clipboard: Option<ClipboardContent>,
}

impl Default for TextBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl TextBuffer {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor: Position { row: 0, col: 0 },
            history: Vec::new(),
            redo_stack: Vec::new(),
            clipboard: None,
        }
    }

    pub fn from_string(text: &str) -> Self {
        let mut lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
        if lines.is_empty() || (text.ends_with('\n') && lines.len() < MAX_SHEET_LINES) {
            lines.push(String::new());
        }
        if lines.len() > MAX_SHEET_LINES {
            lines.truncate(MAX_SHEET_LINES);
        }
        Self {
            lines,
            cursor: Position { row: 0, col: 0 },
            history: Vec::new(),
            redo_stack: Vec::new(),
            clipboard: None,
        }
    }
}

impl std::fmt::Display for TextBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.lines.join("\n"))
    }
}

impl TextBuffer {
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn snapshot(&mut self) {
        self.history.push(HistoryState {
            lines: self.lines.clone(),
            cursor: self.cursor,
        });
        self.redo_stack.clear();
        if self.history.len() > 200 {
            self.history.remove(0);
        }
    }

    pub fn undo(&mut self) -> bool {
        if let Some(prev) = self.history.pop() {
            self.redo_stack.push(HistoryState {
                lines: self.lines.clone(),
                cursor: self.cursor,
            });
            self.lines = prev.lines;
            self.cursor = prev.cursor;
            self.clamp_cursor(false);
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        if let Some(next) = self.redo_stack.pop() {
            self.history.push(HistoryState {
                lines: self.lines.clone(),
                cursor: self.cursor,
            });
            self.lines = next.lines;
            self.cursor = next.cursor;
            self.clamp_cursor(false);
            true
        } else {
            false
        }
    }

    /// Clamping cursor position. In normal mode, cursor can't be beyond last char unless line is empty.
    /// In insert mode, cursor can be one position after the last char.
    pub fn clamp_cursor(&mut self, insert_mode: bool) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        if self.cursor.row >= self.lines.len() {
            self.cursor.row = self.lines.len().saturating_sub(1);
        }
        let char_len = self.lines[self.cursor.row].chars().count();
        let max_col = if insert_mode {
            char_len
        } else if char_len == 0 {
            0
        } else {
            char_len - 1
        };

        if self.cursor.col > max_col {
            self.cursor.col = max_col;
        }
    }

    // --- Cursor motions ---

    pub fn move_left(&mut self) {
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        }
    }

    pub fn move_right(&mut self, insert_mode: bool) {
        let char_len = self.lines[self.cursor.row].chars().count();
        let max_col = if insert_mode {
            char_len
        } else if char_len == 0 {
            0
        } else {
            char_len - 1
        };
        if self.cursor.col < max_col {
            self.cursor.col += 1;
        }
    }

    pub fn move_up(&mut self) {
        if self.cursor.row > 0 {
            self.cursor.row -= 1;
            self.clamp_cursor(false);
        }
    }

    pub fn move_down(&mut self) {
        if self.cursor.row + 1 < self.lines.len() {
            self.cursor.row += 1;
            self.clamp_cursor(false);
        }
    }

    pub fn move_to_line_start(&mut self) {
        self.cursor.col = 0;
    }

    pub fn move_to_line_end(&mut self, insert_mode: bool) {
        let char_len = self.lines[self.cursor.row].chars().count();
        self.cursor.col = if insert_mode {
            char_len
        } else if char_len == 0 {
            0
        } else {
            char_len - 1
        };
    }

    pub fn move_to_first_line(&mut self) {
        self.cursor.row = 0;
        self.clamp_cursor(false);
    }

    pub fn move_to_last_line(&mut self) {
        self.cursor.row = self.lines.len().saturating_sub(1);
        self.clamp_cursor(false);
    }

    pub fn move_word_forward(&mut self) {
        let line = &self.lines[self.cursor.row];
        let chars: Vec<char> = line.chars().collect();
        let len = chars.len();
        if self.cursor.col >= len {
            if self.cursor.row + 1 < self.lines.len() {
                self.cursor.row += 1;
                self.cursor.col = 0;
            }
            return;
        }

        let mut idx = self.cursor.col;
        let is_space = |c: char| c.is_whitespace();
        let is_ident = |c: char| c.is_alphanumeric() || c == '_';

        let start_type = if is_space(chars[idx]) {
            0
        } else if is_ident(chars[idx]) {
            1
        } else {
            2
        };

        // Advance over current token type
        while idx < len {
            let t = if is_space(chars[idx]) {
                0
            } else if is_ident(chars[idx]) {
                1
            } else {
                2
            };
            if t != start_type {
                break;
            }
            idx += 1;
        }

        // Advance over any whitespace
        while idx < len && is_space(chars[idx]) {
            idx += 1;
        }

        if idx < len {
            self.cursor.col = idx;
        } else if self.cursor.row + 1 < self.lines.len() {
            self.cursor.row += 1;
            self.cursor.col = 0;
            // Skip leading whitespace on new line
            let next_chars: Vec<char> = self.lines[self.cursor.row].chars().collect();
            let mut next_idx = 0;
            while next_idx < next_chars.len() && is_space(next_chars[next_idx]) {
                next_idx += 1;
            }
            if next_idx < next_chars.len() {
                self.cursor.col = next_idx;
            }
        } else {
            self.cursor.col = len.saturating_sub(1);
        }
    }

    pub fn move_word_backward(&mut self) {
        if self.cursor.col == 0 {
            if self.cursor.row > 0 {
                self.cursor.row -= 1;
                self.cursor.col = self.lines[self.cursor.row]
                    .chars()
                    .count()
                    .saturating_sub(1);
            }
            return;
        }

        let chars: Vec<char> = self.lines[self.cursor.row].chars().collect();
        let mut idx = self.cursor.col;
        let is_space = |c: char| c.is_whitespace();
        let is_ident = |c: char| c.is_alphanumeric() || c == '_';

        idx = idx.saturating_sub(1);
        // Skip whitespace
        while idx > 0 && is_space(chars[idx]) {
            idx -= 1;
        }

        let target_type = if is_space(chars[idx]) {
            0
        } else if is_ident(chars[idx]) {
            1
        } else {
            2
        };

        while idx > 0 {
            let prev_c = chars[idx - 1];
            let t = if is_space(prev_c) {
                0
            } else if is_ident(prev_c) {
                1
            } else {
                2
            };
            if t != target_type {
                break;
            }
            idx -= 1;
        }

        self.cursor.col = idx;
    }

    // --- Insert & editing operations ---

    pub fn insert_char(&mut self, c: char) {
        let line = &mut self.lines[self.cursor.row];
        let mut chars: Vec<char> = line.chars().collect();
        if self.cursor.col > chars.len() {
            self.cursor.col = chars.len();
        }
        chars.insert(self.cursor.col, c);
        *line = chars.into_iter().collect();
        self.cursor.col += 1;
    }

    pub fn insert_newline(&mut self) {
        // 1 枚 = たて 24 行。25 行目は入力できない（改行を無視）。
        if self.lines.len() >= MAX_SHEET_LINES {
            return;
        }
        let line = &self.lines[self.cursor.row];
        let chars: Vec<char> = line.chars().collect();
        let col = self.cursor.col.min(chars.len());

        let before: String = chars[..col].iter().collect();
        let after: String = chars[col..].iter().collect();

        self.lines[self.cursor.row] = before;
        self.lines.insert(self.cursor.row + 1, after);
        self.cursor.row += 1;
        self.cursor.col = 0;
    }

    pub fn delete_char_before_cursor(&mut self) {
        if self.cursor.col > 0 {
            let line = &mut self.lines[self.cursor.row];
            let mut chars: Vec<char> = line.chars().collect();
            chars.remove(self.cursor.col - 1);
            *line = chars.into_iter().collect();
            self.cursor.col -= 1;
        } else if self.cursor.row > 0 {
            // Join with previous line
            let current = self.lines.remove(self.cursor.row);
            self.cursor.row -= 1;
            let prev_len = self.lines[self.cursor.row].chars().count();
            self.lines[self.cursor.row].push_str(&current);
            self.cursor.col = prev_len;
        }
    }

    pub fn delete_char_under_cursor(&mut self) -> Option<char> {
        let line = &mut self.lines[self.cursor.row];
        let mut chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            return None;
        }
        if self.cursor.col < chars.len() {
            let removed = chars.remove(self.cursor.col);
            *line = chars.into_iter().collect();
            self.clamp_cursor(false);
            Some(removed)
        } else {
            None
        }
    }

    pub fn delete_line(&mut self) -> String {
        if self.lines.len() <= 1 {
            let old = std::mem::take(&mut self.lines[0]);
            self.cursor.col = 0;
            old
        } else {
            let removed = self.lines.remove(self.cursor.row);
            if self.cursor.row >= self.lines.len() {
                self.cursor.row = self.lines.len().saturating_sub(1);
            }
            self.clamp_cursor(false);
            removed
        }
    }

    pub fn yank_line(&self) -> String {
        if self.cursor.row < self.lines.len() {
            self.lines[self.cursor.row].clone()
        } else {
            String::new()
        }
    }

    pub fn paste(&mut self, clip: &ClipboardContent) {
        match clip {
            ClipboardContent::Lines(lines) => {
                // Paste below current line, respecting MAX_SHEET_LINES
                let available = MAX_SHEET_LINES.saturating_sub(self.lines.len());
                let to_insert = lines.len().min(available);
                for (i, line) in lines.iter().take(to_insert).enumerate() {
                    self.lines.insert(self.cursor.row + 1 + i, line.clone());
                }
                if to_insert > 0 {
                    self.cursor.row += 1;
                    self.cursor.col = 0;
                }
            }
            ClipboardContent::Chars(text) => {
                let line = &mut self.lines[self.cursor.row];
                let mut chars: Vec<char> = line.chars().collect();
                let insert_at = if chars.is_empty() {
                    0
                } else {
                    (self.cursor.col + 1).min(chars.len())
                };
                for (i, c) in text.chars().enumerate() {
                    chars.insert(insert_at + i, c);
                }
                *line = chars.into_iter().collect();
                self.cursor.col = insert_at + text.chars().count().saturating_sub(1);
            }
        }
        self.clamp_cursor(false);
    }

    pub fn open_line_below(&mut self) {
        if self.lines.len() >= MAX_SHEET_LINES {
            return;
        }
        self.lines.insert(self.cursor.row + 1, String::new());
        self.cursor.row += 1;
        self.cursor.col = 0;
    }

    pub fn open_line_above(&mut self) {
        if self.lines.len() >= MAX_SHEET_LINES {
            return;
        }
        self.lines.insert(self.cursor.row, String::new());
        self.cursor.col = 0;
    }

    /// Calculate visual width (column on screen) for cursor
    pub fn visual_col(&self, tab_width: usize) -> usize {
        if self.cursor.row >= self.lines.len() {
            return 0;
        }
        let line = &self.lines[self.cursor.row];
        let mut width = 0;
        for (i, c) in line.chars().enumerate() {
            if i >= self.cursor.col {
                break;
            }
            if c == '\t' {
                width += tab_width - (width % tab_width);
            } else {
                width += UnicodeWidthChar::width(c).unwrap_or(1);
            }
        }
        width
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_motions() {
        let mut buf = TextBuffer::from_string("hello world\nfoo bar baz");
        assert_eq!(buf.cursor, Position { row: 0, col: 0 });

        buf.move_right(false);
        assert_eq!(buf.cursor.col, 1);

        buf.move_word_forward();
        assert_eq!(buf.cursor.col, 6);

        buf.move_word_backward();
        assert_eq!(buf.cursor.col, 0);

        buf.move_to_line_end(false);
        assert_eq!(buf.cursor.col, 10);

        buf.move_down();
        assert_eq!(buf.cursor.row, 1);
        assert_eq!(buf.cursor.col, 10);

        buf.move_to_line_start();
        assert_eq!(buf.cursor.col, 0);
    }

    #[test]
    fn test_insert_and_undo() {
        let mut buf = TextBuffer::new();
        buf.snapshot();
        buf.insert_char('a');
        buf.insert_char('b');
        assert_eq!(buf.to_string(), "ab");

        assert!(buf.undo());
        assert_eq!(buf.to_string(), "");

        assert!(buf.redo());
        assert_eq!(buf.to_string(), "ab");
    }

    #[test]
    fn test_dd_and_p() {
        let mut buf = TextBuffer::from_string("line 1\nline 2\nline 3");
        let deleted = buf.delete_line();
        assert_eq!(deleted, "line 1");
        assert_eq!(buf.lines, vec!["line 2", "line 3"]);

        buf.paste(&ClipboardContent::Lines(vec![deleted]));
        assert_eq!(buf.lines, vec!["line 2", "line 1", "line 3"]);
    }

    #[test]
    fn test_24_lines_limit() {
        let mut lines = Vec::new();
        for i in 0..24 {
            lines.push(format!("line {}", i));
        }
        let full_text = lines.join("\n");
        let mut buf = TextBuffer::from_string(&full_text);
        assert_eq!(buf.line_count(), 24);

        // 25行目の newline は無視される
        buf.insert_newline();
        assert_eq!(buf.line_count(), 24);

        // open_line_below / open_line_above も無視される
        buf.open_line_below();
        assert_eq!(buf.line_count(), 24);
        buf.open_line_above();
        assert_eq!(buf.line_count(), 24);

        // paste も 24 行を超えない
        buf.paste(&ClipboardContent::Lines(vec!["extra".to_string()]));
        assert_eq!(buf.line_count(), 24);
    }
}
