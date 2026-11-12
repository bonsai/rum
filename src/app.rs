use crate::buffer::{ClipboardContent, Position, TextBuffer};
use crate::config::{BoxConfig, Config};
use crate::doc::{derive_title, generate_note_id, is_markdown, Document, FrontMatter};
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
pub enum Pane {
    Box,
    Sheet,
    Editor,
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

/// コマンドの実行結果。分類（`dispatch_command`）と表示（`apply_outcome`）を分ける。
#[derive(Debug, Clone, PartialEq, Eq)]
enum CommandOutcome {
    /// 成功（追加のメッセージなし）
    Done,
    /// 実行結果をそのまま表示する（usage / 確認 / 成功通知）
    Notice(String),
    /// 失敗。表示は `Error: {detail}` に統一される
    Failure(String),
    /// 終了
    Quit,
}

/// DB のノート一覧を読む（接続〜クエリまで）。一覧の取得元が 1 か所になる。
fn load_notes(path: &Path) -> Result<Vec<NoteEntry>> {
    let conn = Connection::open(path)?;
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
    let mut notes = Vec::new();
    for n in rows {
        notes.push(n?);
    }
    Ok(notes)
}

/// ディレクトリ内の md/txt をパス順に集める。
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

pub struct App {
    pub config: Config,
    pub boxes: Vec<BoxConfig>,
    pub box_index: usize,
    pub focused_pane: Pane,

    pub source: Source,
    /// md ディレクトリ（Source::Dir の実体 / `:md` `:sync` の既定）
    pub root_dir: PathBuf,
    pub files: Vec<PathBuf>,
    pub file_index: usize,
    /// DB モードのノート一覧（紙ペイン）
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
    pub pending_operator: Option<char>, // e.g. 'd', 'y', 'g', ']', '['

    pub should_quit: bool,
    pub viewport_top: usize,

    /// 裏写り（チラ裏メタファ）
    pub ura_enabled: bool,
    pub prev_doc_lines: Option<Vec<String>>,
}

impl App {
    pub fn new_with_config(config: Config, source: Source, md_dir: PathBuf) -> Result<Self> {
        let boxes = config.effective_boxes();
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
            config,
            boxes,
            box_index: 0,
            focused_pane: Pane::Editor,
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
            ura_enabled: true,
            prev_doc_lines: None,
        };

        // BOX のインデックスを現在の source と一致させる
        if let Some(pos) = app
            .boxes
            .iter()
            .position(|b| match (&app.source, b.source.as_str()) {
                (Source::Db(p), "db") => &b.resolved_path() == p,
                (Source::Dir(p), "dir") => &b.resolved_path() == p,
                _ => false,
            })
        {
            app.box_index = pos;
        }

        app.refresh();
        app.open_initial();

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

