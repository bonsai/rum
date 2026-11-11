mod app;
mod buffer;
mod config;
mod doc;
mod ui;

use anyhow::Result;
use app::{App, Source};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::path::PathBuf;
use std::time::Duration;

const HELP: &str = "\
rum — vim 風 TUI リーダ/エディタ

使い方:
  rum                    設定（YAML）の source に従う
  rum <dir>              md/txt ディレクトリを開く
  rum <file.db>          SQLite（chira-ura の ura.db）を開く
  rum --db <ura.db>      SQLite を強制
  rum --dir <dir>        ディレクトリを強制
  rum --config <file>    設定ファイルを指定（既定探索を上書き）

設定ファイル探索順:
  1. --config / 環境変数 RUM_CONFIG
  2. ./rum.yaml
  3. ~/.config/rum/config.yaml
  4. ~/.chira/rum.yaml

設定例 (rum.yaml):
  source: db
  db: ~/.chira/ura.db
  dir: ~/.chira/md

主なキー:
  h j k l / w b / 0 $ / gg G      移動
  i a o O A I / Esc                編集
  dd yy p / x / u / Ctrl-r         編集
  / pattern, n N                   検索
  :w :q :wq :e :new :next :prev    コマンド
  :db <path> / :dir <path>         読み込み元を切替
  :md [dir] / :sync [dir]          chira md / chira md import
  Ctrl-s 保存 / Ctrl-p Ctrl-n 前後のノート
";

struct Args {
    config_path: Option<String>,
    target: Option<String>,
    force: Option<SourceKind>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    Db,
    Dir,
}

fn parse_args() -> Result<Option<Args>> {
    let mut config_path = None;
    let mut target = None;
    let mut force = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" | "-c" => config_path = it.next(),
            "--db" => {
                target = it.next();
                force = Some(SourceKind::Db);
            }
            "--dir" => {
                target = it.next();
                force = Some(SourceKind::Dir);
            }
            "-h" | "--help" => {
                print!("{}", HELP);
                return Ok(None);
            }
            other => target = Some(other.to_string()),
        }
    }
    Ok(Some(Args {
        config_path,
        target,
        force,
    }))
}

fn main() -> Result<()> {
    let args = match parse_args()? {
        Some(a) => a,
        None => return Ok(()),
    };

    let cfg = config::Config::load(args.config_path.as_deref());

    let source = match (&args.target, args.force) {
        (Some(t), force) => {
            let p = config::expand(t);
            if !config::validate_path(&p) {
                eprintln!("Warning: specified path may be invalid: {}", t);
            }
            let looks_db = p
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("db"))
                .unwrap_or(false);
            match force {
                Some(SourceKind::Db) => Source::Db(p),
                Some(SourceKind::Dir) => Source::Dir(p),
                None if p.is_dir() => Source::Dir(p),
                None if looks_db => Source::Db(p),
                None => Source::Dir(p),
            }
        }
        (None, Some(SourceKind::Db)) => {
            let p = cfg.db_path();
            if !config::validate_path(&p) {
                eprintln!("Warning: configured db path may be invalid: {}", cfg.db);
            }
            Source::Db(p)
        }
        (None, Some(SourceKind::Dir)) => {
            let p = cfg.dir_path();
            if !config::validate_path(&p) {
                eprintln!("Warning: configured dir path may be invalid: {}", cfg.dir);
            }
            Source::Dir(p)
        }
        (None, None) => match cfg.source.to_ascii_lowercase().as_str() {
            "dir" => {
                let p = cfg.dir_path();
                if !config::validate_path(&p) {
                    eprintln!("Warning: configured dir path may be invalid: {}", cfg.dir);
                }
                Source::Dir(p)
            }
            _ => {
                let p = cfg.db_path();
                if !config::validate_path(&p) {
                    eprintln!("Warning: configured db path may be invalid: {}", cfg.db);
                }
                Source::Db(p)
            }
        },
    };

    let md_dir: PathBuf = cfg.dir_path();
    let mut app = App::new_with_config(cfg, source, md_dir)?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Error: {:?}", err);
    }

    Ok(())
}

fn run_app<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| ui::render(f, app))?;

        if app.should_quit {
            break;
        }

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                app.handle_key(key);
            }
        }
    }

    Ok(())
}
