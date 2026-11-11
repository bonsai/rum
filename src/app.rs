use crate::buffer::{ClipboardContent, Position, TextBuffer};
use crate::doc::{derive_title, generate_note_id, Document, FrontMatter};
use anyhow::Result;
use chrono::Utc;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rusqlite::{Connection, ErrorCode};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::thread;
use std::time::Duration;

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

    /// Cached SQLite connection for DB mode
    pub db_conn: Option<Connection>,
    /// Path of the currently cached SQLite connection
    pub db_conn_path: Option<PathBuf>,
}

/// Helper to check if a rusqlite Error represents a lock or busy condition.
pub fn is_sqlite_locked_or_busy(err: &rusqlite::Error) -> bool {
    match err {
        rusqlite::Error::SqliteFailure(ffi_err, _) => {
            ffi_err.code == ErrorCode::DatabaseLocked || ffi_err.code == ErrorCode::DatabaseBusy
        }
        _ => false,
    }
}

/// Generic retry helper with exponential backoff for SQLite operations experiencing lock contention.
pub fn retry_on_lock<T, F: FnMut() -> rusqlite::Result<T>>(mut op: F) -> rusqlite::Result<T> {
    let mut delay = Duration::from_millis(10);
    let max_delay = Duration::from_millis(200);
    let max_retries = 5;

    for attempt in 0..=max_retries {
        match op() {
            Ok(val) => return Ok(val),
            Err(err) if is_sqlite_locked_or_busy(&err) && attempt < max_retries => {
                thread::sleep(delay);
                delay = (delay * 2).min(max_delay);
            }
            Err(err) => return Err(err),
        }
    }
    unreachable!()
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
            db_conn: None,
            db_conn_path: None,
        };

        app.refresh();
        app.open_initial();

        Ok(app)
    }

    /// 読み込み元に応じて一覧を取り直す
    /// Retrieve or open a cached database connection for the given path, configuring busy timeout and WAL mode.
    pub fn get_db_connection(&mut self, path: &Path) -> rusqlite::Result<&mut Connection> {
        let same_path = self.db_conn_path.as_deref() == Some(path);
        if !same_path || self.db_conn.is_none() {
            self.db_conn = None;
            self.db_conn_path = None;

            let path_buf = path.to_path_buf();
            let conn = retry_on_lock(|| {
                let conn = Connection::open(&path_buf)?;
                conn.busy_timeout(Duration::from_millis(5000))?;
                let _ = conn.pragma_update(None, "journal_mode", "WAL");
                Ok(conn)
            })?;

            self.db_conn = Some(conn);
            self.db_conn_path = Some(path.to_path_buf());
        }

        Ok(self.db_conn.as_mut().expect("db_conn must be present"))
    }

    /// Reset cached DB connection (e.g. when switching database source)
    pub fn reset_db_connection(&mut self) {
        self.db_conn = None;
        self.db_conn_path = None;
    }

    /// 読み込み元に応じて一覧を取り直す
    pub fn refresh(&mut self) {
        if matches!(self.source, Source::Db(_)) {
            self.refresh_notes();
        } else {
            self.refresh_file_list();
        }
    }

    fn open_initial(&mut self) {
        if matches!(self.source, Source::Db(_)) {
            if !self.notes.is_empty() {
                self.open_note_at_index(0);
            } else {
                self.new_note();
            }
        } else if !self.files.is_empty() {
            self.open_file_at_index(0);
        } else {
            self.new_note();
        }
    }

    /// DB（~/.chira/ura.db 等）のノート一覧を新しい順に読む
    pub fn refresh_notes(&mut self) {
        self.notes.clear();
        let db_path = match &self.source {
            Source::Db(p) => Some(p.clone()),
            _ => None,
        };
        if let Some(path) = db_path {
            let res = (|| -> rusqlite::Result<Vec<NoteEntry>> {
                let conn = self.get_db_connection(&path)?;
                retry_on_lock(|| {
                    conn.execute(
                        "CREATE TABLE IF NOT EXISTS notes (\
                         id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '', \
                         content TEXT NOT NULL DEFAULT '', updatedAt INTEGER NOT NULL DEFAULT 0)",
                        rusqlite::params![],
                    )?;
                    let mut stmt = conn.prepare(
                        "SELECT id, title, updatedAt FROM notes ORDER BY updatedAt DESC, id DESC",
                    )?;
                    let rows = stmt.query_map(rusqlite::params![], |r| {
                        Ok(NoteEntry {
                            id: r.get(0)?,
                            title: r.get(1)?,
                            updated_at: r.get(2)?,
                        })
                    })?;
                    let mut entries = Vec::new();
                    for n in rows {
                        entries.push(n?);
                    }
                    Ok(entries)
                })
            })();

            match res {
                Ok(entries) => {
                    self.notes = entries;
                }
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
    pub fn open_note_at_index(&mut self, idx: usize) {
        if idx >= self.notes.len() {
            return;
        }
        let entry = self.notes[idx].clone();
        let db_path = match &self.source {
            Source::Db(p) => p.clone(),
            _ => return,
        };
        let content_res = (|| -> rusqlite::Result<String> {
            let conn = self.get_db_connection(&db_path)?;
            retry_on_lock(|| {
                conn.query_row(
                    "SELECT content FROM notes WHERE id = ?1",
                    rusqlite::params![&entry.id],
                    |r| r.get(0),
                )
            })
        })();
        let content = match content_res {
            Ok(c) => c,
            Err(e) => {
                self.status_message = Some(format!("db query failed: {e}"));
                return;
            }
        };
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
        self.status_message = Some(format!("\"{}\" opened", entry.title));
    }

    /// DB モードで保存（upsert）
    fn save_note(&mut self) -> Result<()> {
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
            _ => anyhow::bail!("not in db mode"),
        };

        let conn = self.get_db_connection(&db_path)?;
        retry_on_lock(|| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO notes (id, title, content, updatedAt) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(id) DO UPDATE SET title = excluded.title, \
                 content = excluded.content, updatedAt = excluded.updatedAt",
                rusqlite::params![&id, &title, &body, now],
            )?;
            tx.commit()?;
            Ok(())
        })?;

        self.doc.modified = false;
        self.refresh_notes();
        if let Some(pos) = self.notes.iter().position(|n| n.id == id) {
            self.note_index = pos;
        }
        self.status_message = Some(format!("\"{}\" saved to db", title));
        Ok(())
    }

    pub fn refresh_file_list(&mut self) {
        let mut list = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.root_dir) {
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
        self.files = list;
    }

    pub fn open_file_at_index(&mut self, idx: usize) {
        if idx < self.files.len() {
            let target = &self.files[idx];
            if let Ok(doc) = Document::load_from_file(target) {
                self.buffer = TextBuffer::from_string(&doc.body);
                self.doc = doc;
                self.file_index = idx;
                self.status_message = Some(format!(
                    "\"{}\" opened",
                    target.file_name().unwrap().to_string_lossy()
                ));
            } else {
                self.status_message = Some("Failed to read file".to_string());
            }
        }
    }

    pub fn open_file_path<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let full_path = if path.as_ref().is_relative() {
            self.root_dir.join(path)
        } else {
            path.as_ref().to_path_buf()
        };

        if full_path.exists() {
            let doc = Document::load_from_file(&full_path)?;
            self.buffer = TextBuffer::from_string(&doc.body);
            self.doc = doc;
            self.refresh_file_list();
            if let Some(pos) = self.files.iter().position(|p| p == &full_path) {
                self.file_index = pos;
            }
            self.status_message = Some(format!(
                "\"{}\" opened",
                full_path.file_name().unwrap().to_string_lossy()
            ));
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
            self.status_message = Some(format!(
                "\"{}\" [New file]",
                full_path.file_name().unwrap().to_string_lossy()
            ));
        }
        Ok(())
    }

    pub fn new_note(&mut self) {
        let mut doc = Document::new_empty();
        doc.ensure_front_matter();
        self.doc = doc;
        self.buffer = TextBuffer::new();
        self.status_message = Some("New note created".to_string());
    }

    pub fn save_current_file(&mut self, target_path: Option<&str>) -> Result<()> {
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
                let _ = self.doc.save_to(&path_buf);
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
            self.doc.save_to(&path_buf)?;
        } else if self.doc.path.is_none() {
            // Generate filename based on date / note id
            let now = chrono::Utc::now().timestamp_millis();
            let name = format!("{}.md", now);
            let path_buf = self.root_dir.join(name);
            self.doc.ensure_front_matter();
            self.doc.save_to(&path_buf)?;
        } else {
            self.doc.save()?;
        }

        self.refresh_file_list();
        if let Some(cur_path) = &self.doc.path {
            if let Some(pos) = self.files.iter().position(|p| p == cur_path) {
                self.file_index = pos;
            }
            self.status_message = Some(format!(
                "\"{}\" written",
                cur_path.file_name().unwrap().to_string_lossy()
            ));
        }
        Ok(())
    }

    pub fn next_file(&mut self) {
        if matches!(self.source, Source::Db(_)) {
            if self.notes.is_empty() {
                return;
            }
            let next_idx = (self.note_index + 1) % self.notes.len();
            self.open_note_at_index(next_idx);
            return;
        }
        if self.files.is_empty() {
            return;
        }
        let next_idx = (self.file_index + 1) % self.files.len();
        self.open_file_at_index(next_idx);
    }

    pub fn prev_file(&mut self) {
        if matches!(self.source, Source::Db(_)) {
            if self.notes.is_empty() {
                return;
            }
            let prev_idx = if self.note_index == 0 {
                self.notes.len().saturating_sub(1)
            } else {
                self.note_index - 1
            };
            self.open_note_at_index(prev_idx);
            return;
        }
        if self.files.is_empty() {
            return;
        }
        let prev_idx = if self.file_index == 0 {
            self.files.len().saturating_sub(1)
        } else {
            self.file_index - 1
        };
        self.open_file_at_index(prev_idx);
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
                    if let Err(e) = self.save_current_file(None) {
                        self.status_message = Some(format!("Error saving: {}", e));
                    }
                    return;
                }
                KeyCode::Char('p') => {
                    self.prev_file();
                    return;
                }
                KeyCode::Char('n') => {
                    self.next_file();
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
            if let Err(e) = self.save_current_file(None) {
                self.status_message = Some(format!("Error saving: {}", e));
            }
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

    pub fn execute_command(&mut self, command_line: &str) {
        let parts: Vec<&str> = command_line.split_whitespace().collect();
        if parts.is_empty() {
            return;
        }

        let cmd = parts[0];
        let arg = parts.get(1).copied();

        match cmd {
            "w" => {
                if let Err(e) = self.save_current_file(arg) {
                    self.status_message = Some(format!("Error: {}", e));
                }
            }
            "q" => {
                if self.doc.modified {
                    self.status_message =
                        Some("No write since last change (add ! to override)".to_string());
                } else {
                    self.should_quit = true;
                }
            }
            "q!" => {
                self.should_quit = true;
            }
            "wq" | "x" => {
                if let Err(e) = self.save_current_file(arg) {
                    self.status_message = Some(format!("Error: {}", e));
                } else {
                    self.should_quit = true;
                }
            }
            "e" => {
                if let Some(path) = arg {
                    if self.doc.modified {
                        self.status_message =
                            Some("No write since last change (add ! to override)".to_string());
                    } else if let Err(e) = self.open_file_path(path) {
                        self.status_message = Some(format!("Error: {}", e));
                    }
                } else {
                    self.status_message = Some("Argument required for :e".to_string());
                }
            }
            "e!" => {
                if let Some(path) = arg {
                    if let Err(e) = self.open_file_path(path) {
                        self.status_message = Some(format!("Error: {}", e));
                    }
                } else if let Some(cur_path) = self.doc.path.clone() {
                    let _ = self.open_file_path(cur_path);
                    self.status_message = Some("Reloaded".to_string());
                }
            }
            "new" => {
                if self.doc.modified {
                    self.status_message =
                        Some("No write since last change (save or discard first)".to_string());
                } else {
                    self.new_note();
                }
            }
            "next" => {
                self.next_file();
            }
            "prev" => {
                self.prev_file();
            }
            "db" => {
                if let Some(p) = arg {
                    let path = crate::config::expand(p);
                    if !crate::config::validate_path(&path) {
                        self.status_message = Some(format!("Invalid path: {}", p));
                    } else {
                        self.reset_db_connection();
                        self.source = Source::Db(path.clone());
                        self.refresh_notes();
                        if !self.notes.is_empty() {
                            self.open_note_at_index(0);
                        } else {
                            self.new_note();
                        }
                        self.status_message = Some(format!("db -> {}", path.display()));
                    }
                } else {
                    self.status_message = Some("usage: :db <path>".to_string());
                }
            }
            "dir" => {
                if let Some(p) = arg {
                    let path = crate::config::expand(p);
                    if !crate::config::validate_path(&path) {
                        self.status_message = Some(format!("Invalid path: {}", p));
                    } else {
                        if !path.exists() {
                            let _ = fs::create_dir_all(&path);
                        }
                        self.root_dir = path.clone();
                        self.source = Source::Dir(path.clone());
                        self.refresh_file_list();
                        if !self.files.is_empty() {
                            self.open_file_at_index(0);
                        } else {
                            self.new_note();
                        }
                        self.status_message = Some(format!("dir -> {}", path.display()));
                    }
                } else {
                    self.status_message = Some("usage: :dir <path>".to_string());
                }
            }
            "md" => {
                // External `chira md [dir]`
                let target_dir = arg.unwrap_or(self.root_dir.to_str().unwrap_or(""));
                self.status_message = Some(format!("Running chira md {}...", target_dir));
                let output = ProcessCommand::new("chira")
                    .arg("md")
                    .arg(target_dir)
                    .output();
                match output {
                    Ok(out) => {
                        self.refresh_file_list();
                        if out.status.success() {
                            self.status_message = Some("chira md completed".to_string());
                        } else {
                            let err_msg = String::from_utf8_lossy(&out.stderr);
                            self.status_message =
                                Some(format!("chira md failed: {}", err_msg.trim()));
                        }
                    }
                    Err(e) => {
                        self.status_message = Some(format!("Failed to run chira: {}", e));
                    }
                }
            }
            "sync" => {
                // External `chira md import [dir]`
                let target_dir = arg.unwrap_or(self.root_dir.to_str().unwrap_or(""));
                self.status_message = Some(format!("Running chira md import {}...", target_dir));
                let output = ProcessCommand::new("chira")
                    .arg("md")
                    .arg("import")
                    .arg(target_dir)
                    .output();
                match output {
                    Ok(out) => {
                        if out.status.success() {
                            self.status_message = Some("chira md import completed".to_string());
                        } else {
                            let err_msg = String::from_utf8_lossy(&out.stderr);
                            self.status_message =
                                Some(format!("chira md import failed: {}", err_msg.trim()));
                        }
                    }
                    Err(e) => {
                        self.status_message = Some(format!("Failed to run chira: {}", e));
                    }
                }
            }
            _ => {
                self.status_message = Some(format!("Unknown command: {}", cmd));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn test_db_save_note_transaction_atomicity() {
        let temp_dir = std::env::temp_dir().join(format!(
            "rum_test_atomicity_{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = fs::create_dir_all(&temp_dir);
        let db_path = temp_dir.join("test.db");

        let mut app = App::new(Source::Db(db_path.clone()), temp_dir.clone()).unwrap();
        app.buffer = TextBuffer::from_string("# Test Note\nHello World");
        app.save_current_file(None).unwrap();

        assert_eq!(app.notes.len(), 1);
        assert_eq!(app.notes[0].title, "# Test Note");

        // Verify direct DB query shows committed data
        let conn = Connection::open(&db_path).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM notes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let content: String = conn
            .query_row(
                "SELECT content FROM notes WHERE id = ?1",
                [&app.notes[0].id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(content, "# Test Note\nHello World");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_db_connection_reuse() {
        let temp_dir = std::env::temp_dir().join(format!(
            "rum_test_reuse_{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = fs::create_dir_all(&temp_dir);
        let db_path = temp_dir.join("test.db");

        let mut app = App::new(Source::Db(db_path.clone()), temp_dir.clone()).unwrap();
        assert!(app.db_conn.is_some());
        assert_eq!(app.db_conn_path.as_deref(), Some(db_path.as_path()));

        // Perform multiple saves and reads
        app.buffer = TextBuffer::from_string("# First Note\nBody 1");
        app.save_current_file(None).unwrap();

        app.new_note();
        app.buffer = TextBuffer::from_string("# Second Note\nBody 2");
        app.save_current_file(None).unwrap();

        assert_eq!(app.notes.len(), 2);
        // Connection is still cached
        assert!(app.db_conn.is_some());
        assert_eq!(app.db_conn_path.as_deref(), Some(db_path.as_path()));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_db_retry_on_lock_contention() {
        let temp_dir = std::env::temp_dir().join(format!(
            "rum_test_lock_{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = fs::create_dir_all(&temp_dir);
        let db_path = temp_dir.join("test.db");

        // Initialize table
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "CREATE TABLE IF NOT EXISTS notes (\
                 id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '', \
                 content TEXT NOT NULL DEFAULT '', updatedAt INTEGER NOT NULL DEFAULT 0)",
                rusqlite::params![],
            )
            .unwrap();
        }

        let locked = Arc::new(AtomicBool::new(false));
        let locked_clone = locked.clone();
        let path_clone = db_path.clone();

        // Spawn a thread that holds an EXCLUSIVE transaction lock briefly
        let handle = thread::spawn(move || {
            let mut conn = Connection::open(&path_clone).unwrap();
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive)
                .unwrap();
            tx.execute("INSERT INTO notes (id, title, content, updatedAt) VALUES ('hold', 'Locked', 'Holding lock', 0)", []).unwrap();
            locked_clone.store(true, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(50));
            tx.commit().unwrap();
        });

        // Wait until lock is acquired by the thread
        while !locked.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(5));
        }

        // Now test retry_on_lock helper with an operation that attempts an exclusive transaction
        let mut conn2 = Connection::open(&db_path).unwrap();
        conn2.busy_timeout(Duration::from_millis(10)).unwrap();

        let retry_result = retry_on_lock(|| {
            let tx = conn2.transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive)?;
            tx.execute("INSERT INTO notes (id, title, content, updatedAt) VALUES ('test2', 'Retry', 'Worked', 1)", [])?;
            tx.commit()?;
            Ok(())
        });

        handle.join().unwrap();
        assert!(retry_result.is_ok());

        let count: i64 = conn2
            .query_row("SELECT count(*) FROM notes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
