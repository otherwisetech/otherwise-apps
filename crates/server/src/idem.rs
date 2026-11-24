//! Idempotency keys (SPEC §12.3): the first answer to `(login, key)` is kept 48 hours; a retry
//! gets it back with `replayed` and writes nothing. Kept on disk so a server restart between
//! the write and the retry doesn't write twice.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const KEEP_SECS: i64 = 48 * 3600;
pub const MAX: usize = 5000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stored {
    pub at: i64,
    pub status: u16,
    pub body: Value,
}

#[derive(Default)]
pub struct Idem {
    path: Option<PathBuf>,
    done: BTreeMap<String, Stored>,
    in_flight: BTreeSet<String>,
}

pub enum Begin {
    New,
    Replay(Stored),
    /// The same key is being answered right now.
    Busy,
}

fn k(login: &str, key: &str) -> String {
    format!("{}\u{1f}{key}", login.to_ascii_lowercase())
}

impl Idem {
    pub fn open(server_dir: &Path) -> Idem {
        let path = server_dir.join("idempotency.json");
        let done = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Idem { path: Some(path), done, in_flight: BTreeSet::new() }
    }

    pub fn begin(&mut self, login: &str, key: &str, now: i64) -> Begin {
        let id = k(login, key);
        if let Some(s) = self.done.get(&id).filter(|s| now - s.at < KEEP_SECS) {
            return Begin::Replay(s.clone());
        }
        if !self.in_flight.insert(id) {
            return Begin::Busy;
        }
        Begin::New
    }

    /// The answer (only answers that wrote or were refused for good are stored: a server
    /// that was unavailable may be retried).
    pub fn finish(&mut self, login: &str, key: &str, now: i64, stored: Option<Stored>) {
        let id = k(login, key);
        self.in_flight.remove(&id);
        let Some(s) = stored else { return };
        self.done.insert(id, s);
        self.done.retain(|_, s| now - s.at < KEEP_SECS);
        while self.done.len() > MAX {
            let oldest = self.done.iter().min_by_key(|(_, s)| s.at).map(|(k, _)| k.clone());
            match oldest {
                Some(o) => self.done.remove(&o),
                None => break,
            };
        }
        if let Some(p) = &self.path
            && let Ok(b) = serde_json::to_vec(&self.done)
            && otherwise_link::fsutil::write_atomic(p, &b).is_ok()
        {
            let _ = crate::config::private_file(p);
        }
    }
}

/// A valid key: 16 to 128 characters of `[A-Za-z0-9_-]`.
pub fn valid_key(k: &str) -> bool {
    (16..=128).contains(&k.len()) && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replays_per_login_and_survives_a_restart() {
        let t = tempfile::tempdir().unwrap();
        let mut i = Idem::open(t.path());
        let key = "0123456789abcdef0123";
        assert!(matches!(i.begin("ana@x.com", key, 100), Begin::New));
        assert!(matches!(i.begin("ana@x.com", key, 100), Begin::Busy));
        assert!(matches!(i.begin("bob@x.com", key, 100), Begin::New), "keys are per login");
        i.finish("ana@x.com", key, 101, Some(Stored { at: 101, status: 200, body: json!({"ok": true}) }));
        i.finish("bob@x.com", key, 101, None);
        let mut j = Idem::open(t.path());
        assert!(matches!(j.begin("ANA@x.com", key, 200), Begin::Replay(s) if s.body["ok"] == true));
        assert!(matches!(j.begin("bob@x.com", key, 200), Begin::New), "an unavailable answer isn't kept");
        assert!(matches!(j.begin("ana@x.com", key, 101 + KEEP_SECS), Begin::New), "expired");
        assert!(valid_key(key) && !valid_key("short") && !valid_key("../../../../../../../x"));
    }
}
