use serde::Deserialize;
use std::path::{Path, PathBuf};

/// rum の設定。パスはハードコードせず YAML から読む。
///
/// 探索順（最初に見つかったものを使う）:
///   1. `--config <path>` / 環境変数 `RUM_CONFIG`
///   2. `./rum.yaml`（カレント）
///   3. `~/.config/rum/config.yaml`
///   4. `~/.chira/rum.yaml`
///
/// 値が無いフィールドは Default（下記）で埋まる。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 既定の読み込み元: "db"（SQLite）または "dir"（md/txt ディレクトリ）
    pub source: String,
    /// source: db のときに開く SQLite
    pub db: String,
    /// source: dir のときに開くディレクトリ（`:md` / `:sync` の既定でもある）
    pub dir: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            source: "db".to_string(),
            db: "~/.chira/ura.db".to_string(),
            dir: "~/.chira/md".to_string(),
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
                if let Ok(cfg) = serde_yaml::from_str::<Config>(&text) {
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
