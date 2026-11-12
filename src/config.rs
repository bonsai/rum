use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// BOX（箱 / ノートブック）の設定
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxConfig {
    pub name: String,
    pub source: String, // "db" または "dir"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
}

impl BoxConfig {
    pub fn resolved_path(&self) -> PathBuf {
        match self.source.to_ascii_lowercase().as_str() {
            "dir" => {
                let p = self.dir.as_deref().unwrap_or("~/.chira/md");
                expand(p)
            }
            _ => {
                let p = self.db.as_deref().unwrap_or("~/.chira/ura.db");
                expand(p)
            }
        }
    }
}

/// rum の設定。パスはハードコードせず YAML から読む。
///
/// 探索順（最初に見つかったものを使う）:
///   1. `--config <path>` / 環境変数 `RUM_CONFIG`
///   2. `./rum.yaml`（カレント）
///   3. `~/.config/rum/config.yaml`
///   4. `~/.chira/rum.yaml`
///
/// 値が無いフィールドは Default（下記）で埋まる。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 既定の読み込み元: "db"（SQLite）または "dir"（md/txt ディレクトリ）
    pub source: String,
    /// source: db のときに開く SQLite
    pub db: String,
    /// source: dir のときに開くディレクトリ（`:md` / `:sync` の既定でもある）
    pub dir: String,
    /// BOX 一覧（未設定の場合は source/db/dir から自動生成）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boxes: Vec<BoxConfig>,
    /// 読み込んだ設定ファイルのパス（追記・保存用）
    #[serde(skip)]
    pub loaded_path: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            source: "db".to_string(),
            db: "~/.chira/ura.db".to_string(),
            dir: "~/.chira/md".to_string(),
            boxes: Vec::new(),
            loaded_path: None,
        }
    }
}

impl Config {
    /// 設定を読む。explicit が指すファイルが無ければ既定 config を探す。
    pub fn load(explicit: Option<&str>) -> Self {
        let path = match explicit {
            Some(p) => Some(PathBuf::from(p)),
            None => find_default_config(),
        };
        if let Some(path) = path {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(mut cfg) = serde_yaml::from_str::<Config>(&text) {
                    cfg.loaded_path = Some(path);
                    return cfg;
                }
            }
        }
        Config::default()
    }

    pub fn db_path(&self) -> PathBuf {
        expand(&self.db)
    }

    pub fn dir_path(&self) -> PathBuf {
        expand(&self.dir)
    }

    /// 有効な BOX 一覧を返す。
    /// `boxes` が空なら後方互換として既存の source / db / dir から 1 つ生成する。
    pub fn effective_boxes(&self) -> Vec<BoxConfig> {
        if !self.boxes.is_empty() {
            return self.boxes.clone();
        }
        let is_dir = self.source.eq_ignore_ascii_case("dir");
        vec![BoxConfig {
            name: if is_dir {
                "下書き".to_string()
            } else {
                "メモ".to_string()
            },
            source: if is_dir {
                "dir".to_string()
            } else {
                "db".to_string()
            },
            db: if is_dir { None } else { Some(self.db.clone()) },
            dir: if is_dir { Some(self.dir.clone()) } else { None },
        }]
    }

    /// 新しい BOX を設定に追加し、設定ファイルに保存する。
    pub fn add_box(&mut self, box_cfg: BoxConfig) -> anyhow::Result<PathBuf> {
        if self.boxes.is_empty() {
            self.boxes = self.effective_boxes();
        }
        self.boxes.push(box_cfg);

        let target_path = self.loaded_path.clone().unwrap_or_else(|| {
            dirs::config_dir()
                .map(|d| d.join("rum").join("config.yaml"))
                .unwrap_or_else(|| PathBuf::from("rum.yaml"))
        });

        if let Some(parent) = target_path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }

        let yaml = serde_yaml::to_string(self)?;
        std::fs::write(&target_path, yaml)?;
        self.loaded_path = Some(target_path.clone());
        Ok(target_path)
    }
}

fn find_default_config() -> Option<PathBuf> {
    let candidates = [
        std::env::var_os("RUM_CONFIG").map(PathBuf::from),
        Some(PathBuf::from("rum.yaml")),
        dirs::config_dir().map(|d| d.join("rum").join("config.yaml")),
        dirs::home_dir().map(|h| h.join(".chira").join("rum.yaml")),
    ];
    candidates.into_iter().flatten().find(|p| p.is_file())
}

/// パスが有効な形式か検証する。
/// 空文字列、ヌル文字、または Windows での無効な文字/予約名をチェックする。
pub fn validate_path(p: &Path) -> bool {
    let s = p.to_string_lossy();
    if s.trim().is_empty() {
        return false;
    }
    // ヌルバイトが含まれる場合は無効
    if s.contains('\0') {
        return false;
    }

    // Windows 向けパス検証（Windows 実行時、または Windows 形式パスのチェック）
    // 各コンポーネントの検証
    for component in p.components() {
        if let std::path::Component::Normal(os_str) = component {
            let seg = os_str.to_string_lossy();
            // Windows の予約デバイス名チェック (CON, PRN, AUX, NUL, COM1..9, LPT1..9)
            let stem = seg.split('.').next().unwrap_or(&seg);
            if matches!(
                stem.to_ascii_uppercase().as_str(),
                "CON"
                    | "PRN"
                    | "AUX"
                    | "NUL"
                    | "COM1"
                    | "COM2"
                    | "COM3"
                    | "COM4"
                    | "COM5"
                    | "COM6"
                    | "COM7"
                    | "COM8"
                    | "COM9"
                    | "LPT1"
                    | "LPT2"
                    | "LPT3"
                    | "LPT4"
                    | "LPT5"
                    | "LPT6"
                    | "LPT7"
                    | "LPT8"
                    | "LPT9"
            ) {
                return false;
            }
        }
    }

    true
}

