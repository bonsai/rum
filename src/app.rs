use crate::buffer::{ClipboardContent, Position, TextBuffer};
use crate::doc::{derive_title, generate_note_id, Document, FrontMatter};
use anyhow::Result;
use chrono::Utc;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

/// 読み込み元。db = chira-ura の SQLite、dir = md/txt ディレクトリ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Dir(PathBuf),
    Db(PathBuf),
}

/// DB モードのサイドバーに出すノート
#[derive(Debug, Clone)]
pub struct NoteEntry {
    pub id: String,
    pub title: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    Command,
    Search,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchState {
    pub pattern: String,
    pub matches: Vec<Position>,
    pub current_index: Option<usize>,
}

pub struct App {
    pub source: Source,
    /// md ディレクトリ（Source::Dir の実体 / `:md` `:sync` の既定）
    pub root_dir: PathBuf,
    pub files: Vec<PathBuf>,
    pub file_index: usize,
    /// DB モードのノート一覧（サイドバー）
    pub notes: Vec<NoteEntry>,
    pub note_index: usize,

    pub doc: Document,
    pub buffer: TextBuffer,

    pub mode: Mode,
    pub visual_start: Option<Position>,

    pub command_input: String,
    pub command_cursor: usize,

    pub search_input: String,
    pub search_cursor: usize,
    pub search_state: Option<SearchState>,

    pub status_message: Option<String>,
    pub pending_operator: Option<char>, // e.g. 'd', 'y', 'g'

    pub should_quit: bool,
    pub viewport_top: usize,
}

impl App {
    pub fn new(source: Source, md_dir: PathBuf) -> Result<Self> {
        let root_dir = match &source {
            Source::Dir(p) => p.clone(),
            Source::Db(_) => md_dir,
        };
        if let Source::Dir(dir) = &source {
            if !dir.exists() {
                let _ = fs::create_dir_all(dir);
            }
        }

        let mut app = Self {
            source,
            root_dir,
            files: Vec::new(),
            file_index: 0,
            notes: Vec::new(),
            note_index: 0,
            doc: Document::new_empty(),
            buffer: TextBuffer::new(),
            mode: Mode::Normal,
            visual_start: None,
            command_input: String::new(),
            command_cursor: 0,
            search_input: String::new(),
            search_cursor: 0,
            search_state: None,
            status_message: None,
            pending_operator: None,
            should_quit: false,
            viewport_top: 0,
        };

        app.refresh();
        let initial_result = app.open_initial();
        app.apply_command_result(initial_result);

        Ok(app)
    }

    /// 読み込み元に応じて一覧を取り直す
    pub fn refresh(&mut self) {
        if matches!(self.source, Source::Db(_)) {
            self.refresh_notes();
        } else {
            self.refresh_file_list();
        }
    }

    fn open_initial(&mut self) -> Result<String, String> {
        if matches!(self.source, Source::Db(_)) {
            if !self.notes.is_empty() {
                self.open_note_at_index(0)
            } else {
                Ok(self.new_note())
            }
        } else if !self.files.is_empty() {
            self.open_file_at_index(0)
        } else {
            Ok(self.new_note())
        }
    }

    /// DB からノート一覧を読み出す（副作用なし）。
    fn load_notes(path: &Path) -> anyhow::Result<Vec<NoteEntry>> {
        let conn = Connection::open(path)?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS notes (\
             id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '', \
             content TEXT NOT NULL DEFAULT '', updatedAt INTEGER NOT NULL DEFAULT 0)",
            rusqlite::params![],
        )?;
        let mut stmt = conn
            .prepare("SELECT id, title, updatedAt FROM notes ORDER BY updatedAt DESC, id DESC")?;
        let rows = stmt.query_map(rusqlite::params![], |r| {
            Ok(NoteEntry {
                id: r.get(0)?,
                title: r.get(1)?,
                updated_at: r.get(2)?,
            })
        })?;
        let mut notes = Vec::new();
        for n in rows {
            notes.push(n?);
        }
        Ok(notes)
    }

