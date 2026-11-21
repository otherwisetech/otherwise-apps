//! The Assistant's conversation history: one JSON file per conversation under
//! `<data root>/otherwise-apps/assistant/` (the data root from otherwise-link, so it moves with
//! the rest of the user's Otherwise data). Written atomically; unreadable files are skipped.
use crate::agent_cli::{Backend, ToolRef};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolUse {
    #[serde(flatten)]
    pub tool: ToolRef,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub failed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub user: bool,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolUse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    pub schema: u32,
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<Backend>,
    /// Unix seconds.
    pub created: i64,
    pub updated: i64,
    pub messages: Vec<Message>,
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// `<root>/otherwise-apps/assistant`.
pub fn dir_in(root: &Path) -> PathBuf {
    root.join(otherwise_link::link::HUB_APP).join("assistant")
}

/// The folder in the current data root.
pub fn dir() -> PathBuf {
    dir_in(&otherwise_link::paths::data_root())
}

/// Ids become file names: plain ASCII only.
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

impl Conversation {
    pub fn new() -> Conversation {
        let t = now();
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        Conversation { schema: SCHEMA, id: format!("{t}-{nanos:08x}"), title: String::new(), backend: None, created: t, updated: t, messages: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    pub fn push(&mut self, m: Message) {
        if self.title.is_empty() && m.user {
            self.title = title_of(&m.text);
        }
        self.messages.push(m);
        self.updated = now();
    }

    /// `(from the user, text)` pairs for the prompt; failed turns are left out.
    pub fn pairs(&self) -> Vec<(bool, String)> {
        self.messages.iter().filter(|m| m.error.is_none() && !m.text.trim().is_empty()).map(|m| (m.user, m.text.clone())).collect()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        if !safe_id(&self.id) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad conversation id"));
        }
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        otherwise_link::fsutil::write_atomic(&dir.join(format!("{}.json", self.id)), (json + "\n").as_bytes())
    }
}

impl Default for Conversation {
    fn default() -> Self {
        Conversation::new()
    }
}

/// The first line of the question, at most 60 characters.
pub fn title_of(text: &str) -> String {
    let line = text.trim().lines().next().unwrap_or("").trim();
    let mut t: String = line.chars().take(60).collect();
    if line.chars().count() > 60 {
        t.push('…');
    }
    t
}

/// Every saved conversation, the most recent first (newer schemas and broken files skipped).
pub fn list(dir: &Path) -> Vec<Conversation> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<Conversation> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|s| serde_json::from_str::<Conversation>(&s).ok())
        .filter(|c| c.schema <= SCHEMA && safe_id(&c.id))
        .collect();
    v.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| b.id.cmp(&a.id)));
    v
}
