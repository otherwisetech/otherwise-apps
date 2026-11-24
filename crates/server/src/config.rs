//! `<device dir>/server/server.json`: what this computer shares, with whom, and how. Machine
//! state (never the data root): moving the data root doesn't move it, and nothing syncs it.
use otherwise_remote::Role;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const SCHEMA: u32 = 1;
pub const DEFAULT_PORT: u16 = 47900;

/// One shared document: an app's folder on this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Doc {
    pub app: String,
    pub id: String,
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    /// Tailscale login (`ana@example.com`, `juan@github`) or a node tag (`tag:ci`).
    pub login: String,
    #[serde(default)]
    pub name: String,
    pub role: Role,
    /// Per document (`<app>/<doc>`): another role, or `none` to hide it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub docs: BTreeMap<String, DocRole>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DocRole {
    Role(Role),
    None(NoneTag),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoneTag {
    None,
}

/// The team's licence tier, as far as seats go (prices and names are Vcity's).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tier {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_members: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Backups {
    /// Where nightly copies go (default `~/.otherwise-backups`). Never inside a document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<PathBuf>,
    #[serde(default = "d14")]
    pub keep_daily: u32,
    #[serde(default = "d8")]
    pub keep_weekly: u32,
    #[serde(default = "d12")]
    pub keep_monthly: u32,
    /// Local time `HH:MM`.
    #[serde(default = "d0330")]
    pub at: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn d14() -> u32 {
    14
}
fn d8() -> u32 {
    8
}
fn d12() -> u32 {
    12
}
fn d0330() -> String {
    "03:30".into()
}
fn yes() -> bool {
    true
}

impl Default for Backups {
    fn default() -> Self {
        Backups { dir: None, keep_daily: 14, keep_weekly: 8, keep_monthly: 12, at: d0330(), enabled: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub schema: u32,
    #[serde(default = "port")]
    pub port: u16,
    #[serde(default)]
    pub docs: Vec<Doc>,
    #[serde(default)]
    pub members: Vec<Member>,
    #[serde(default)]
    pub tier: Tier,
    #[serde(default)]
    pub backups: Backups,
    /// Skip the check that loopback connections come from Tailscale. Only for test servers:
    /// any process of this user could then claim any identity.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub trust_loopback: bool,
}

fn port() -> u16 {
    DEFAULT_PORT
}

impl Default for Config {
    fn default() -> Self {
        Config { schema: SCHEMA, port: DEFAULT_PORT, docs: Vec::new(), members: Vec::new(), tier: Tier::default(), backups: Backups::default(), trust_loopback: false }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Invalid(String),
    Newer(u32),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "{e}"),
            ConfigError::Invalid(e) => write!(f, "server.json: {e}"),
            ConfigError::Newer(s) => write!(f, "server.json has schema {s}, newer than {SCHEMA}"),
        }
    }
}

/// `<device dir>/server`.
pub fn dir(device_dir: &Path) -> PathBuf {
    device_dir.join("server")
}

/// A document id from a name: `[a-z0-9-]`.
pub fn slug(s: &str) -> String {
    let folded = otherwise_link::text::fold(s);
    let mut out = String::new();
    for c in folded.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.trim_end_matches('-').chars().take(60).collect();
    if out.is_empty() { "doc".into() } else { out }
}

impl Config {
    pub fn path(server_dir: &Path) -> PathBuf {
        server_dir.join("server.json")
    }