    /// ディレクトリ内の md/txt ファイルを収集する（副作用なし）。
    fn collect_files(dir: &Path) -> Vec<PathBuf> {
        let mut list = Vec::new();
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let ext = path
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if ext == "md" || ext == "txt" {
                        list.push(path);
                    }
                }
            }
        }
        list.sort();
        list
    }

    /// DB（~/.chira/ura.db 等）のノート一覧を新しい順に読む
    pub fn refresh_notes(&mut self) {
        self.notes.clear();
        let db_path = match &self.source {
            Source::Db(p) => Some(p.clone()),
            _ => None,
        };
        if let Some(path) = db_path {
            match Self::load_notes(&path) {
                Ok(notes) => self.notes = notes,
                Err(e) => {
                    self.status_message = Some(format!("db open failed: {}: {e}", path.display()));
                }
            }
        }
        if self.note_index >= self.notes.len() {
            self.note_index = self.notes.len().saturating_sub(1);
        }
    }

    /// DB の 1 ノートを開いて本文をバッファに載せる
    pub fn open_note_at_index(&mut self, idx: usize) -> Result<String, String> {
        if idx >= self.notes.len() {
            return Ok(String::new());
        }
        let entry = self.notes[idx].clone();
        let db_path = match &self.source {
            Source::Db(p) => p.clone(),
            _ => return Err("not in db mode".to_string()),
        };
        let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
        let content: String = conn
            .query_row(
                "SELECT content FROM notes WHERE id = ?1",
                rusqlite::params![&entry.id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        self.buffer = TextBuffer::from_string(&content);
        self.doc = Document {
            path: None,
            front_matter: Some(FrontMatter {
                id: entry.id.clone(),
                title: entry.title.clone(),
                updated_at: entry.updated_at,
                extra: BTreeMap::new(),
            }),
            body: content,
            modified: false,
        };
        self.note_index = idx;
        self.viewport_top = 0;
        Ok(format!("\"{}\" opened", entry.title))
    }

    /// DB モードで保存（upsert）
    fn save_note(&mut self) -> Result<String, String> {
        let now = Utc::now().timestamp_millis();
        let body = self.doc.body.clone();
        let fm = self.doc.front_matter.get_or_insert_with(|| FrontMatter {
            id: generate_note_id(now),
            title: String::new(),
            updated_at: now,
            extra: BTreeMap::new(),
        });
        if fm.id.is_empty() {
            fm.id = generate_note_id(now);
        }
        fm.title = derive_title(&body);
        fm.updated_at = now;
        let id = fm.id.clone();
        let title = fm.title.clone();

        let db_path = match &self.source {
            Source::Db(p) => p.clone(),
            _ => return Err("not in db mode".to_string()),
        };
        let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO notes (id, title, content, updatedAt) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(id) DO UPDATE SET title = excluded.title, \
             content = excluded.content, updatedAt = excluded.updatedAt",
            rusqlite::params![id, title, body, now],
        )
        .map_err(|e| e.to_string())?;
        self.doc.modified = false;
        self.refresh_notes();
        if let Some(pos) = self.notes.iter().position(|n| n.id == id) {
            self.note_index = pos;
        }
        Ok(format!("\"{}\" saved to db", title))
    }

    pub fn refresh_file_list(&mut self) {
        self.files = Self::collect_files(&self.root_dir);
    }

    pub fn open_file_at_index(&mut self, idx: usize) -> Result<String, String> {
        if idx >= self.files.len() {
            return Ok(String::new());
        }
        let target = self.files[idx].clone();
        match Document::load_from_file(&target) {
            Ok(doc) => {
                self.buffer = TextBuffer::from_string(&doc.body);
                self.doc = doc;
                self.file_index = idx;
                Ok(format!(
                    "\"{}\" opened",
                    target.file_name().unwrap().to_string_lossy()
                ))
            }
            Err(_) => Err("Failed to read file".to_string()),
        }
    }

    pub fn open_file_path<P: AsRef<Path>>(&mut self, path: P) -> Result<String, String> {
        let full_path = if path.as_ref().is_relative() {
            self.root_dir.join(path)
        } else {
            path.as_ref().to_path_buf()
        };

        if full_path.exists() {
            let doc = Document::load_from_file(&full_path).map_err(|e| e.to_string())?;
            self.buffer = TextBuffer::from_string(&doc.body);
            self.doc = doc;
            self.refresh_file_list();
            if let Some(pos) = self.files.iter().position(|p| p == &full_path) {
                self.file_index = pos;
            }
            Ok(format!(
                "\"{}\" opened",
                full_path.file_name().unwrap().to_string_lossy()
            ))
        } else {
            // Create a new document with this path
            let mut doc = Document::new_empty();
            doc.path = Some(full_path.clone());
            let ext = full_path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext == "md" {
                doc.ensure_front_matter();
            }
            self.buffer = TextBuffer::new();
            self.doc = doc;
            self.refresh_file_list();
            Ok(format!(
                "\"{}\" [New file]",
                full_path.file_name().unwrap().to_string_lossy()
            ))
        }
    }

    pub fn new_note(&mut self) -> String {
        let mut doc = Document::new_empty();
        doc.ensure_front_matter();
        self.doc = doc;
        self.buffer = TextBuffer::new();
        "New note created".to_string()
    }

    pub fn save_current_file(&mut self, target_path: Option<&str>) -> Result<String, String> {
        self.doc.body = self.buffer.to_string();

        if matches!(self.source, Source::Db(_)) {
            if let Some(p) = target_path {
                let path_buf = if Path::new(p).is_relative() {
                    self.root_dir.join(p)
                } else {
                    PathBuf::from(p)
                };
                let ext = path_buf.extension().and_then(|s| s.to_str()).unwrap_or("");
                if ext == "md" {
                    self.doc.ensure_front_matter();
                }
                self.doc.save_to(&path_buf).map_err(|e| e.to_string())?;
            }
            return self.save_note();
        }

        if let Some(p) = target_path {
            let path_buf = if Path::new(p).is_relative() {
                self.root_dir.join(p)
            } else {
                PathBuf::from(p)
            };
            let ext = path_buf.extension().and_then(|s| s.to_str()).unwrap_or("");
            if ext == "md" {
                self.doc.ensure_front_matter();
            }
            self.doc.save_to(&path_buf).map_err(|e| e.to_string())?;
        } else if self.doc.path.is_none() {
            // Generate filename based on date / note id
            let now = chrono::Utc::now().timestamp_millis();
            let name = format!("{}.md", now);
            let path_buf = self.root_dir.join(name);
            self.doc.ensure_front_matter();
            self.doc.save_to(&path_buf).map_err(|e| e.to_string())?;
        } else {
            self.doc.save().map_err(|e| e.to_string())?;
        }

        self.refresh_file_list();
        let cur_path = self
            .doc
            .path
            .as_ref()
            .ok_or_else(|| "No file name".to_string())?;
        if let Some(pos) = self.files.iter().position(|p| p == cur_path) {
            self.file_index = pos;
        }
        Ok(format!(
            "\"{}\" written",
            cur_path.file_name().unwrap().to_string_lossy()
        ))
    }

    pub fn next_file(&mut self) -> Result<String, String> {
        if matches!(self.source, Source::Db(_)) {
            if self.notes.is_empty() {
                return Err("No notes".to_string());
            }
            if self.notes.len() == 1 {
                return Err("Only one note".to_string());
            }
            let next_idx = (self.note_index + 1) % self.notes.len();
            return self.open_note_at_index(next_idx);
        }
        if self.files.is_empty() {
            return Err("No files".to_string());
        }
        if self.files.len() == 1 {
            return Err("Only one file".to_string());
        }
        let next_idx = (self.file_index + 1) % self.files.len();
        self.open_file_at_index(next_idx)
    }

    pub fn prev_file(&mut self) -> Result<String, String> {
        if matches!(self.source, Source::Db(_)) {
            if self.notes.is_empty() {
                return Err("No notes".to_string());
            }
            if self.notes.len() == 1 {
                return Err("Only one note".to_string());
            }
            let prev_idx = if self.note_index == 0 {
                self.notes.len().saturating_sub(1)
            } else {
                self.note_index - 1
            };
            return self.open_note_at_index(prev_idx);
        }
        if self.files.is_empty() {
            return Err("No files".to_string());
        }
        if self.files.len() == 1 {
            return Err("Only one file".to_string());
        }
        let prev_idx = if self.file_index == 0 {
            self.files.len().saturating_sub(1)
        } else {
            self.file_index - 1
        };
        self.open_file_at_index(prev_idx)
    }

    // --- Key Event Handler ---

    pub fn handle_key(&mut self, key: KeyEvent) {
        match self.mode {
            Mode::Normal => self.handle_normal_key(key),
            Mode::Insert => self.handle_insert_key(key),
            Mode::Visual => self.handle_visual_key(key),
            Mode::Command => self.handle_command_key(key),
            Mode::Search => self.handle_search_key(key),
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        // Clear status message on key press if not setting a new one
        self.status_message = None;

        // Check Ctrl shortcuts
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('s') => {
                    let result = self.save_current_file(None);
                    self.apply_command_result(result);
                    return;
                }
                KeyCode::Char('p') => {
                    let result = self.prev_file();
                    self.apply_command_result(result);
                    return;
                }
                KeyCode::Char('n') => {
                    let result = self.next_file();
                    self.apply_command_result(result);
                    return;
                }
                KeyCode::Char('r') => {
                    if self.buffer.redo() {
                        self.doc.modified = true;
                    }
                    return;
                }
                _ => {}
            }
        }

        // Handle pending multi-key operators
        if let Some(op) = self.pending_operator.take() {
            match (op, key.code) {
                ('d', KeyCode::Char('d')) => {
                    self.buffer.snapshot();
                    let deleted = self.buffer.delete_line();
                    self.buffer.clipboard = Some(ClipboardContent::Lines(vec![deleted]));
                    self.doc.modified = true;
                    return;
                }
                ('y', KeyCode::Char('y')) => {
                    let yanked = self.buffer.yank_line();
                    self.buffer.clipboard = Some(ClipboardContent::Lines(vec![yanked]));
                    self.status_message = Some("1 line yanked".to_string());
                    return;
                }
                ('g', KeyCode::Char('g')) => {
                    self.buffer.move_to_first_line();
                    return;
                }
                _ => {
                    // Cancel pending operator if not matching
                    return;
                }
            }
        }

        match key.code {
            KeyCode::Char(':') => {
                self.mode = Mode::Command;
                self.command_input.clear();
                self.command_cursor = 0;
            }
            KeyCode::Char('/') => {
                self.mode = Mode::Search;
                self.search_input.clear();
                self.search_cursor = 0;
            }
            KeyCode::Char('n') => {
                self.goto_next_search_match();
            }
            KeyCode::Char('N') => {
                self.goto_prev_search_match();
            }
            KeyCode::Char('v') => {
                self.mode = Mode::Visual;
                self.visual_start = Some(self.buffer.cursor);
            }
            KeyCode::Char('i') => {
                self.buffer.snapshot();
                self.mode = Mode::Insert;
            }
            KeyCode::Char('a') => {
                self.buffer.snapshot();
                self.buffer.move_right(true);
                self.mode = Mode::Insert;
            }
            KeyCode::Char('I') => {
                self.buffer.snapshot();
                self.buffer.move_to_line_start();
                self.mode = Mode::Insert;
            }
            KeyCode::Char('A') => {
                self.buffer.snapshot();
                self.buffer.move_to_line_end(true);
                self.mode = Mode::Insert;
            }
            KeyCode::Char('o') => {
                self.buffer.snapshot();
                self.buffer.open_line_below();
                self.doc.modified = true;
                self.mode = Mode::Insert;
            }
            KeyCode::Char('O') => {
                self.buffer.snapshot();
                self.buffer.open_line_above();
                self.doc.modified = true;
                self.mode = Mode::Insert;
            }
            KeyCode::Char('x') => {
                self.buffer.snapshot();
                if let Some(c) = self.buffer.delete_char_under_cursor() {
                    self.buffer.clipboard = Some(ClipboardContent::Chars(c.to_string()));
                    self.doc.modified = true;
                }
            }
            KeyCode::Char('d') => {
                self.pending_operator = Some('d');
            }
            KeyCode::Char('y') => {
                self.pending_operator = Some('y');
            }
            KeyCode::Char('p') => {
                if let Some(clip) = self.buffer.clipboard.clone() {
                    self.buffer.snapshot();
                    self.buffer.paste(&clip);
                    self.doc.modified = true;
                }
            }
            KeyCode::Char('u') => {
                if self.buffer.undo() {
                    self.doc.modified = true;
                }
            }
            KeyCode::Char('g') => {
                self.pending_operator = Some('g');
            }
            KeyCode::Char('G') => {
                self.buffer.move_to_last_line();
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.buffer.move_left();
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.buffer.move_right(false);
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.buffer.move_down();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.buffer.move_up();
            }
            KeyCode::Char('w') => {
                self.buffer.move_word_forward();
            }
            KeyCode::Char('b') => {
                self.buffer.move_word_backward();
            }
            KeyCode::Char('0') => {
                self.buffer.move_to_line_start();
            }
            KeyCode::Char('$') => {
                self.buffer.move_to_line_end(false);
            }
            _ => {}
        }
    }

    fn handle_insert_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            let result = self.save_current_file(None);
            self.apply_command_result(result);
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.buffer.clamp_cursor(false);
            }
            KeyCode::Char(c) => {
                self.buffer.insert_char(c);
                self.doc.modified = true;
            }
            KeyCode::Enter => {
                self.buffer.insert_newline();
                self.doc.modified = true;
            }
            KeyCode::Backspace => {
                self.buffer.delete_char_before_cursor();
                self.doc.modified = true;
            }
            KeyCode::Left => {
                self.buffer.move_left();
            }
            KeyCode::Right => {
                self.buffer.move_right(true);
            }
            KeyCode::Up => {
                self.buffer.move_up();
            }
            KeyCode::Down => {
                self.buffer.move_down();
            }
            _ => {}
        }
    }

    fn handle_visual_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.visual_start = None;
            }
            KeyCode::Char('y') => {
                self.yank_visual();
                self.mode = Mode::Normal;
                self.visual_start = None;
            }
            KeyCode::Char('d') | KeyCode::Char('x') => {
                self.delete_visual();
                self.mode = Mode::Normal;
                self.visual_start = None;
                self.doc.modified = true;
            }
            KeyCode::Char('h') | KeyCode::Left => self.buffer.move_left(),
            KeyCode::Char('l') | KeyCode::Right => self.buffer.move_right(false),
            KeyCode::Char('j') | KeyCode::Down => self.buffer.move_down(),
            KeyCode::Char('k') | KeyCode::Up => self.buffer.move_up(),
            KeyCode::Char('w') => self.buffer.move_word_forward(),
            KeyCode::Char('b') => self.buffer.move_word_backward(),
            KeyCode::Char('0') => self.buffer.move_to_line_start(),
            KeyCode::Char('$') => self.buffer.move_to_line_end(false),
            _ => {}
        }
    }

    fn handle_command_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.command_input.clear();
            }
            KeyCode::Enter => {
                let cmd = std::mem::take(&mut self.command_input);
                self.mode = Mode::Normal;
                self.execute_command(&cmd);
            }
            KeyCode::Char(c) => {
                self.command_input.insert(self.command_cursor, c);
                self.command_cursor += 1;
            }
            KeyCode::Backspace => {
                if self.command_cursor > 0 {
                    self.command_input.remove(self.command_cursor - 1);
                    self.command_cursor -= 1;
                } else if self.command_input.is_empty() {
                    self.mode = Mode::Normal;
                }
            }
            KeyCode::Left => {
                if self.command_cursor > 0 {
                    self.command_cursor -= 1;
                }
            }
            KeyCode::Right => {
                if self.command_cursor < self.command_input.len() {
                    self.command_cursor += 1;
                }
            }
            _ => {}
        }
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.search_input.clear();
            }
            KeyCode::Enter => {
                let pattern = std::mem::take(&mut self.search_input);
                self.mode = Mode::Normal;
                self.execute_search(&pattern);
            }
            KeyCode::Char(c) => {
                self.search_input.insert(self.search_cursor, c);
                self.search_cursor += 1;
            }
            KeyCode::Backspace => {
                if self.search_cursor > 0 {
                    self.search_input.remove(self.search_cursor - 1);
                    self.search_cursor -= 1;
                } else if self.search_input.is_empty() {
                    self.mode = Mode::Normal;
                }
            }
            _ => {}
        }
    }

    // --- Visual selection helpers ---

    fn get_visual_range(&self) -> (Position, Position) {
        let start = self.visual_start.unwrap_or(self.buffer.cursor);
        let end = self.buffer.cursor;
        if start.row < end.row || (start.row == end.row && start.col <= end.col) {
            (start, end)
        } else {
            (end, start)
        }
    }

    fn yank_visual(&mut self) {
        let (start, end) = self.get_visual_range();
        if start.row == end.row {
            let line = &self.buffer.lines[start.row];
            let chars: Vec<char> = line.chars().collect();
            let sub: String = chars[start.col..=end.col.min(chars.len().saturating_sub(1))]
                .iter()
                .collect();
            self.buffer.clipboard = Some(ClipboardContent::Chars(sub));
        } else {
            let mut extracted = Vec::new();
            for r in start.row..=end.row {
                let line = &self.buffer.lines[r];
                let chars: Vec<char> = line.chars().collect();
                if r == start.row {
                    let sub: String = chars[start.col..].iter().collect();
                    extracted.push(sub);
                } else if r == end.row {
                    let sub: String = chars[..=end.col.min(chars.len().saturating_sub(1))]
                        .iter()
                        .collect();
                    extracted.push(sub);
                } else {
                    extracted.push(line.clone());
                }
            }
            self.buffer.clipboard = Some(ClipboardContent::Lines(extracted));
        }
        self.status_message = Some("Visual selection yanked".to_string());
    }

    fn delete_visual(&mut self) {
        self.buffer.snapshot();
        let (start, end) = self.get_visual_range();
        if start.row == end.row {
            let line = &mut self.buffer.lines[start.row];
            let mut chars: Vec<char> = line.chars().collect();
            let end_col = end.col.min(chars.len().saturating_sub(1));
            chars.drain(start.col..=end_col);
            *line = chars.into_iter().collect();
            self.buffer.cursor = start;
        } else {
            let start_line = &self.buffer.lines[start.row];
            let start_prefix: String = start_line.chars().take(start.col).collect();

            let end_line = &self.buffer.lines[end.row];
            let end_suffix: String = end_line.chars().skip(end.col + 1).collect();

            self.buffer.lines.drain(start.row + 1..=end.row);
            self.buffer.lines[start.row] = format!("{}{}", start_prefix, end_suffix);
            self.buffer.cursor = start;
        }
        self.buffer.clamp_cursor(false);
    }

    // --- Search helpers ---

    fn execute_search(&mut self, pattern: &str) {
        if pattern.is_empty() {
            return;
        }
        let mut matches = Vec::new();
        for (r, line) in self.buffer.lines.iter().enumerate() {
            let mut start_idx = 0;
            while let Some(byte_idx) = line[start_idx..].find(pattern) {
                let actual_byte_idx = start_idx + byte_idx;
                let char_idx = line[..actual_byte_idx].chars().count();
                matches.push(Position {
                    row: r,
                    col: char_idx,
                });
                start_idx = actual_byte_idx + pattern.len().max(1);
                if start_idx >= line.len() {
                    break;
                }
            }
        }

        if matches.is_empty() {
            self.status_message = Some(format!("Pattern not found: {}", pattern));
            self.search_state = None;
        } else {
            // Find first match after cursor
            let cur = self.buffer.cursor;
            let mut idx = 0;
            for (i, m) in matches.iter().enumerate() {
                if m.row > cur.row || (m.row == cur.row && m.col >= cur.col) {
                    idx = i;
                    break;
                }
            }
            self.buffer.cursor = matches[idx];
            self.search_state = Some(SearchState {
                pattern: pattern.to_string(),
                matches,
                current_index: Some(idx),
            });
            self.status_message = Some(format!("/{}", pattern));
        }
    }

    fn goto_next_search_match(&mut self) {
        if let Some(state) = &mut self.search_state {
            if state.matches.is_empty() {
                return;
            }
            let next_idx = match state.current_index {
                Some(i) => (i + 1) % state.matches.len(),
                None => 0,
            };
            state.current_index = Some(next_idx);
            self.buffer.cursor = state.matches[next_idx];
        }
    }

    fn goto_prev_search_match(&mut self) {
        if let Some(state) = &mut self.search_state {
            if state.matches.is_empty() {
                return;
            }
            let prev_idx = match state.current_index {
                Some(i) => {
                    if i == 0 {
                        state.matches.len().saturating_sub(1)
                    } else {
                        i - 1
                    }
                }
                None => 0,
            };
            state.current_index = Some(prev_idx);
            self.buffer.cursor = state.matches[prev_idx];
        }
    }

    // --- Command parser & execution ---

    fn apply_command_result(&mut self, result: Result<String, String>) {
        match result {
            Ok(msg) if !msg.is_empty() => self.status_message = Some(msg),
            Err(msg) => self.status_message = Some(format!("Error: {}", msg)),
            _ => self.status_message = None,
        }
    }

    fn cmd_q(&mut self) -> Result<String, String> {
        if self.doc.modified {
            Err("No write since last change (add ! to override)".to_string())
        } else {
            self.should_quit = true;
            Ok(String::new())
        }
    }

    fn cmd_wq(&mut self, arg: Option<&str>) -> Result<String, String> {
        self.save_current_file(arg)?;
        self.should_quit = true;
        Ok(String::new())
    }

    fn cmd_e(&mut self, arg: Option<&str>) -> Result<String, String> {
        let path = arg.ok_or_else(|| "Argument required for :e".to_string())?;
        if self.doc.modified {
            Err("No write since last change (add ! to override)".to_string())
        } else {
            self.open_file_path(path)
        }
    }

    fn cmd_e_bang(&mut self, arg: Option<&str>) -> Result<String, String> {
        if let Some(path) = arg {
            self.open_file_path(path)
        } else if let Some(cur_path) = self.doc.path.clone() {
            // 再読み込み（e!）は disk 上の現物を読み直す。無ければ新規作成せずエラー。
            if !cur_path.exists() {
                return Err(format!(
                    "No such file: {}",
                    cur_path.file_name().unwrap().to_string_lossy()
                ));
            }
            self.open_file_path(cur_path)?;
            Ok("Reloaded".to_string())
        } else {
            Err("No file name".to_string())
        }
    }

    fn cmd_new(&mut self) -> Result<String, String> {
        if self.doc.modified {
            Err("No write since last change (save or discard first)".to_string())
        } else {
            Ok(self.new_note())
        }
    }

    fn cmd_db(&mut self, arg: Option<&str>) -> Result<String, String> {
        let p = arg.ok_or_else(|| "usage: :db <path>".to_string())?;
        let mut path = crate::config::expand(p);
        // 相対パスは現在の root_dir 基準。
        if path.is_relative() {
            path = self.root_dir.join(path);
        }
        if !crate::config::validate_path(&path) {
            return Err(format!("Invalid path: {}", p));
        }
        if !path.exists() {
            return Err(format!("db not found: {}", path.display()));
        }
        let notes = Self::load_notes(&path).map_err(|e| e.to_string())?;
        self.source = Source::Db(path.clone());
        self.notes = notes;
        if !self.notes.is_empty() {
            self.open_note_at_index(0)?;
            Ok(format!("db -> {}", path.display()))
        } else {
            Ok(format!("db -> {}. {}", path.display(), self.new_note()))
        }
    }

    fn cmd_dir(&mut self, arg: Option<&str>) -> Result<String, String> {
        let p = arg.ok_or_else(|| "usage: :dir <path>".to_string())?;
        let mut path = crate::config::expand(p);
        // 相対パスは現在の root_dir 基準。
        if path.is_relative() {
            path = self.root_dir.join(path);
        }
        if !crate::config::validate_path(&path) {
            return Err(format!("Invalid path: {}", p));
        }
        if !path.exists() {
            fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        }
        if !path.is_dir() {
            return Err(format!("Not a directory: {}", p));
        }
        let files = Self::collect_files(&path);
        self.root_dir = path.clone();
        self.source = Source::Dir(path.clone());
        self.files = files;
        if !self.files.is_empty() {
            self.open_file_at_index(0)?;
            Ok(format!("dir -> {}", path.display()))
        } else {
            Ok(format!("dir -> {}. {}", path.display(), self.new_note()))
        }
    }

    fn cmd_md(&mut self, arg: Option<&str>) -> Result<String, String> {
        let target_dir = arg.unwrap_or(self.root_dir.to_str().unwrap_or(""));
        let output = ProcessCommand::new("chira")
            .arg("md")
            .arg(target_dir)
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    "chira not found; try `cargo install chira-ura`".to_string()
                } else {
                    e.to_string()
                }
            })?;
        self.refresh_file_list();
        if output.status.success() {
            Ok("chira md completed".to_string())
        } else {
            let err_msg = String::from_utf8_lossy(&output.stderr);
            Err(format!("chira md failed: {}", err_msg.trim()))
        }
    }

    fn cmd_sync(&mut self, arg: Option<&str>) -> Result<String, String> {
        let target_dir = arg.unwrap_or(self.root_dir.to_str().unwrap_or(""));
        let output = ProcessCommand::new("chira")
            .arg("md")
            .arg("import")
            .arg(target_dir)
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    "chira not found; try `cargo install chira-ura`".to_string()
                } else {
                    e.to_string()
                }
            })?;
        self.refresh_file_list();
        if output.status.success() {
            Ok("chira md import completed".to_string())
        } else {
            let err_msg = String::from_utf8_lossy(&output.stderr);
            Err(format!("chira md import failed: {}", err_msg.trim()))
        }
    }

    pub fn execute_command(&mut self, command_line: &str) {
        self.mode = Mode::Normal;

        let parts: Vec<&str> = command_line.split_whitespace().collect();
        if parts.is_empty() {
            return;
        }

        let cmd = parts[0];
        let arg = parts.get(1).copied();

        let result = match cmd {
            "w" => self.save_current_file(arg),
            "q" => self.cmd_q(),
            "q!" => {
                self.should_quit = true;
                Ok(String::new())
            }
            "wq" | "x" => self.cmd_wq(arg),
            "e" => self.cmd_e(arg),
            "e!" => self.cmd_e_bang(arg),
            "new" => self.cmd_new(),
            "next" => self.next_file(),
            "prev" => self.prev_file(),
            "db" => self.cmd_db(arg),
            "dir" => self.cmd_dir(arg),
            "md" => self.cmd_md(arg),
            "sync" => self.cmd_sync(arg),
            _ => Err(format!("Unknown command: {}", cmd)),
        };
        self.apply_command_result(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn tmp_dir() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join("rum_app_tests");
        let path = base.join(format!("t-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&path);
        let _ = fs::create_dir_all(&path);
        path
    }

    fn tmp_db_path() -> PathBuf {
        tmp_dir().join("test.db")
    }

    fn make_app_dir() -> App {
        let dir = tmp_dir();
        App::new(Source::Dir(dir.clone()), dir.clone()).unwrap()
    }

    fn make_app_db() -> (App, PathBuf) {
        let db_path = tmp_db_path();
        let app = App::new(
            Source::Db(db_path.clone()),
            db_path.parent().unwrap().to_path_buf(),
        )
        .unwrap();
        (app, db_path)
    }

    fn make_app_with_db_notes() -> (App, PathBuf) {
        let db_path = tmp_db_path();
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS notes (\
             id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '', \
             content TEXT NOT NULL DEFAULT '', updatedAt INTEGER NOT NULL DEFAULT 0)",
            rusqlite::params![],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO notes (id, title, content, updatedAt) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["note-1", "First", "Body 1", 1000i64],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO notes (id, title, content, updatedAt) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["note-2", "Second", "Body 2", 2000i64],
        )
        .unwrap();
        let app = App::new(
            Source::Db(db_path.clone()),
            db_path.parent().unwrap().to_path_buf(),
        )
        .unwrap();
        (app, db_path)
    }

    fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn mode_returns_to_normal_after_command() {
        let mut app = make_app_dir();
        app.mode = Mode::Command;
        app.execute_command("unknown-cmd");
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn empty_command_leaves_mode_normal() {
        let mut app = make_app_dir();
        app.mode = Mode::Command;
        app.execute_command("   ");
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn unknown_command_reports_error() {
        let mut app = make_app_dir();
        app.execute_command("frobnicate");
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("Unknown command"));
        assert!(msg.starts_with("Error:"));
    }

    #[test]
    fn w_saves_new_file_and_reports_written() {
        let mut app = make_app_dir();
        app.buffer = TextBuffer::from_string("Hello world");
        app.doc.modified = true;
        app.execute_command("w");
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("written"));
        assert!(app.files.len() >= 1);
        assert!(!app.doc.modified);
    }

    #[test]
    fn w_saves_to_db() {
        let (mut app, db_path) = make_app_with_db_notes();
        app.buffer = TextBuffer::from_string("Updated body");
        app.doc.modified = true;
        app.execute_command("w");
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("saved to db"));
        let conn = Connection::open(&db_path).unwrap();
        let content: String = conn
            .query_row(
                "SELECT content FROM notes WHERE id = ?1",
                rusqlite::params!["note-2"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(content, "Updated body");
    }

    #[test]
    fn w_reports_failure_when_path_blocked() {
        let dir = tmp_dir();
        let blocker = dir.join("subdir");
        fs::File::create(&blocker).unwrap();
        let mut app = App::new(Source::Dir(dir.clone()), dir.clone()).unwrap();
        app.buffer = TextBuffer::from_string("content");
        app.doc.modified = true;
        app.doc.path = Some(blocker.join("file.md"));
        app.execute_command("w");
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.starts_with("Error:"));
    }

    #[test]
    fn q_sets_should_quit_when_clean() {
        let mut app = make_app_dir();
        app.doc.modified = false;
        app.execute_command("q");
        assert!(app.should_quit);
        assert!(app.status_message.is_none());
    }

    #[test]
    fn q_rejects_when_modified() {
        let mut app = make_app_dir();
        app.doc.modified = true;
        app.execute_command("q");
        assert!(!app.should_quit);
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("add ! to override"));
        assert!(msg.starts_with("Error:"));
    }

    #[test]
    fn q_bang_always_quits() {
        let mut app = make_app_dir();
        app.doc.modified = true;
        app.execute_command("q!");
        assert!(app.should_quit);
    }

    #[test]
    fn wq_saves_then_quits() {
        let mut app = make_app_dir();
        app.buffer = TextBuffer::from_string("Bye");
        app.doc.modified = true;
        app.execute_command("wq");
        assert!(app.should_quit);
        assert!(!app.doc.modified);
    }

    #[test]
    fn e_requires_argument() {
        let mut app = make_app_dir();
        app.execute_command("e");
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("Argument required"));
        assert!(msg.starts_with("Error:"));
    }

    #[test]
    fn e_rejects_when_modified() {
        let dir = tmp_dir();
        write_file(&dir, "existing.md", "# Existing\n");
        let mut app = App::new(Source::Dir(dir.clone()), dir.clone()).unwrap();
        app.buffer = TextBuffer::from_string("changed");
        app.doc.modified = true;
        app.execute_command("e existing.md");
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("add ! to override"));
        // buffer は変化せず（save 時に doc.body へ同期されるアーキテクチャ）
        assert_eq!(app.buffer.to_string(), "changed");
    }

    #[test]
    fn e_opens_file_when_clean() {
        let dir = tmp_dir();
        write_file(&dir, "existing.md", "# Existing\n");
        let mut app = App::new(Source::Dir(dir.clone()), dir.clone()).unwrap();
        app.execute_command("e existing.md");
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("opened"));
        assert_eq!(app.doc.body, "# Existing\n");
    }

    #[test]
    fn e_bang_reloads_current_file() {
        let dir = tmp_dir();
        let path = write_file(&dir, "mutated.md", "original\n");
        let mut app = App::new(Source::Dir(dir.clone()), dir.clone()).unwrap();
        app.execute_command("e mutated.md");
        app.buffer = TextBuffer::from_string("changed");
        app.doc.modified = true;
        fs::write(&path, "reloaded\n").unwrap();
        app.execute_command("e!");
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("Reloaded"));
        assert_eq!(app.doc.body, "reloaded\n");
        assert!(!app.doc.modified);
    }

    #[test]
    fn e_bang_without_path_reports_no_file_name() {
        let mut app = make_app_dir();
        app.doc.path = None;
        app.execute_command("e!");
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("No file name"));
        assert!(msg.starts_with("Error:"));
    }

    #[test]
    fn new_rejects_when_modified() {
        let mut app = make_app_dir();
        app.buffer = TextBuffer::from_string("changed");
        app.doc.modified = true;
        app.execute_command("new");
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("save or discard first"));
        assert_eq!(app.buffer.to_string(), "changed");
    }

    #[test]
    fn new_creates_note_when_clean() {
        let mut app = make_app_dir();
        app.buffer = TextBuffer::from_string("old");
        app.doc.modified = false;
        app.execute_command("new");
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("New note created"));
        assert!(app.doc.front_matter.is_some());
    }

    #[test]
    fn next_prev_empty_file_list_report_message() {
        let mut app = make_app_dir();
        app.files.clear();
        app.execute_command("next");
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("No files"));
        app.execute_command("prev");
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("No files"));
    }

    #[test]
    fn next_prev_single_file_report_message() {
        let dir = tmp_dir();
        write_file(&dir, "only.md", "only\n");
        let mut app = App::new(Source::Dir(dir.clone()), dir.clone()).unwrap();
        app.execute_command("next");
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("Only one file"));
    }

    #[test]
    fn next_prev_cycle_through_files() {
        let dir = tmp_dir();
        write_file(&dir, "a.md", "A\n");
        write_file(&dir, "b.md", "B\n");
        let mut app = App::new(Source::Dir(dir.clone()), dir.clone()).unwrap();
        app.execute_command("next");
        assert_eq!(app.doc.body, "B\n");
        app.execute_command("prev");
        assert_eq!(app.doc.body, "A\n");
    }

    #[test]
    fn next_prev_empty_db_report_message() {
        let (mut app, _) = make_app_db();
        app.notes.clear();
        app.execute_command("next");
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("No notes"));
    }

    #[test]
    fn next_prev_cycle_through_notes() {
        let (mut app, _) = make_app_with_db_notes();
        assert_eq!(app.doc.body, "Body 2");
        app.execute_command("next");
        assert_eq!(app.doc.body, "Body 1");
        app.execute_command("prev");
        assert_eq!(app.doc.body, "Body 2");
    }

    #[test]
    fn db_missing_arg_reports_usage() {
        let mut app = make_app_dir();
        let original_source = app.source.clone();
        app.execute_command("db");
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("usage: :db <path>"));
        assert_eq!(app.source, original_source);
    }

    #[test]
    fn db_invalid_path_does_not_change_source() {
        let dir = tmp_dir();
        let blocker = dir.join("blocker");
        fs::File::create(&blocker).unwrap();
        let mut app = App::new(Source::Dir(dir.clone()), dir.clone()).unwrap();
        let original_source = app.source.clone();
        app.execute_command("db blocker/inside.db");
        assert_eq!(app.source, original_source);
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.starts_with("Error:"));
    }

    #[test]
    fn db_invalid_target_does_not_change_source() {
        let dir = tmp_dir();
        let blocker = dir.join("notadb");
        fs::File::create(&blocker).unwrap();
        let mut app = App::new(Source::Dir(dir.clone()), dir.clone()).unwrap();
        let original_source = app.source.clone();
        app.execute_command("db notadb");
        assert_eq!(app.source, original_source);
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.starts_with("Error:"));
    }

    #[test]
    fn db_switches_source_and_loads_notes() {
        let (mut app, db_path) = make_app_with_db_notes();
        let dir = tmp_dir();
        let mut app = App::new(Source::Dir(dir.clone()), dir.clone()).unwrap();
        app.execute_command(&format!("db {}", db_path.display()));
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("db ->"));
        assert_eq!(app.notes.len(), 2);
        assert!(matches!(app.source, Source::Db(_)));
    }

    #[test]
    fn dir_missing_arg_reports_usage() {
        let (mut app, _) = make_app_db();
        let original_source = app.source.clone();
        app.execute_command("dir");
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("usage: :dir <path>"));
        assert_eq!(app.source, original_source);
    }

    #[test]
    fn dir_invalid_path_does_not_change_source() {
        let dir = tmp_dir();
        let blocker = dir.join("blocker");
        fs::File::create(&blocker).unwrap();
        let (mut app, _) = make_app_db();
        app.root_dir = dir.clone();
        let original_source = app.source.clone();
        app.execute_command("dir blocker/inside");
        assert_eq!(app.source, original_source);
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.starts_with("Error:"));
    }

    #[test]
    fn dir_regular_file_does_not_change_source() {
        let dir = tmp_dir();
        let file_path = write_file(&dir, "regular.md", "# x\n");
        let mut app = App::new(Source::Db(tmp_db_path()), dir.clone()).unwrap();
        let original_source = app.source.clone();
        let cmd = format!("dir {}", file_path.file_name().unwrap().to_string_lossy());
        app.execute_command(&cmd);
        assert_eq!(app.source, original_source);
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.starts_with("Error:"));
    }

    #[test]
    fn dir_creates_and_switches_to_new_directory() {
        let mut app = make_app_dir();
        let new_dir = tmp_dir().join("created");
        app.execute_command(&format!("dir {}", new_dir.display()));
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("dir ->"));
        assert!(new_dir.is_dir());
        assert!(matches!(app.source, Source::Dir(_)));
    }

    #[test]
    fn dir_switches_to_existing_directory() {
        let dir = tmp_dir();
        write_file(&dir, "x.md", "X\n");
        let mut app = make_app_dir();
        app.execute_command(&format!("dir {}", dir.display()));
        assert_eq!(app.doc.body, "X\n");
    }

    #[test]
    fn db_and_dir_do_not_check_modified() {
        let dir = tmp_dir();
        write_file(&dir, "x.md", "X\n");
        let db_path = tmp_db_path();
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS notes (\
             id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '', \
             content TEXT NOT NULL DEFAULT '', updatedAt INTEGER NOT NULL DEFAULT 0)",
            rusqlite::params![],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO notes (id, title, content, updatedAt) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["note-1", "Note", "Body", 1i64],
        )
        .unwrap();

        let mut app = App::new(Source::Dir(dir.clone()), dir.clone()).unwrap();
        app.buffer = TextBuffer::from_string("modified");
        app.doc.modified = true;
        app.execute_command(&format!("db {}", db_path.display()));
        assert!(matches!(app.source, Source::Db(_)));

        let mut app = App::new(Source::Db(db_path.clone()), dir.clone()).unwrap();
        app.buffer = TextBuffer::from_string("modified");
        app.doc.modified = true;
        app.execute_command(&format!("dir {}", dir.display()));
        assert!(matches!(app.source, Source::Dir(_)));
    }

    static PATH_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn md_missing_chira_suggests_install() {
        let _lock = PATH_LOCK.lock().unwrap();
        let original = std::env::var_os("PATH");
        let empty = tmp_dir();
        std::env::set_var("PATH", &empty);
        let mut app = make_app_dir();
        app.execute_command("md");
        if let Some(orig) = original {
            std::env::set_var("PATH", orig);
        } else {
            std::env::remove_var("PATH");
        }
        drop(_lock);
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("cargo install chira-ura"));
    }

    #[test]
    fn sync_missing_chira_suggests_install() {
        let _lock = PATH_LOCK.lock().unwrap();
        let original = std::env::var_os("PATH");
        let empty = tmp_dir();
        std::env::set_var("PATH", &empty);
        let mut app = make_app_dir();
        app.execute_command("sync");
        if let Some(orig) = original {
            std::env::set_var("PATH", orig);
        } else {
            std::env::remove_var("PATH");
        }
        drop(_lock);
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("cargo install chira-ura"));
    }

    #[test]
    fn startup_reports_first_file_opened() {
        let dir = tmp_dir();
        write_file(&dir, "x.md", "# Hello\n");
        let app = App::new(Source::Dir(dir.clone()), dir.clone()).unwrap();
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("opened"));
        assert!(msg.contains("x.md"));
    }

    #[test]
    fn startup_reports_new_note_for_empty_dir() {
        let dir = tmp_dir();
        let app = App::new(Source::Dir(dir.clone()), dir.clone()).unwrap();
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("New note created"));
    }

    #[test]
    fn startup_reports_first_db_note_opened() {
        let (app, _) = make_app_with_db_notes();
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("opened"));
        assert!(msg.contains("Second"));
    }

    #[test]
    fn startup_reports_new_note_for_empty_db() {
        let db_path = tmp_db_path();
        Connection::open(&db_path).unwrap();
        let app = App::new(
            Source::Db(db_path.clone()),
            db_path.parent().unwrap().to_path_buf(),
        )
        .unwrap();
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("New note created"));
    }

    #[test]
    fn dir_empty_reports_new_note_created() {
        let mut app = make_app_dir();
        let target = tmp_dir().join("empty-dir");
        app.execute_command(&format!("dir {}", target.display()));
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("dir ->"));
        assert!(msg.contains("New note created"));
    }

    #[test]
    fn db_empty_reports_new_note_created() {
        let mut app = make_app_dir();
        let db_path = tmp_db_path();
        Connection::open(&db_path).unwrap();
        app.execute_command(&format!("db {}", db_path.display()));
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.contains("db ->"));
        assert!(msg.contains("New note created"));
    }

    #[test]
    fn next_propagates_error_when_file_is_removed() {
        let dir = tmp_dir();
        write_file(&dir, "a.md", "A\n");
        let b_path = write_file(&dir, "b.md", "B\n");
        let mut app = App::new(Source::Dir(dir.clone()), dir.clone()).unwrap();
        fs::remove_file(&b_path).unwrap();
        app.execute_command("next");
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.starts_with("Error:"));
        assert!(msg.contains("Failed to read file"));
    }

    #[test]
    fn next_propagates_error_when_db_is_removed() {
        let (mut app, db_path) = make_app_with_db_notes();
        fs::remove_file(&db_path).unwrap();
        app.execute_command("next");
        let msg = app.status_message.as_ref().unwrap();
        assert!(msg.starts_with("Error:"));
    }
}

