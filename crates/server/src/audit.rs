//! `<server dir>/audit/<yyyy-mm>.jsonl` (SPEC §12.7): one line per write and per snapshot
//! download, appended. Never the arguments (they are the user's data; the document's own journal
//! has them): their SHA-256.
use serde_json::{Value, json};
use sha2::Digest;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct Audit {
    dir: PathBuf,
    lock: Mutex<()>,
}

pub fn sha256(v: &Value) -> String {
    let d = sha2::Sha256::digest(serde_json::to_vec(v).unwrap_or_default());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

impl Audit {
    pub fn new(server_dir: &Path) -> Audit {
        Audit { dir: server_dir.join("audit"), lock: Mutex::new(()) }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `line` gets `at`; written in the current month's file.
    pub fn write(&self, mut line: Value) {
        let now = chrono::Utc::now();
        line["at"] = json!(now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
        let _g = self.lock.lock();
        if std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let _ = crate::config::private_dir(&self.dir);
        let p = self.dir.join(format!("{}.jsonl", now.format("%Y-%m")));
        let mut o = std::fs::OpenOptions::new();
        o.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        if let Ok(mut f) = o.open(&p) {
            let _ = writeln!(f, "{line}");
        }
    }

    /// Lines of `month` (`YYYY-MM`, default this one), at most `limit`, newest last.
    pub fn read(&self, month: Option<&str>, limit: usize) -> Vec<Value> {
        let m = month.filter(|m| m.len() == 7 && m.as_bytes()[4] == b'-' && m.bytes().enumerate().all(|(i, b)| i == 4 || b.is_ascii_digit())).map(str::to_string).unwrap_or_else(|| chrono::Utc::now().format("%Y-%m").to_string());
        let s = std::fs::read_to_string(self.dir.join(format!("{m}.jsonl"))).unwrap_or_default();
        let v: Vec<Value> = s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let skip = v.len().saturating_sub(limit);
        v.into_iter().skip(skip).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_hashes_never_arguments() {
        let t = tempfile::tempdir().unwrap();
        let a = Audit::new(t.path());
        let args = json!({"id": "r1", "merchant": "Café Secreto"});
        a.write(json!({"login": "ana@x.com", "op": "set_receipt", "argsSha256": sha256(&args), "result": "ok"}));
        a.write(json!({"login": "ana@x.com", "op": "snapshot", "result": "ok"}));
        let lines = a.read(None, 10);
        assert_eq!(lines.len(), 2);
        assert!(lines[0]["at"].is_string());
        let raw = std::fs::read_to_string(std::fs::read_dir(a.dir()).unwrap().next().unwrap().unwrap().path()).unwrap();
        assert!(!raw.contains("Secreto"), "no arguments in the audit");
        assert_eq!(a.read(Some("../../etc"), 10).len(), 2, "a bad month reads this one");
        assert_eq!(a.read(None, 1)[0]["op"], "snapshot");
    }
}