    /// Missing: the default (nothing shared, no members).
    pub fn load(server_dir: &Path) -> Result<Config, ConfigError> {
        let s = match std::fs::read_to_string(Self::path(server_dir)) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(ConfigError::Io(e)),
        };
        let c: Config = serde_json::from_str(&s).map_err(|e| ConfigError::Invalid(e.to_string()))?;
        if c.schema > SCHEMA {
            return Err(ConfigError::Newer(c.schema));
        }
        Ok(c)
    }

    pub fn save(&self, server_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(server_dir)?;
        private_dir(server_dir)?;
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        otherwise_link::fsutil::write_atomic(&Self::path(server_dir), &bytes)?;
        private_file(&Self::path(server_dir))
    }

    pub fn member(&self, login: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.login.eq_ignore_ascii_case(login))
    }

    pub fn doc(&self, app: &str, id: &str) -> Option<&Doc> {
        self.docs.iter().find(|d| d.app == app && d.id == id)
    }

    /// A member's role on a document; `None`: the document doesn't exist for them.
    pub fn role_on(&self, login: &str, app: &str, doc: &str) -> Option<Role> {
        let m = self.member(login)?;
        self.doc(app, doc)?;
        match m.docs.get(&format!("{app}/{doc}")) {
            Some(DocRole::Role(r)) => Some(*r),
            Some(DocRole::None(_)) => None,
            None => Some(m.role),
        }
    }

    pub fn owners(&self) -> impl Iterator<Item = &Member> {
        self.members.iter().filter(|m| m.role == Role::Owner)
    }

    /// Share a folder as a document of `app`; returns its id (stable per path).
    pub fn share(&mut self, app: &str, path: &Path, name: &str, id: Option<&str>) -> Result<String, ConfigError> {
        if !otherwise_remote::client::valid_id(app) {
            return Err(ConfigError::Invalid(format!("bad app id {app}")));
        }
        let path = std::fs::canonicalize(path).map_err(ConfigError::Io)?;
        if let Some(d) = self.docs.iter().find(|d| d.app == app && d.path == path) {
            return Ok(d.id.clone());
        }
        let base = id.map(str::to_string).unwrap_or_else(|| slug(name));
        if !otherwise_remote::client::valid_id(&base) {
            return Err(ConfigError::Invalid(format!("bad document id {base}")));
        }
        let mut id = base.clone();
        let mut n = 2;
        while self.doc(app, &id).is_some() {
            id = format!("{base}-{n}");
            n += 1;
        }
        self.docs.push(Doc { app: app.into(), id: id.clone(), name: name.into(), path });
        Ok(id)
    }

    pub fn unshare(&mut self, app: &str, id: &str) -> bool {
        let n = self.docs.len();
        self.docs.retain(|d| !(d.app == app && d.id == id));
        n != self.docs.len()
    }

    pub fn set_member(&mut self, login: &str, name: &str, role: Role) {
        match self.members.iter_mut().find(|m| m.login.eq_ignore_ascii_case(login)) {
            Some(m) => {
                m.role = role;
                if !name.is_empty() {
                    m.name = name.into();
                }
            }
            None => self.members.push(Member { login: login.into(), name: name.into(), role, docs: BTreeMap::new() }),
        }
    }

    pub fn remove_member(&mut self, login: &str) -> bool {
        let n = self.members.len();
        self.members.retain(|m| !m.login.eq_ignore_ascii_case(login));
        n != self.members.len()
    }
}

/// `0700`, ours.
pub fn private_dir(d: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700))?;
    }
    let _ = d;
    Ok(())
}

pub fn private_file(p: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600))?;
    }
    let _ = p;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_per_document_and_sharing() {
        let t = tempfile::tempdir().unwrap();
        let book = t.path().join("Acme SAS");
        std::fs::create_dir_all(&book).unwrap();
        let mut c = Config::default();
        let id = c.share("expense", &book, "Acme SAS", None).unwrap();
        assert_eq!(id, "acme-sas");
        assert_eq!(c.share("expense", &book, "Again", None).unwrap(), id, "same folder, same document");
        c.set_member("ana@example.com", "Ana", Role::Owner);
        c.set_member("bob@example.com", "Bob", Role::Editor);
        c.members[1].docs.insert("expense/acme-sas".into(), DocRole::Role(Role::Viewer));
        c.set_member("eve@example.com", "", Role::Editor);
        c.members[2].docs.insert("expense/acme-sas".into(), DocRole::None(NoneTag::None));
        assert_eq!(c.role_on("ANA@example.com", "expense", &id), Some(Role::Owner));
        assert_eq!(c.role_on("bob@example.com", "expense", &id), Some(Role::Viewer));
        assert_eq!(c.role_on("eve@example.com", "expense", &id), None, "hidden");
        assert_eq!(c.role_on("mallory@example.com", "expense", &id), None);
        assert_eq!(c.role_on("ana@example.com", "expense", "nope"), None);
        let d = dir(t.path());
        c.save(&d).unwrap();
        let back = Config::load(&d).unwrap();
        assert_eq!(back, c);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(Config::path(&d)).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert!(c.remove_member("bob@example.com") && c.unshare("expense", &id));
        std::fs::write(Config::path(&d), r#"{"schema": 7}"#).unwrap();
        assert!(matches!(Config::load(&d), Err(ConfigError::Newer(7))));
    }
}
