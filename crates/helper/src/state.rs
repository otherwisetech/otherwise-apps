//! What the helper remembers on this computer: last runs, failures, the notifications it already
//! showed, a pause. Kept next to the pointer file (`~/.otherwise/helper/state.json`), never in
//! the (possibly synced) data root: two computers run their own schedules.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A shown notification key is remembered this long.
pub const SHOWN_KEEP_DAYS: i64 = 30;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JobState {
    pub last_start: Option<DateTime<Utc>>,
    pub last_end: Option<DateTime<Utc>>,
    pub last_ok: Option<DateTime<Utc>>,
    /// Consecutive failures (0 after a success).
    pub failures: u32,
    /// The job's last report line.
    pub status: Option<String>,
    /// Nothing configured in the app.
    pub idle: bool,
    /// Why the last run failed (`exit 1`, `timeout`, `memory`), for the menu only.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct State {
    pub schema: u32,
    /// `<app>/<job>` → state.
    pub jobs: BTreeMap<String, JobState>,
    /// `<app>/<notification id>` → Unix seconds it was shown.
    pub shown: BTreeMap<String, i64>,
    pub paused_until: Option<DateTime<Utc>>,
    /// Start at login was turned on once by default; the user's later choice is the OS's.
    pub login_defaulted: bool,
}

pub fn key(app: &str, job: &str) -> String {
    format!("{app}/{job}")
}

/// `~/.otherwise/helper` (the pointer folder, which never moves).
pub fn dir() -> PathBuf {
    otherwise_link::paths::Home::system().pointer_dir().join("helper")
}

pub fn file_in(dir: &Path) -> PathBuf {
    dir.join("state.json")
}

impl State {
    pub fn load(dir: &Path) -> State {
        std::fs::read_to_string(file_in(dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let mut s = self.clone();
        s.schema = 1;
        otherwise_link::fsutil::write_atomic(&file_in(dir), (serde_json::to_string_pretty(&s).unwrap_or_default() + "\n").as_bytes())
    }

    pub fn job(&self, app: &str, job: &str) -> JobState {
        self.jobs.get(&key(app, job)).cloned().unwrap_or_default()
    }

    pub fn paused(&self, now: DateTime<Utc>) -> bool {
        self.paused_until.is_some_and(|t| t > now)
    }

    /// Records a key as shown; false if it already was.
    pub fn mark_shown(&mut self, key: &str, now: DateTime<Utc>) -> bool {
        let cutoff = now.timestamp() - SHOWN_KEEP_DAYS * 86400;
        self.shown.retain(|_, t| *t >= cutoff);
        if self.shown.contains_key(key) {
            return false;
        }
        self.shown.insert(key.to_string(), now.timestamp());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_shown_once() {
        let t = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let mut s = State::load(t.path());
        assert_eq!(s, State::default());
        assert!(s.mark_shown("expense/a", now));
        assert!(!s.mark_shown("expense/a", now));
        s.jobs.insert(key("expense", "mail"), JobState { failures: 2, ..Default::default() });
        s.save(t.path()).unwrap();
        let back = State::load(t.path());
        assert_eq!(back.job("expense", "mail").failures, 2);
        // forgotten after the keep window
        let mut s = back;
        assert!(s.mark_shown("expense/a", now + chrono::Duration::days(SHOWN_KEEP_DAYS + 1)));
    }
}
