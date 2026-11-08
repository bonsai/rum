use serde::Deserialize;
use std::path::PathBuf;

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
    candidates
        .into_iter()
        .flatten()
        .find(|p| p.is_file())
}

/// `~` をホームに展開する（先頭のみ）。
pub fn expand(p: &str) -> PathBuf {
    if p == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(p)
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
}
