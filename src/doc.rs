use chrono::Utc;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Front matter metadata representing note information
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontMatter {
    pub id: String,
    pub title: String,
    pub updated_at: i64,
    /// Any additional custom front matter fields preserved as raw key-value pairs
    pub extra: BTreeMap<String, String>,
}

/// A Document representing either a file with YAML/front matter or plain text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub path: Option<PathBuf>,
    pub front_matter: Option<FrontMatter>,
    pub body: String,
    pub modified: bool,
}

impl Document {
    pub fn new_empty() -> Self {
        Self {
            path: None,
            front_matter: None,
            body: String::new(),
            modified: false,
        }
    }

    /// Load document from file path
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let path_ref = path.as_ref();
        let content = fs::read_to_string(path_ref)?;
        let (front_matter, body) = parse_markdown_or_plain(&content);
        Ok(Self {
            path: Some(path_ref.to_path_buf()),
            front_matter,
            body,
            modified: false,
        })
    }

    /// Serialize document back to string format.
    /// If front_matter exists, serialize with --- delimiters.
    /// Otherwise serialize as plain text body.
    pub fn to_string_content(&self) -> String {
        if let Some(fm) = &self.front_matter {
            serialize_markdown(fm, &self.body)
        } else {
            self.body.clone()
        }
    }

    /// Save to current path or specific path. Updates `updatedAt` on save if front_matter exists.
    /// If front_matter does not exist but it's saved as .md (or newly requested front matter),
    /// we check if we should create front matter or keep plain text:
    /// Per task instructions:
    /// "front matter（id / title / updatedAt）を保って保存。保存時 updatedAt を now に。
    ///  id が無ければ採番して front matter を作る。front matter の無いファイルは素テキストのまま保存。"
    pub fn save_to<P: AsRef<Path>>(&mut self, path: P) -> anyhow::Result<()> {
        let path_ref = path.as_ref();
        let now = Utc::now().timestamp_millis();

        if let Some(fm) = &mut self.front_matter {
            if fm.id.is_empty() {
                fm.id = generate_note_id(now);
            }
            fm.updated_at = now;
            // Always keep title up to date or fallback to derive_title
            fm.title = derive_title(&self.body);
        }

        let content = self.to_string_content();
        if let Some(parent) = path_ref.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        fs::write(path_ref, content)?;
        self.path = Some(path_ref.to_path_buf());
        self.modified = false;
        Ok(())
    }

    pub fn save(&mut self) -> anyhow::Result<()> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No file name specified"))?;
        self.save_to(path)
    }

    /// Create default front matter for newly created markdown note
    pub fn ensure_front_matter(&mut self) {
        if self.front_matter.is_none() {
            let now = Utc::now().timestamp_millis();
            self.front_matter = Some(FrontMatter {
                id: generate_note_id(now),
                title: derive_title(&self.body),
                updated_at: now,
                extra: BTreeMap::new(),
            });
            self.modified = true;
        }
    }
}

/// Generate note ID matching chira-ura format: note-<now>-<rand4>
pub fn generate_note_id(now: i64) -> String {
    let rand = format!("{:04x}", (now % 65535));
    format!("note-{}-{}", now, rand)
}

/// Derive title from the first non-empty line of content, max 30 chars
pub fn derive_title(content: &str) -> String {
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            let mut chars = trimmed.chars();
            let title: String = chars.by_ref().take(30).collect();
            return title;
        }
    }
    "無題".to_string()
}