/// `~` をホームに展開する（先頭のみ）。
/// Windows と Unix のセパレータ差異（`/` と `\`）および混在セパレータに対応。
pub fn expand(p: &str) -> PathBuf {
    let p = p.trim();
    if p.is_empty() {
        return PathBuf::new();
    }

    let (is_tilde, rest) = if p == "~" {
        (true, "")
    } else if let Some(r) = p.strip_prefix("~/") {
        (true, r)
    } else if let Some(r) = p.strip_prefix("~\\") {
        (true, r)
    } else {
        (false, p)
    };

    if is_tilde {
        let home = dirs::home_dir();
        match home {
            Some(mut base) => {
                let trimmed_rest = rest.trim_start_matches(['/', '\\']);
                if trimmed_rest.is_empty() {
                    return base;
                }
                // `/` または `\` で分割して順に join することで、
                // OS に依存せず混在セパレータ（例: "dir\\file" on Unix, "dir/file" on Windows）を
                // 適切な PathBuf コンポーネントとして結合できる。
                for part in trimmed_rest.split(['/', '\\']) {
                    if !part.is_empty() {
                        base.push(part);
                    }
                }
                base
            }
            None => {
                // home_dir が取得できない場合は "~" を維持した PathBuf を返す
                if rest.is_empty() {
                    PathBuf::from("~")
                } else {
                    let mut pb = PathBuf::from("~");
                    for part in rest.split(['/', '\\']) {
                        if !part.is_empty() {
                            pb.push(part);
                        }
                    }
                    pb
                }
            }
        }
    } else {
        PathBuf::from(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_points_at_chira() {
        let cfg = Config::default();
        assert_eq!(cfg.source, "db");
        assert!(cfg.db.contains(".chira"));
        assert!(cfg.db.ends_with("ura.db"));
        assert!(cfg.dir.ends_with("md"));
    }

    #[test]
    fn yaml_parses_and_fills_missing_fields() {
        let cfg: Config = serde_yaml::from_str("source: dir\ndir: /tmp/notes\n").unwrap();
        assert_eq!(cfg.source, "dir");
        assert_eq!(cfg.dir, "/tmp/notes");
        // 未指定は Default
        assert!(cfg.db.ends_with("ura.db"));
        let boxes = cfg.effective_boxes();
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].name, "下書き");
        assert_eq!(boxes[0].source, "dir");
    }

    #[test]
    fn yaml_parses_boxes() {
        let yaml = r#"
boxes:
  - name: メモ
    source: db
    db: ~/.chira/ura.db
  - name: 下書き
    source: dir
    dir: ~/.chira/md
"#;
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.boxes.len(), 2);
        assert_eq!(cfg.boxes[0].name, "メモ");
        assert_eq!(cfg.boxes[0].source, "db");
        assert_eq!(cfg.boxes[1].name, "下書き");
        assert_eq!(cfg.boxes[1].source, "dir");
    }

    #[test]
    fn add_box_persists_to_file() {
        let tmp = std::env::temp_dir().join("rum_test_config.yaml");
        let mut cfg = Config {
            loaded_path: Some(tmp.clone()),
            ..Config::default()
        };
        cfg.add_box(BoxConfig {
            name: "新規箱".to_string(),
            source: "dir".to_string(),
            db: None,
            dir: Some("/tmp/new_box".to_string()),
        })
        .unwrap();

        let loaded = Config::load(Some(tmp.to_str().unwrap()));
        assert_eq!(loaded.boxes.len(), 2);
        assert_eq!(loaded.boxes[1].name, "新規箱");
        let _ = std::fs::remove_file(tmp);
    }

    #[test]
    fn expand_tilde() {
        let out = expand("~/foo/bar");
        assert!(out.ends_with("foo/bar") || out.ends_with("foo\\bar"));
        assert!(!out.to_string_lossy().contains('~'));
    }

    #[test]
    fn expand_plain_path_unchanged() {
        assert_eq!(expand("/tmp/x"), PathBuf::from("/tmp/x"));
    }

    #[test]
    fn expand_tilde_alone_and_trailing_separators() {
        let home = dirs::home_dir().expect("home dir available");
        assert_eq!(expand("~"), home);
        assert_eq!(expand("~/"), home);
        assert_eq!(expand("~\\"), home);
        assert_eq!(expand("~///"), home);
        assert_eq!(expand("~\\\\\\"), home);
    }

    #[test]
    fn expand_windows_and_mixed_separators() {
        let home = dirs::home_dir().expect("home dir available");
        let expected = home.join("dir").join("file");

        assert_eq!(expand("~/dir/file"), expected);
        assert_eq!(expand("~/dir\\file"), expected);
        assert_eq!(expand("~\\dir\\file"), expected);
        assert_eq!(expand("~\\dir/file"), expected);
    }

    #[test]
    fn expand_empty_and_whitespace() {
        assert_eq!(expand(""), PathBuf::new());
        assert_eq!(expand("   "), PathBuf::new());
    }

    #[test]
    fn validate_path_cases() {
        assert!(validate_path(Path::new("/valid/path")));
        assert!(validate_path(Path::new("relative/path.txt")));
        assert!(!validate_path(Path::new("")));
        assert!(!validate_path(Path::new("   ")));
        assert!(!validate_path(Path::new("invalid/\0/path")));
        assert!(!validate_path(Path::new("dir/nul.txt")));
        assert!(!validate_path(Path::new("dir/con")));
    }
}