    fn open_initial(&mut self) {
        let opened = if matches!(self.source, Source::Db(_)) {
            if self.notes.is_empty() {
                self.new_note();
                return;
            }
            self.open_note_at_index(0)
        } else if self.files.is_empty() {
            self.new_note();
            return;
        } else {
            self.open_file_at_index(0)
        };
        if let Err(e) = opened {
            self.status_message = Some(format!("Error: {e}"));
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
            match load_notes(&path) {
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

    /// 直前の紙（前の面）の行を取得（裏写り用）
    pub fn update_prev_doc_lines(&mut self) {
        self.prev_doc_lines = None;
        match &self.source {
            Source::Db(db_path) => {
                if self.note_index > 0 && self.note_index - 1 < self.notes.len() {
                    let prev_id = &self.notes[self.note_index - 1].id;
                    if let Ok(conn) = Connection::open(db_path) {
                        if let Ok(content) = conn.query_row(
                            "SELECT content FROM notes WHERE id = ?1",
                            rusqlite::params![prev_id],
                            |r| r.get::<_, String>(0),
                        ) {
                            self.prev_doc_lines =
                                Some(content.lines().map(|s| s.to_string()).collect());
                        }
                    }
                }
            }
            Source::Dir(_) => {
                if self.file_index > 0 && self.file_index - 1 < self.files.len() {
                    let prev_file = &self.files[self.file_index - 1];
                    if let Ok(doc) = Document::load_from_file(prev_file) {
                        self.prev_doc_lines =
                            Some(doc.body.lines().map(|s| s.to_string()).collect());
                    }
                }
            }
        }
    }

    /// DB の 1 ノートを開いて本文をバッファに載せる
    pub fn open_note_at_index(&mut self, idx: usize) -> Result<()> {
        if idx >= self.notes.len() {
            return Ok(());
        }
        let entry = self.notes[idx].clone();
        let db_path = match &self.source {
            Source::Db(p) => p.clone(),
            _ => return Ok(()),
        };
        let conn = Connection::open(&db_path)?;
        let content: String = conn.query_row(
            "SELECT content FROM notes WHERE id = ?1",
            rusqlite::params![&entry.id],
            |r| r.get(0),
        )?;
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
        self.update_prev_doc_lines();
        self.status_message = Some(format!("\"{}\" opened", entry.title));
        Ok(())
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
        let conn = Connection::open(&db_path)?;
        conn.execute(
            "INSERT INTO notes (id, title, content, updatedAt) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(id) DO UPDATE SET title = excluded.title, \
             content = excluded.content, updatedAt = excluded.updatedAt",
            rusqlite::params![id, title, body, now],
        )?;
        self.doc.modified = false;
        self.refresh_notes();
        if let Some(pos) = self.notes.iter().position(|n| n.id == id) {
            self.note_index = pos;
        }
        self.status_message = Some(format!("\"{}\" saved to db", title));
        Ok(())
    }

    pub fn refresh_file_list(&mut self) {
        self.files = collect_files(&self.root_dir);
    }

    pub fn open_file_at_index(&mut self, idx: usize) -> Result<()> {
        if idx >= self.files.len() {
            return Ok(());
        }
        let target = self.files[idx].clone();
        let doc = Document::load_from_file(&target)?;
        self.buffer = TextBuffer::from_string(&doc.body);
        self.doc = doc;
        self.file_index = idx;
        self.update_prev_doc_lines();
        self.status_message = Some(format!(
            "\"{}\" opened",
            target.file_name().unwrap().to_string_lossy()
        ));
        Ok(())
    }

    /// 引数のパスを root_dir 基準の実パスに解決する。
    fn resolve_target_path(&self, p: &str) -> PathBuf {
        if Path::new(p).is_relative() {
            self.root_dir.join(p)
        } else {
            PathBuf::from(p)
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
            if is_markdown(&full_path) {
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
        self.update_prev_doc_lines();
        self.status_message = Some("New sheet created".to_string());
    }

    /// BOX をインデックス指定で切り替える
    pub fn select_box(&mut self, idx: usize) {
        if idx >= self.boxes.len() {
            return;
        }
        self.box_index = idx;
        let b = self.boxes[idx].clone();
        let path = b.resolved_path();
        match b.source.to_ascii_lowercase().as_str() {
            "dir" => {
                if !path.exists() {
                    let _ = fs::create_dir_all(&path);
                }
                self.root_dir = path.clone();
                self.source = Source::Dir(path);
                self.refresh_file_list();
                self.file_index = 0;
                if !self.files.is_empty() {
                    self.open_file_at_index(0);
                } else {
                    self.new_note();
                }
            }
            _ => {
                self.source = Source::Db(path);
                self.refresh_notes();
                self.note_index = 0;
                if !self.notes.is_empty() {
                    self.open_note_at_index(0);
                } else {
                    self.new_note();
                }
            }
        }
        self.status_message = Some(format!("Switched to BOX: {}", b.name));
    }

    pub fn save_current_file(&mut self, target_path: Option<&str>) -> Result<()> {
        self.doc.body = self.buffer.to_string();
        let explicit_target = target_path.map(|p| self.resolve_target_path(p));

        if let Some(path) = &explicit_target {
            // 明示された `.md` 保存先は新規作成として front matter を補完する。
            if is_markdown(path) {
                self.doc.ensure_front_matter();
            }
            self.doc.save_to(path)?;
        }

        if matches!(self.source, Source::Db(_)) {
            return self.save_note();
        }

        if explicit_target.is_none() {
            if self.doc.path.is_some() {
                self.doc.save()?;
            } else {
                let now = chrono::Utc::now().timestamp_millis();
                let name = format!("{}.md", now);
                self.doc.save_to(self.root_dir.join(name))?;
            }
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

    /// バッファ差し替え前に未保存変更を拒否する。
    /// override 不可のコマンドと `!` 付きコマンドで文言が異なる。
    fn reject_if_modified(&self, override_hint: bool) -> Option<CommandOutcome> {
        if !self.doc.modified {
            return None;
        }
        let msg = if override_hint {
            "No write since last change (add ! to override)"
        } else {
            "No write since last change (save or discard first)"
        };
        Some(CommandOutcome::Notice(msg.to_string()))
    }

    pub fn next_file(&mut self) -> CommandOutcome {
        if let Some(rejection) = self.reject_if_modified(false) {
            return rejection;
        }
        if matches!(self.source, Source::Db(_)) {
            if self.notes.is_empty() {
                return CommandOutcome::Notice("No notes".to_string());
            }
            let next_idx = (self.note_index + 1) % self.notes.len();
            return match self.open_note_at_index(next_idx) {
                Ok(()) => CommandOutcome::Done,
                Err(e) => CommandOutcome::Failure(format!("cannot open note: {e}")),
            };
        }
        if self.files.is_empty() {
            return CommandOutcome::Notice("No files".to_string());
        }
        let next_idx = (self.file_index + 1) % self.files.len();
        match self.open_file_at_index(next_idx) {
            Ok(()) => CommandOutcome::Done,
            Err(e) => CommandOutcome::Failure(format!("cannot open file: {e}")),
        }
    }

    pub fn prev_file(&mut self) -> CommandOutcome {
        if let Some(rejection) = self.reject_if_modified(false) {
            return rejection;
        }
        if matches!(self.source, Source::Db(_)) {
            if self.notes.is_empty() {
                return CommandOutcome::Notice("No notes".to_string());
            }
            let prev_idx = if self.note_index == 0 {
                self.notes.len().saturating_sub(1)
            } else {
                self.note_index - 1
            };
            return match self.open_note_at_index(prev_idx) {
                Ok(()) => CommandOutcome::Done,
                Err(e) => CommandOutcome::Failure(format!("cannot open note: {e}")),
            };
        }
        if self.files.is_empty() {
            return CommandOutcome::Notice("No files".to_string());
        }
        let prev_idx = if self.file_index == 0 {
            self.files.len().saturating_sub(1)
        } else {
            self.file_index - 1
        };
        match self.open_file_at_index(prev_idx) {
            Ok(()) => CommandOutcome::Done,
            Err(e) => CommandOutcome::Failure(format!("cannot open file: {e}")),
        }
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

    pub fn next_pane(&mut self) {
        self.focused_pane = match self.focused_pane {
            Pane::Box => Pane::Sheet,
            Pane::Sheet => Pane::Editor,
            Pane::Editor => Pane::Box,
        };
    }

    pub fn prev_pane(&mut self) {
        self.focused_pane = match self.focused_pane {
            Pane::Box => Pane::Editor,
            Pane::Sheet => Pane::Box,
            Pane::Editor => Pane::Sheet,
        };
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        // Clear status message on key press if not setting a new one
        self.status_message = None;

        // Pane switching with Tab / Shift-Tab (BackTab)
        if key.code == KeyCode::Tab {
            self.next_pane();
            return;
        }
        if key.code == KeyCode::BackTab {
            self.prev_pane();
            return;
        }

        // Check Ctrl shortcuts (sheet navigation Ctrl-n / Ctrl-p)
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('s') => {
                    if let Err(e) = self.save_current_file(None) {
                        self.status_message = Some(format!("Error saving: {}", e));
                    }
                    return;
                }
                KeyCode::Char('p') => {
                    let outcome = self.prev_file();
                    self.apply_outcome(outcome);
                    return;
                }
                KeyCode::Char('n') => {
                    let outcome = self.next_file();
                    self.apply_outcome(outcome);
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
                (']', KeyCode::Char('s')) => {
                    self.next_file();
                    return;
                }
                ('[', KeyCode::Char('s')) => {
                    self.prev_file();
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
                match self.focused_pane {
                    Pane::Box => {
                        // BOX ペインで n: 箱を新規作成
                        self.mode = Mode::Command;
                        self.command_input = "box ".to_string();
                        self.command_cursor = self.command_input.len();
                        self.status_message =
                            Some("Create new box: :box <name> <db|dir> <path>".to_string());
                    }
                    Pane::Sheet => {
                        // 紙ペインで n: 紙を新規作成
                        self.new_note();
                        self.focused_pane = Pane::Editor;
                        self.mode = Mode::Insert;
                    }
                    Pane::Editor => {
                        self.goto_next_search_match();
                    }
                }
            }
            KeyCode::Char(']') => {
                self.pending_operator = Some(']');
            }
            KeyCode::Char('[') => {
                self.pending_operator = Some('[');
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
            KeyCode::Down => match self.focused_pane {
                Pane::Box => {
                    if !self.boxes.is_empty() {
                        let next = (self.box_index + 1) % self.boxes.len();
                        self.select_box(next);
                    }
                }
                Pane::Sheet => {
                    self.next_file();
                }
                Pane::Editor => {
                    self.buffer.move_down();
                }
            },
            KeyCode::Up => match self.focused_pane {
                Pane::Box => {
                    if !self.boxes.is_empty() {
                        let prev = if self.box_index == 0 {
                            self.boxes.len().saturating_sub(1)
                        } else {
                            self.box_index - 1
                        };
                        self.select_box(prev);
                    }
                }
                Pane::Sheet => {
                    self.prev_file();
                }
                Pane::Editor => {
                    self.buffer.move_up();
                }
            },
            KeyCode::Char('j') => match self.focused_pane {
                Pane::Box => {
                    if !self.boxes.is_empty() {
                        let next = (self.box_index + 1) % self.boxes.len();
                        self.select_box(next);
                    }
                }
                Pane::Sheet => {
                    self.next_file();
                }
                Pane::Editor => {
                    self.buffer.move_down();
                }
            },
            KeyCode::Char('k') => match self.focused_pane {
                Pane::Box => {
                    if !self.boxes.is_empty() {
                        let prev = if self.box_index == 0 {
                            self.boxes.len().saturating_sub(1)
                        } else {
                            self.box_index - 1
                        };
                        self.select_box(prev);
                    }
                }
                Pane::Sheet => {
                    self.prev_file();
                }
                Pane::Editor => {
                    self.buffer.move_up();
                }
            },
            KeyCode::Enter => {
                if self.focused_pane == Pane::Box || self.focused_pane == Pane::Sheet {
                    self.focused_pane = Pane::Editor;
                }
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
                self.buffer.snapshot();
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
                self.buffer.snapshot();
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
        self.buffer.commit_edit();
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

    /// コマンドの実行結果を唯一の出力点で反映する。
    fn apply_outcome(&mut self, outcome: CommandOutcome) {
        match outcome {
            CommandOutcome::Done => {}
            CommandOutcome::Notice(msg) => self.status_message = Some(msg),
            CommandOutcome::Failure(detail) => {
                self.status_message = Some(format!("Error: {detail}"));
            }
            CommandOutcome::Quit => self.should_quit = true,
        }
    }

    /// コマンドを実行する。どの結果でもモードは Normal に戻る。
    pub fn execute_command(&mut self, command_line: &str) {
        self.mode = Mode::Normal;

        let parts: Vec<&str> = command_line.split_whitespace().collect();
        if parts.is_empty() {
            return;
        }

        let cmd = parts[0];
        let arg = parts.get(1).copied();

        let outcome = self.dispatch_command(cmd, arg, &parts);
        self.apply_outcome(outcome);
    }

    fn dispatch_command(&mut self, cmd: &str, arg: Option<&str>, parts: &[&str]) -> CommandOutcome {
        match cmd {
            "w" => match self.save_current_file(arg) {
                Ok(()) => CommandOutcome::Done,
                Err(e) => CommandOutcome::Failure(e.to_string()),
            },
            "q" => {
                if let Some(rejection) = self.reject_if_modified(true) {
                    rejection
                } else {
                    CommandOutcome::Quit
                }
            }
            "q!" => CommandOutcome::Quit,
            "wq" | "x" => match self.save_current_file(arg) {
                Ok(()) => CommandOutcome::Quit,
                Err(e) => CommandOutcome::Failure(e.to_string()),
            },
            "e" => {
                let Some(path) = arg else {
                    return CommandOutcome::Notice("Argument required for :e".to_string());
                };
                if let Some(rejection) = self.reject_if_modified(true) {
                    return rejection;
                }
                match self.open_file_path(path) {
                    Ok(()) => CommandOutcome::Done,
                    Err(e) => CommandOutcome::Failure(e.to_string()),
                }
            }
            "e!" => {
                if let Some(path) = arg {
                    return match self.open_file_path(path) {
                        Ok(()) => CommandOutcome::Done,
                        Err(e) => CommandOutcome::Failure(e.to_string()),
                    };
                }
                let Some(cur_path) = self.doc.path.clone() else {
                    return CommandOutcome::Notice("No file name".to_string());
                };
                // 再読み込み（e!）は disk 上の現物を読み直す。無ければ新規作成せずエラー。
                if !cur_path.exists() {
                    return CommandOutcome::Failure(format!(
                        "No such file: {}",
                        cur_path.file_name().unwrap().to_string_lossy()
                    ));
                }
                match self.open_file_path(cur_path) {
                    Ok(()) => CommandOutcome::Notice("Reloaded".to_string()),
                    Err(e) => CommandOutcome::Failure(e.to_string()),
                }
            }
            "new" => {
                if let Some(rejection) = self.reject_if_modified(false) {
                    return rejection;
                }
                self.new_note();
                CommandOutcome::Done
            }
            "next" => self.next_file(),
            "prev" => self.prev_file(),
            "db" => self.switch_to_db(arg),
            "dir" => self.switch_to_dir(arg),
            "md" => self.run_chira(arg, false),
            "sync" => self.run_chira(arg, true),
            // PR #11: 裏写りの ON/OFF
            "set" => match arg {
                Some("ura") => {
                    self.ura_enabled = true;
                    CommandOutcome::Notice("ura (裏写り) enabled".to_string())
                }
                Some("noura") => {
                    self.ura_enabled = false;
                    CommandOutcome::Notice("ura (裏写り) disabled".to_string())
                }
                _ => CommandOutcome::Notice("usage: :set ura | :set noura".to_string()),
            },
            // PR #11: BOX を設定に追加して選ぶ（:box <name> <db|dir> <path>）
            "box" => {
                if parts.len() >= 4 {
                    let name = parts[1].to_string();
                    let kind = parts[2].to_lowercase();
                    let path_str = parts[3].to_string();
                    let is_dir = kind == "dir";
                    let new_box = BoxConfig {
                        name: name.clone(),
                        source: if is_dir {
                            "dir".to_string()
                        } else {
                            "db".to_string()
                        },
                        db: if is_dir { None } else { Some(path_str.clone()) },
                        dir: if is_dir { Some(path_str.clone()) } else { None },
                    };
                    match self.config.add_box(new_box) {
                        Ok(cfg_path) => {
                            self.boxes = self.config.boxes.clone();
                            let new_idx = self.boxes.len() - 1;
                            self.select_box(new_idx);
                            CommandOutcome::Notice(format!(
                                "Added BOX \"{}\" to {}",
                                name,
                                cfg_path.display()
                            ))
                        }
                        Err(e) => CommandOutcome::Failure(format!("failed to save config: {e}")),
                    }
                } else {
                    CommandOutcome::Notice("usage: :box <name> <db|dir> <path>".to_string())
                }
            }
            _ => CommandOutcome::Notice(format!("Unknown command: {cmd}")),
        }
    }

    /// `:db <path>` — 読み込み成功後にだけ source と一覧を差し替える。
    fn switch_to_db(&mut self, arg: Option<&str>) -> CommandOutcome {
        let Some(p) = arg else {
            return CommandOutcome::Notice("usage: :db <path>".to_string());
        };
        if let Some(rejection) = self.reject_if_modified(false) {
            return rejection;
        }
        let path = crate::config::expand(p);
        if !crate::config::validate_path(&path) {
            return CommandOutcome::Notice(format!("Invalid path: {p}"));
        }
        let notes = match load_notes(&path) {
            Ok(notes) => notes,
            Err(e) => {
                return CommandOutcome::Failure(format!(
                    "db open failed: {}: {e}",
                    path.display()
                ))
            }
        };
        self.source = Source::Db(path.clone());
        self.notes = notes;
        self.note_index = 0;
        if self.notes.is_empty() {
            self.new_note();
            return CommandOutcome::Notice(format!("db -> {}", path.display()));
        }
        if let Err(e) = self.open_note_at_index(0) {
            return CommandOutcome::Failure(format!("cannot open note: {e}"));
        }
        CommandOutcome::Notice(format!("db -> {}", path.display()))
    }

    /// `:dir <path>` — 読み込み成功後にだけ source と一覧を差し替える。
    fn switch_to_dir(&mut self, arg: Option<&str>) -> CommandOutcome {
        let Some(p) = arg else {
            return CommandOutcome::Notice("usage: :dir <path>".to_string());
        };
        if let Some(rejection) = self.reject_if_modified(false) {
            return rejection;
        }
        let path = crate::config::expand(p);
        if !crate::config::validate_path(&path) {
            return CommandOutcome::Notice(format!("Invalid path: {p}"));
        }
        if !path.exists() {
            if let Err(e) = fs::create_dir_all(&path) {
                return CommandOutcome::Failure(format!(
                    "cannot create {}: {e}",
                    path.display()
                ));
            }
        }
        if !path.is_dir() {
            return CommandOutcome::Notice(format!("Not a directory: {}", path.display()));
        }
        let files = collect_files(&path);
        self.root_dir = path.clone();
        self.source = Source::Dir(path.clone());
        self.files = files;
        self.file_index = 0;
        if self.files.is_empty() {
            self.new_note();
            return CommandOutcome::Notice(format!("dir -> {}", path.display()));
        }
        if let Err(e) = self.open_file_at_index(0) {
            return CommandOutcome::Failure(format!("cannot open file: {e}"));
        }
        CommandOutcome::Notice(format!("dir -> {}", path.display()))
    }

    /// `:md [dir]` / `:sync [dir]` — 外部 chira の実行結果を報告する。
    fn run_chira(&mut self, arg: Option<&str>, import: bool) -> CommandOutcome {
        let target_dir = match arg {
            Some(p) => p.to_string(),
            None => match self.root_dir.to_str() {
                Some(dir) => dir.to_string(),
                None => return CommandOutcome::Notice("No directory".to_string()),
            },
        };

        let (label, output) = if import {
            self.apply_outcome(CommandOutcome::Notice(format!(
                "Running chira md import {target_dir}..."
            )));
            let output = ProcessCommand::new("chira")
                .arg("md")
                .arg("import")
                .arg(&target_dir)
                .output();
            ("chira md import", output)
        } else {
            self.apply_outcome(CommandOutcome::Notice(format!(
                "Running chira md {target_dir}..."
            )));
            let output = ProcessCommand::new("chira")
                .arg("md")
                .arg(&target_dir)
                .output();
            ("chira md", output)
        };

        let out = match output {
            Ok(out) => out,
            Err(e) => return CommandOutcome::Failure(format!("failed to run chira: {e}")),
        };
        if !import {
            self.refresh_file_list();
        }
        if out.status.success() {
            CommandOutcome::Notice(format!("{label} completed"))
        } else {
            let err_msg = String::from_utf8_lossy(&out.stderr);
            CommandOutcome::Failure(format!("{} failed: {}", label, err_msg.trim()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn app_with(label: &str, content: &str) -> (App, PathBuf) {
        let dir = std::env::temp_dir().join(format!("rum-test-{}-{}", std::process::id(), label));
        let _ = fs::remove_dir_all(&dir);
        let mut app = App::new(Source::Dir(dir.clone()), dir.clone()).expect("app created");
        app.buffer = TextBuffer::from_string(content);
        app.doc.modified = false;
        (app, dir)
    }

    #[test]
    fn test_insert_mode_without_edit_consumes_no_undo_step() {
        let (mut app, dir) = app_with("insert-no-edit", "abc");

        app.handle_key(key(KeyCode::Char('i')));
        app.handle_key(key(KeyCode::Esc));

        app.handle_key(key(KeyCode::Char('u')));
        assert_eq!(app.buffer.to_string(), "abc");
        assert!(!app.doc.modified);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_insert_mode_without_edit_preserves_redo() {
        let (mut app, dir) = app_with("insert-preserve-redo", "abc");

        app.handle_key(key(KeyCode::Char('x')));
        assert_eq!(app.buffer.to_string(), "bc");

        app.handle_key(key(KeyCode::Char('u')));
        assert_eq!(app.buffer.to_string(), "abc");

        app.handle_key(key(KeyCode::Char('i')));
        app.handle_key(key(KeyCode::Esc));

        app.handle_key(ctrl('r'));
        assert_eq!(app.buffer.to_string(), "bc");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_each_insert_session_is_one_undo_step() {
        let (mut app, dir) = app_with("insert-sessions", "abc");

        app.handle_key(key(KeyCode::Char('i')));
        app.handle_key(key(KeyCode::Char('X')));
        app.handle_key(key(KeyCode::Esc));

        app.handle_key(key(KeyCode::Char('i')));
        app.handle_key(key(KeyCode::Char('Y')));
        app.handle_key(key(KeyCode::Esc));

        assert_eq!(app.buffer.to_string(), "XYabc");

        app.handle_key(key(KeyCode::Char('u')));
        assert_eq!(app.buffer.to_string(), "Xabc");

        app.handle_key(key(KeyCode::Char('u')));
        assert_eq!(app.buffer.to_string(), "abc");

        assert!(!app.buffer.undo());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_visual_mode_switch_preserves_undo_history() {
        let (mut app, dir) = app_with("visual-preserve", "abc");

        app.handle_key(key(KeyCode::Char('x')));
        assert_eq!(app.buffer.to_string(), "bc");

        app.handle_key(key(KeyCode::Char('v')));
        app.handle_key(key(KeyCode::Char('l')));
        app.handle_key(key(KeyCode::Esc));

        app.handle_key(key(KeyCode::Char('u')));
        assert_eq!(app.buffer.to_string(), "abc");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_visual_delete_is_one_undo_step() {
        let (mut app, dir) = app_with("visual-delete", "abc");

        app.handle_key(key(KeyCode::Char('v')));
        app.handle_key(key(KeyCode::Char('l')));
        app.handle_key(key(KeyCode::Char('d')));
        assert_eq!(app.buffer.to_string(), "c");

        app.handle_key(key(KeyCode::Char('u')));
        assert_eq!(app.buffer.to_string(), "abc");

        let _ = fs::remove_dir_all(&dir);
    }

    fn status(app: &App) -> &str {
        app.status_message.as_deref().unwrap_or("")
    }

    fn app_in_db(label: &str) -> (App, PathBuf) {
        let dir = std::env::temp_dir().join(format!("rum-test-{}-{}", std::process::id(), label));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        let db_path = dir.join("ura.db");
        let conn = Connection::open(&db_path).expect("db created");
        conn.execute(
            "CREATE TABLE IF NOT EXISTS notes (\
             id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '', \
             content TEXT NOT NULL DEFAULT '', updatedAt INTEGER NOT NULL DEFAULT 0)",
            rusqlite::params![],
        )
        .expect("schema");
        conn.execute(
            "INSERT INTO notes (id, title, content, updatedAt) VALUES ('n1', 'first', 'one', 1)",
            rusqlite::params![],
        )
        .expect("seed");
        drop(conn);

        let mut app = App::new(Source::Db(db_path), dir.clone()).expect("app created");
        app.doc.modified = false;
        (app, dir)
    }

    #[test]
    fn test_execute_command_always_returns_to_normal_mode() {
        let (mut app, dir) = app_with("cmd-mode-normal", "abc");
        app.doc.modified = true;

        app.mode = Mode::Command;
        app.execute_command("q");
        assert_eq!(app.mode, Mode::Normal);
        assert!(!app.should_quit);

        app.mode = Mode::Command;
        app.execute_command("");
        assert_eq!(app.mode, Mode::Normal);

        app.mode = Mode::Command;
        app.execute_command("nosuch");
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(status(&app), "Unknown command: nosuch");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_save_failure_is_reported_and_written_file_is_not() {
        let (mut app, dir) = app_with("cmd-save-failure", "abc");
        let blocker = dir.join("blocker");
        fs::write(&blocker, "x").expect("blocker");

        app.doc.modified = true;
        app.execute_command(&format!("w {}", blocker.join("note.md").display()));

        assert_eq!(status(&app).starts_with("Error: "), true, "{}", status(&app));
        assert!(!status(&app).contains("written"), "{}", status(&app));
        assert!(!blocker.join("note.md").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_quit_saves_before_quitting() {
        let (mut app, dir) = app_with("cmd-wq", "abc");
        app.doc.modified = true;
        let target = dir.join("out.md");

        app.execute_command(&format!("wq {}", target.display()));

        assert!(app.should_quit);
        assert!(fs::read_to_string(&target).expect("saved").contains("abc"));
        assert!(status(&app).ends_with("written"), "{}", status(&app));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_modified_buffer_blocks_source_switch_and_navigation() {
        let (mut app, dir) = app_with("cmd-guard", "abc");
        let original_source = app.source.clone();
        app.doc.modified = true;

        app.execute_command(&format!(
            "db {}",
            dir.join("nonexistent-rum-test.db").display()
        ));
        assert_eq!(status(&app), "No write since last change (save or discard first)");
        assert_eq!(app.source, original_source);

        app.execute_command(&format!(
            "dir {}",
            dir.join("nonexistent-rum-test-dir").display()
        ));
        assert_eq!(status(&app), "No write since last change (save or discard first)");
        assert_eq!(app.source, original_source);

        app.execute_command("next");
        assert_eq!(status(&app), "No write since last change (save or discard first)");

        app.execute_command("prev");
        assert_eq!(status(&app), "No write since last change (save or discard first)");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_failed_db_switch_keeps_previous_source_and_reports_error() {
        let (mut app, dir) = app_with("cmd-db-failure", "abc");
        let blocker = dir.join("blocker");
        fs::write(&blocker, "x").expect("blocker");
        let original_source = app.source.clone();

        app.execute_command(&format!("db {}", blocker.join("ura.db").display()));

        assert!(status(&app).starts_with("Error: "), "{}", status(&app));
        assert!(!status(&app).contains("db -> "), "{}", status(&app));
        assert_eq!(app.source, original_source);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_failed_dir_switch_keeps_previous_source_and_reports_error() {
        let (mut app, dir) = app_with("cmd-dir-failure", "abc");
        let blocker = dir.join("blocker");
        fs::write(&blocker, "x").expect("blocker");
        let original_source = app.source.clone();
        let original_root = app.root_dir.clone();

        app.execute_command(&format!("dir {}", blocker.join("sub").display()));

        assert!(status(&app).starts_with("Error: "), "{}", status(&app));
        assert!(!status(&app).contains("dir -> "), "{}", status(&app));
        assert_eq!(app.source, original_source);
        assert_eq!(app.root_dir, original_root);
        assert!(!blocker.join("sub").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_db_switch_loads_notes() {
        let (mut app, dir) = app_in_db("cmd-db-switch");
        let db_path = match &app.source {
            Source::Db(p) => p.clone(),
            Source::Dir(_) => unreachable!("db app"),
        };

        app.execute_command(&format!("db {}", db_path.display()));

        assert!(status(&app).starts_with("db -> "), "{}", status(&app));
        assert_eq!(app.notes.len(), 1);
        assert_eq!(app.buffer.to_string(), "one");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_next_on_empty_list_reports_no_files() {
        let (mut app, dir) = app_with("cmd-next-empty", "abc");
        app.files.clear();
        app.buffer = TextBuffer::from_string("abc");

        app.execute_command("next");

        assert_eq!(status(&app), "No files");
        assert_eq!(app.buffer.to_string(), "abc");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_usage_messages_leave_state_untouched() {
        let (mut app, dir) = app_with("cmd-usage", "abc");
        let original_source = app.source.clone();

        app.execute_command("db");
        assert_eq!(status(&app), "usage: :db <path>");

        app.execute_command("dir");
        assert_eq!(status(&app), "usage: :dir <path>");

        app.execute_command("e");
        assert_eq!(status(&app), "Argument required for :e");

        app.execute_command("e!");
        assert_eq!(status(&app), "No file name");

        assert_eq!(app.source, original_source);
        assert_eq!(app.buffer.to_string(), "abc");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_reload_failure_reports_error_instead_of_reloaded() {
        let (mut app, dir) = app_with("cmd-reload-failure", "abc");
        let blocker = dir.join("blocker");
        fs::write(&blocker, "x").expect("blocker");
        app.doc.path = Some(blocker.join("note.md"));

        app.execute_command("e!");

        assert!(status(&app).starts_with("Error: "), "{}", status(&app));
        assert!(!status(&app).contains("Reloaded"), "{}", status(&app));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_reload_success_reports_reloaded() {
        let (mut app, dir) = app_with("cmd-reload-ok", "abc");
        let target = dir.join("note.md");
        fs::write(&target, "on disk").expect("write");
        app.doc.path = Some(target.clone());
        app.doc.modified = true;
        app.buffer = TextBuffer::from_string("edited");

        app.execute_command("e!");

        assert_eq!(status(&app), "Reloaded");
        assert_eq!(app.buffer.to_string(), "on disk");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_ctrl_p_obeys_unsaved_guard() {
        let (mut app, dir) = app_with("cmd-ctrl-p-guard", "abc");
        fs::write(dir.join("a.md"), "a").expect("a");
        fs::write(dir.join("b.md"), "b").expect("b");
        app.refresh_file_list();
        app.open_file_at_index(0).expect("open");
        app.doc.modified = true;

        app.handle_key(ctrl('p'));

        assert_eq!(status(&app), "No write since last change (save or discard first)");
        assert_eq!(app.buffer.to_string(), "a");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_unknown_command_reports_message() {
        let (mut app, dir) = app_with("cmd-unknown", "abc");

        app.execute_command("frobnicate");

        assert_eq!(status(&app), "Unknown command: frobnicate");
        assert!(!app.should_quit);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_save_success_reports_written() {
        let (mut app, dir) = app_with("cmd-save-ok", "abc");
        app.doc.modified = true;

        app.execute_command("w");

        assert!(status(&app).ends_with("written"), "{}", status(&app));
        assert!(!status(&app).starts_with("Error: "), "{}", status(&app));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_new_txt_path_gets_no_front_matter() {
        let (mut app, dir) = app_with("fm-new-txt", "");
        let target = dir.join("note.txt");

        app.execute_command(&format!("e {}", target.display()));

        assert!(app.doc.front_matter.is_none());
        app.buffer = TextBuffer::from_string("plain body\nsecond line");
        app.doc.modified = true;
        app.execute_command("w");

        let written = fs::read_to_string(&target).expect("txt written");
        assert!(!written.contains("---"), "{written}");
        assert!(!written.contains("id: "), "{written}");
        assert!(!written.contains("title: "), "{written}");
        assert!(!written.contains("updatedAt: "), "{written}");
        assert_eq!(written, "plain body\nsecond line");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_new_md_path_gets_front_matter() {
        let (mut app, dir) = app_with("fm-new-md", "");
        let target = dir.join("note.md");

        app.execute_command(&format!("e {}", target.display()));

        assert!(app.doc.front_matter.is_some());
        app.buffer = TextBuffer::from_string("heading\n");
        app.doc.modified = true;
        app.execute_command("w");

        let written = fs::read_to_string(&target).expect("md written");
        assert!(written.starts_with("---\n"), "{written}");
        assert!(written.contains("id: note-"), "{written}");
        assert!(written.contains("title: heading"), "{written}");
        assert!(written.contains("updatedAt: "), "{written}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_to_txt_path_omits_front_matter_of_markdown_document() {
        let (mut app, dir) = app_with("fm-write-txt", "");
        let source = dir.join("note.md");
        app.execute_command(&format!("e {}", source.display()));
        assert!(app.doc.front_matter.is_some());
        app.buffer = TextBuffer::from_string("body text");
        app.doc.modified = true;

        let target = dir.join("out.txt");
        app.execute_command(&format!("w {}", target.display()));

        assert_eq!(
            fs::read_to_string(&target).expect("txt written"),
            "body text"
        );

        app.doc.modified = true;
        app.execute_command("w");

        assert_eq!(
            fs::read_to_string(&target).expect("txt rewritten"),
            "body text"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_to_md_path_adds_front_matter() {
        let (mut app, dir) = app_with("fm-write-md", "body");
        let plain = dir.join("seed.txt");
        fs::write(&plain, "body").expect("seed");
        app.execute_command(&format!("e {}", plain.display()));
        assert!(app.doc.front_matter.is_none());

        let target = dir.join("out.md");
        app.doc.modified = true;
        app.execute_command(&format!("w {}", target.display()));

        let written = fs::read_to_string(&target).expect("md written");
        assert!(written.starts_with("---\n"), "{written}");
        assert!(written.contains("id: note-"), "{written}");
        assert!(written.contains("title: body"), "{written}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_plain_txt_stays_plain_across_open_save_reload() {
        let (mut app, dir) = app_with("fm-txt-roundtrip", "");
        let target = dir.join("plain.txt");
        fs::write(&target, "plain text note\nsecond line").expect("seed");

        app.execute_command(&format!("e {}", target.display()));
        assert!(app.doc.front_matter.is_none());

        app.buffer = TextBuffer::from_string("edited body\n");
        app.doc.modified = true;
        app.execute_command("w");
        assert!(app.doc.front_matter.is_none());
        assert_eq!(
            fs::read_to_string(&target).expect("txt saved"),
            "edited body\n"
        );

        app.execute_command("e!");
        assert!(app.doc.front_matter.is_none());
        assert_eq!(app.buffer.to_string(), "edited body\n");
        assert_eq!(
            fs::read_to_string(&target).expect("txt reloaded"),
            "edited body\n"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_new_note_keeps_front_matter() {
        let (mut app, dir) = app_with("fm-new-note", "abc");

        app.execute_command("new");

        assert!(app.doc.front_matter.is_some());
        assert!(app.doc.path.is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_uppercase_md_keeps_front_matter_on_save() {
        let (mut app, dir) = app_with("fm-uppercase-md", "");
        let target = dir.join("note.MD");
        fs::write(
            &target,
            "---\nid: note-1\ntitle: T\nupdatedAt: 1\n---\nbody\n",
        )
        .expect("seed");

        app.execute_command(&format!("e {}", target.display()));
        assert!(app.doc.front_matter.is_some());
        app.buffer = TextBuffer::from_string("edited body\n");
        app.doc.modified = true;
        app.execute_command("w");

        let written = fs::read_to_string(&target).expect("md written");
        assert!(written.starts_with("---\n"), "{written}");
        assert!(written.contains("id: note-1"), "{written}");
        assert!(written.contains("title: edited body"), "{written}");
        assert!(written.contains("updatedAt: "), "{written}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_plain_md_stays_plain_on_no_argument_save() {
        let (mut app, dir) = app_with("fm-plain-md", "");
        let target = dir.join("plain.md");
        fs::write(&target, "body").expect("seed");

        app.execute_command(&format!("e {}", target.display()));
        assert!(app.doc.front_matter.is_none());

        app.buffer = TextBuffer::from_string("edited body\n");
        app.doc.modified = true;
        app.execute_command("w");
        assert_eq!(
            fs::read_to_string(&target).expect("md saved"),
            "edited body\n"
        );

        app.buffer = TextBuffer::from_string("edited again\n");
        app.doc.modified = true;
        app.handle_key(ctrl('s'));
        assert_eq!(
            fs::read_to_string(&target).expect("md saved again"),
            "edited again\n"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_to_uppercase_txt_path_stays_plain() {
        let (mut app, dir) = app_with("fm-write-upper-txt", "");
        let source = dir.join("note.md");
        app.execute_command(&format!("e {}", source.display()));
        assert!(app.doc.front_matter.is_some());
        app.buffer = TextBuffer::from_string("body text");
        app.doc.modified = true;

        let target = dir.join("out.TXT");
        app.execute_command(&format!("w {}", target.display()));
        assert_eq!(
            fs::read_to_string(&target).expect("txt written"),
            "body text"
        );

        app.execute_command("w");
        assert_eq!(
            fs::read_to_string(&target).expect("txt rewritten"),
            "body text"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_db_note_written_to_txt_path_stays_plain() {
        let (mut app, dir) = app_in_db("fm-db-write-txt");
        app.buffer = TextBuffer::from_string("one edited");
        app.doc.modified = true;

        app.execute_command("w out.txt");

        assert_eq!(fs::read_to_string(dir.join("out.txt")).expect("txt"), "one edited");

        let conn = Connection::open(dir.join("ura.db")).expect("db");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notes",
                rusqlite::params![],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(count, 1);
        let content: String = conn
            .query_row(
                "SELECT content FROM notes WHERE id = 'n1'",
                rusqlite::params![],
                |r| r.get(0),
            )
            .expect("content");
        assert_eq!(content, "one edited");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_pane_switching() {
        let dir = std::env::temp_dir().join("rum_pane_test");
        let _ = fs::create_dir_all(&dir);
        let mut app =
            App::new_with_config(Config::default(), Source::Dir(dir.clone()), dir.clone()).unwrap();

        assert_eq!(app.focused_pane, Pane::Editor);
        app.next_pane();
        assert_eq!(app.focused_pane, Pane::Box);
        app.next_pane();
        assert_eq!(app.focused_pane, Pane::Sheet);
        app.next_pane();
        assert_eq!(app.focused_pane, Pane::Editor);

        app.prev_pane();
        assert_eq!(app.focused_pane, Pane::Sheet);
        app.prev_pane();
        assert_eq!(app.focused_pane, Pane::Box);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_set_ura_toggle() {
        let dir = std::env::temp_dir().join("rum_ura_test");
        let _ = fs::create_dir_all(&dir);
        let mut app =
            App::new_with_config(Config::default(), Source::Dir(dir.clone()), dir.clone()).unwrap();

        assert!(app.ura_enabled);
        app.execute_command("set noura");
        assert!(!app.ura_enabled);
        app.execute_command("set ura");
        assert!(app.ura_enabled);
        let _ = fs::remove_dir_all(dir);
    }
}
