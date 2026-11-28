//! HTTPS: the manifest, GitHub's release API, icons, and release downloads (hashed while they
//! stream to disk). All blocking; run on a background thread.
use crate::catalog::{self, CatalogError, Parsed, Release};
use crate::digest::{HashingWriter, Sha};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Debug)]
pub enum NetError {
    Status(u16),
    Transport(String),
    Io(std::io::Error),
    Catalog(CatalogError),
    TooLarge,
    Cancelled,
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            NetError::Status(403 | 429) => i18n::t!("core.net.rate_limited"),
            NetError::Status(c) => i18n::t!("core.net.status", code = c),
            NetError::Transport(e) => i18n::t!("core.net.transport", detail = e),
            NetError::Io(e) => i18n::t!("core.net.io", detail = e),
            NetError::Catalog(e) => e.to_string(),
            NetError::TooLarge => i18n::t!("core.net.too_large"),
            NetError::Cancelled => i18n::t!("core.net.cancelled"),
        };
        f.write_str(&s)
    }
}

impl From<ureq::Error> for NetError {
    fn from(e: ureq::Error) -> Self {
        match e {
            ureq::Error::StatusCode(c) => NetError::Status(c),
            other => NetError::Transport(other.to_string()),
        }
    }
}

impl From<std::io::Error> for NetError {
    fn from(e: std::io::Error) -> Self {
        NetError::Io(e)
    }
}

fn user_agent() -> String {
    format!("otherwise-apps/{}", env!("CARGO_PKG_VERSION"))
}

/// Short requests: manifest, API, icons.
fn api() -> &'static ureq::Agent {
    static A: OnceLock<ureq::Agent> = OnceLock::new();
    A.get_or_init(|| ureq::Agent::config_builder().user_agent(user_agent()).timeout_global(Some(Duration::from_secs(20))).max_redirects(5).build().into())
}

/// Downloads: no overall limit (big files on slow links), but a stalled connection fails.
fn downloads() -> &'static ureq::Agent {
    static A: OnceLock<ureq::Agent> = OnceLock::new();
    A.get_or_init(|| ureq::Agent::config_builder().user_agent(user_agent()).timeout_connect(Some(Duration::from_secs(20))).timeout_recv_response(Some(Duration::from_secs(30))).max_redirects(10).build().into())
}

pub fn get_text(url: &str, limit: u64) -> Result<String, NetError> {
    let mut resp = api().get(url).header("Accept", "application/json").call()?;
    Ok(resp.body_mut().with_config().limit(limit).read_to_string()?)
}

/// A calendar (.ics) from an http(s) URL, at most 16 MiB.
pub fn get_calendar(url: &str) -> Result<String, NetError> {
    let l = url.trim().to_ascii_lowercase();
    if !(l.starts_with("https://") || l.starts_with("http://")) {
        return Err(NetError::Transport(i18n::t!("core.net.not_http")));
    }
    let mut resp = api().get(url.trim()).header("Accept", "text/calendar, */*").call()?;
    Ok(resp.body_mut().with_config().limit(16 << 20).read_to_string()?)
}

pub fn get_bytes(url: &str, limit: u64) -> Result<Vec<u8>, NetError> {
    let mut resp = api().get(url).call()?;
    Ok(resp.body_mut().with_config().limit(limit).read_to_vec()?)
}

/// `apps.v2.json`, else the schema-1 `apps.json` (a website that doesn't serve v2 yet, or a
/// broken v2 file); the v2 error is the one reported when both fail.
pub fn fetch_manifest() -> Result<Parsed, NetError> {
    let get = |url| catalog::parse_manifest_lenient(&get_text(url, 4 << 20)?).map_err(NetError::Catalog);
    get(catalog::MANIFEST_URL).or_else(|e| get(catalog::LEGACY_MANIFEST_URL).map_err(|_| e))
}

/// `Ok(None)`: the repository has no published release yet.
pub fn fetch_latest_release(repo: &str) -> Result<Option<Release>, NetError> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let req = api().get(&url).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28");
    match req.call() {
        Ok(mut resp) => {
            let body = resp.body_mut().with_config().limit(4 << 20).read_to_string()?;
            catalog::parse_github_release(&body).map_err(NetError::Catalog)
        }
        Err(ureq::Error::StatusCode(404)) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Streams `url` into `dest` while hashing it. `progress(done, total)`; `cancel` is checked
/// between chunks. A partial file is removed on any failure.
pub fn download(url: &str, dest: &Path, expected_size: u64, progress: &dyn Fn(u64, u64), cancel: &AtomicBool) -> Result<Sha, NetError> {
    let run = || -> Result<Sha, NetError> {
        let mut resp = downloads().get(url).call()?;
        let total = resp.body().content_length().unwrap_or(expected_size);
        // a little slack over the published size, but never unbounded
        let limit = expected_size.max(total).saturating_add(1 << 20).max(1 << 20);
        let mut reader = resp.body_mut().with_config().limit(limit).reader();
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut w = HashingWriter::new(std::io::BufWriter::new(std::fs::File::create(dest)?));
        let mut buf = vec![0u8; 1 << 16];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(NetError::Cancelled);
            }
            let n = reader.read(&mut buf).map_err(|e| NetError::Transport(e.to_string()))?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n])?;
            progress(w.written, total);
        }
        if expected_size > 0 && w.written != expected_size {
            return Err(NetError::Transport(i18n::t!("core.net.size", got = w.written, want = expected_size)));
        }
        let (_, sha) = w.finish()?;
        Ok(sha)
    };
    let res = run();
    if res.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    res
}
