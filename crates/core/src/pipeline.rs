//! What a card should offer (install / open / update), and the verified download that every
//! install and update goes through.
use crate::catalog::{AppEntry, Asset, AssetPattern};
use crate::detect::Installed;
use crate::digest::{self, Check};
use crate::net::{self, NetError};
use crate::platform::{self, Target};
use crate::version::Version;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The app has no published release yet.
    NoRelease,
    /// Released, but not for this OS / CPU.
    Unsupported,
    NotInstalled,
    /// Installed; `version` is None when the install doesn't say.
    UpToDate { version: Option<String> },
    Update { from: String, to: String },
}

impl Status {
    pub fn is_installed(&self) -> bool {
        matches!(self, Status::UpToDate { .. } | Status::Update { .. })
    }
}

pub fn status(e: &AppEntry, installed: Option<&Installed>, t: Target) -> Status {
    let latest = e.release.as_ref().and_then(|r| Version::parse(&r.tag).map(|v| (r, v)));
    let available = latest.as_ref().and_then(|(r, v)| platform::select_asset(e, &r.assets, t).map(|_| v.clone()));
    match (installed, available) {
        (Some(i), Some(new)) => match i.version.as_deref().and_then(Version::parse) {
            Some(have) if new > have => Status::Update { from: have.to_string(), to: new.to_string() },
            _ => Status::UpToDate { version: i.version.clone() },
        },
        (Some(i), None) => Status::UpToDate { version: i.version.clone() },
        (None, Some(_)) => Status::NotInstalled,
        (None, None) if latest.is_none() => Status::NoRelease,
        (None, None) => Status::Unsupported,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub tag: String,
    pub pattern: AssetPattern,
    pub asset: Asset,
}

pub fn plan(e: &AppEntry, t: Target) -> Option<Plan> {
    let r = e.release.as_ref()?;
    let (pattern, asset) = platform::select_asset(e, &r.assets, t)?;
    Some(Plan { tag: r.tag.clone(), pattern: pattern.clone(), asset: asset.clone() })
}

#[derive(Debug)]
pub enum FetchError {
    Net(NetError),
    /// No SHA-256 was published for the file: it can't be verified, so it isn't installed.
    NoDigest,
    Mismatch { expected: String, actual: String },
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Net(e) => e.fmt(f),
            FetchError::NoDigest => f.write_str(&i18n::t!("core.verify.missing")),
            FetchError::Mismatch { expected, actual } => f.write_str(&i18n::t!("core.verify.mismatch", expected = short(expected), actual = short(actual))),
        }
    }
}

fn short(h: &str) -> &str {
    h.get(..12).unwrap_or(h)
}

/// The digest to check against: GitHub's, asked again right before downloading, else the one in
/// the catalog. Both come from GitHub's asset metadata.
pub fn fresh_digest(e: &AppEntry, p: &Plan) -> Option<String> {
    let live = net::fetch_latest_release(&e.repo).ok().flatten().filter(|r| r.tag == p.tag).and_then(|r| r.assets.into_iter().find(|a| a.name == p.asset.name)).and_then(|a| a.digest);
    live.or_else(|| p.asset.digest.clone())
}

/// Refuses `file` unless its SHA-256 equals `published`. A refused file is deleted (it is our
/// own download, never anything of the user's).
pub fn verify_file(file: &Path, sha: &digest::Sha, published: Option<&str>) -> Result<(), FetchError> {
    let refuse = |err| {
        let _ = std::fs::remove_file(file);
        Err(err)
    };
    match digest::check(sha, published) {
        Check::Ok => Ok(()),
        Check::Missing => refuse(FetchError::NoDigest),
        Check::Mismatch { expected, actual } => refuse(FetchError::Mismatch { expected, actual }),
    }
}

/// Downloads into `downloads/` and verifies. Returns the archive path and its hex digest.
pub fn fetch_verified(p: &Plan, published: Option<&str>, downloads: &Path, progress: &dyn Fn(u64, u64), cancel: &AtomicBool) -> Result<(PathBuf, String), FetchError> {
    // refuse before spending the bandwidth
    if published.and_then(digest::parse).is_none() {
        return Err(FetchError::NoDigest);
    }
    let dest = downloads.join(&p.asset.name);
    let sha = net::download(&p.asset.url, &dest, p.asset.size, progress, cancel).map_err(FetchError::Net)?;
    verify_file(&dest, &sha, published)?;
    Ok((dest, digest::hex(&sha)))
}