/// Parse markdown content with front matter delimiter `---`.
/// If starts with `---`, parse lines until closing `---`.
/// Returns (Option<FrontMatter>, body).
pub fn parse_markdown_or_plain(content: &str) -> (Option<FrontMatter>, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, content.to_string());
    }

    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() || lines[0].trim() != "---" {
        return (None, content.to_string());
    }

    let mut end_idx = None;
    for (i, line) in lines.iter().enumerate().skip(1) {
        if line.trim() == "---" {
            end_idx = Some(i);
            break;
        }
    }

    let end_i = match end_idx {
        Some(i) => i,
        None => return (None, content.to_string()),
    };

    let mut id = String::new();
    let mut title = String::new();
    let mut updated_at: i64 = 0;
    let mut extra = BTreeMap::new();

    for line in &lines[1..end_i] {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            let val = v.trim().trim_matches('"').trim_matches('\'');
            match key {
                "id" => id = val.to_string(),
                "title" => title = val.to_string(),
                "updatedAt" | "updated_at" => {
                    updated_at = val.parse::<i64>().unwrap_or_else(|_| {
                        // Could be ISO 8601 string or numeric
                        chrono::DateTime::parse_from_rfc3339(val)
                            .map(|dt| dt.timestamp_millis())
                            .unwrap_or(0)
                    });
                }
                _ => {
                    extra.insert(key.to_string(), val.to_string());
                }
            }
        }
    }

    let body_lines = &lines[end_i + 1..];
    // Reconstruct body
    let mut body = body_lines.join("\n");
    // If the original content had a trailing newline and body is not empty
    if content.ends_with('\n') && !body.is_empty() {
        body.push('\n');
    }

    (
        Some(FrontMatter {
            id,
            title,
            updated_at,
            extra,
        }),
        body,
    )
}

/// Serialize FrontMatter and body to Markdown format
pub fn serialize_markdown(fm: &FrontMatter, body: &str) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("id: {}\n", fm.id));
    out.push_str(&format!("title: {}\n", fm.title));
    out.push_str(&format!("updatedAt: {}\n", fm.updated_at));
    for (k, v) in &fm.extra {
        out.push_str(&format!("{}: {}\n", k, v));
    }
    out.push_str("---\n");
    out.push_str(body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_plain_text() {
        let raw = "Hello world\nThis is a plain note.";
        let (fm, body) = parse_markdown_or_plain(raw);
        assert!(fm.is_none());
        assert_eq!(body, raw);
    }

    #[test]
    fn test_parse_front_matter() {
        let raw = "---\nid: note-123\ntitle: My Note\nupdatedAt: 1700000000\nauthor: me\n---\nBody line 1\nBody line 2\n";
        let (fm, body) = parse_markdown_or_plain(raw);
        assert!(fm.is_some());
        let fm = fm.unwrap();
        assert_eq!(fm.id, "note-123");
        assert_eq!(fm.title, "My Note");
        assert_eq!(fm.updated_at, 1700000000);
        assert_eq!(fm.extra.get("author").unwrap(), "me");
        assert_eq!(body, "Body line 1\nBody line 2\n");

        let serialized = serialize_markdown(&fm, &body);
        assert!(serialized.starts_with("---\n"));
        assert!(serialized.contains("id: note-123\n"));
        assert!(serialized.contains("title: My Note\n"));
        assert!(serialized.contains("author: me\n"));
        assert!(serialized.ends_with("Body line 1\nBody line 2\n"));
    }

    #[test]
    fn test_save_updates_updated_at() {
        let tmp = std::env::temp_dir().join("rum_test_save.md");
        let raw = "---\nid: test-id\ntitle: Original\nupdatedAt: 1000\n---\nHello\n";
        let mut doc = Document {
            path: Some(tmp.clone()),
            front_matter: parse_markdown_or_plain(raw).0,
            body: "Hello updated\n".to_string(),
            modified: true,
        };
        doc.save().unwrap();

        let loaded = Document::load_from_file(&tmp).unwrap();
        let fm = loaded.front_matter.unwrap();
        assert_eq!(fm.id, "test-id");
        assert_eq!(fm.title, "Hello updated");
        assert!(fm.updated_at > 1000);
        assert_eq!(loaded.body, "Hello updated\n");
        let _ = fs::remove_file(tmp);
    }

    #[test]
    fn test_plain_text_saved_as_plain() {
        let tmp = std::env::temp_dir().join("rum_test_plain.txt");
        let mut doc = Document {
            path: Some(tmp.clone()),
            front_matter: None,
            body: "Plain text note\nsecond line".to_string(),
            modified: true,
        };
        doc.save().unwrap();

        let loaded = Document::load_from_file(&tmp).unwrap();
        assert!(loaded.front_matter.is_none());
        assert_eq!(loaded.body, "Plain text note\nsecond line");
        let _ = fs::remove_file(tmp);
    }
}
