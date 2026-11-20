//! The app catalog: `https://otherwisetech.com/apps.json` (schema below), GitHub's
//! `releases/latest` as a second source, and a copy bundled with the launcher as the last resort.
//!
//! ```json
//! { "schema": 1, "generatedAt": "2026-11-20T12:00:00Z", "apps": [ {
//!     "id": "rama", "name": "rama", "repo": "otherwisetech/rama-rs",
//!     "tagline": { "en": "…", "es": "…" }, "description": { "en": "…" },
//!     "icon": "https://otherwisetech.com/apps/rama.png", "homepage": "https://otherwisetech.com/apps/rama",
//!     "bundleId": "io.github.otherwisetech.rama", "installName": "rama", "appName": "rama",
//!     "platforms": { "macos": [ { "pattern": "rama-macos-universal.zip", "arch": "universal" } ], … },
//!     "minimumOs": { "macos": "11.0" },
//!     "release": { "tag": "v0.1.0", "publishedAt": "…", "url": "…", "notes": "…",
//!                  "assets": [ { "name": "…", "size": 1, "url": "…", "digest": "sha256:…" } ] } } ] }
//! ```
//!
//! Every URL is checked: release files must come from the app's own GitHub releases, so a
//! tampered manifest can't point a download anywhere else.
use crate::version::Version;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MANIFEST_URL: &str = "https://otherwisetech.com/apps.json";
pub const ORG: &str = "otherwisetech";
/// Our own entry: updating it is a self-update.
pub const SELF_ID: &str = "otherwise-apps";
pub const SCHEMA: u32 = 1;

static BUNDLED: &str = include_str!("../data/catalog.json");

/// Text per language code; see [`pick`].
pub type Localized = BTreeMap<String, String>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
    pub apps: Vec<AppEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    /// `otherwisetech/<repo>`
    pub repo: String,
    #[serde(default)]
    pub tagline: Localized,
    #[serde(default)]
    pub description: Localized,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// macOS `CFBundleIdentifier`; also names the Linux `.desktop` file.
    pub bundle_id: String,
    /// Binary name, and the install folder on Windows and Linux.
    pub install_name: String,
    /// macOS bundle name without `.app`; `installName` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    /// OS key (`macos`, `windows`, `linux`) → the files that can be installed there.
    #[serde(default)]
    pub platforms: BTreeMap<String, Vec<AssetPattern>>,
    #[serde(default)]
    pub minimum_os: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<Release>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetPattern {
    /// File name; `*` matches anything (see [`crate::platform::glob`]).
    pub pattern: String,
    /// `universal`, `x86_64` or `aarch64`.
    pub arch: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    /// The release page on GitHub.
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    pub url: String,
    /// `sha256:<hex>`, as GitHub reports it for release assets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogError {
    Json(String),
    Schema(u32),
    Invalid { id: String, why: &'static str },
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogError::Json(e) => f.write_str(&i18n::t!("core.catalog.json", detail = e)),
            CatalogError::Schema(n) => f.write_str(&i18n::t!("core.catalog.schema", n = n)),
            CatalogError::Invalid { id, why } => f.write_str(&i18n::t!("core.catalog.invalid", id = id, why = why)),
        }
    }
}

/// `lang`, else English, else any language, else "".
pub fn pick<'a>(text: &'a Localized, lang: &str) -> &'a str {
    text.get(lang).or_else(|| text.get("en")).or_else(|| text.values().next()).map(String::as_str).unwrap_or("")
}

impl AppEntry {
    pub fn app_name(&self) -> &str {
        self.app_name.as_deref().unwrap_or(&self.install_name)
    }

    pub fn repo_url(&self) -> String {
        format!("https://github.com/{}", self.repo)
    }

    pub fn releases_url(&self) -> String {
        format!("https://github.com/{}/releases", self.repo)
    }

    pub fn latest_version(&self) -> Option<Version> {
        self.release.as_ref().and_then(|r| Version::parse(&r.tag))
    }

    pub fn is_self(&self) -> bool {
        self.id == SELF_ID
    }

    fn validate(&self) -> Result<(), &'static str> {
        let safe = |s: &str| !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) && !s.starts_with('.');
        if !safe(&self.id) {
            return Err("id"); // i18n-ok: field name
        }
        if !safe(&self.install_name) || self.app_name.as_deref().is_some_and(|n| n.is_empty() || n.contains(['/', '\\', ':']) || n.starts_with('.')) {
            return Err("installName"); // i18n-ok: field name
        }
        let Some(repo) = self.repo.strip_prefix(ORG).and_then(|r| r.strip_prefix('/')) else { return Err("repo") }; // i18n-ok: field name
        if !safe(repo) {
            return Err("repo"); // i18n-ok: field name
        }
        if !safe(&self.bundle_id) {
            return Err("bundleId"); // i18n-ok: field name
        }
        let https = |u: &Option<String>| u.as_deref().is_none_or(|u| u.starts_with("https://"));
        if !https(&self.icon) || !https(&self.homepage) {
            return Err("url"); // i18n-ok: field name
        }
        if let Some(r) = &self.release {
            if !r.url.starts_with(&format!("https://github.com/{}/releases/", self.repo)) {
                return Err("release.url"); // i18n-ok: field name
            }
            let prefix = format!("https://github.com/{}/releases/download/", self.repo);
            for a in &r.assets {
                if !a.url.starts_with(&prefix) || a.name.contains(['/', '\\']) {
                    return Err("release.assets"); // i18n-ok: field name
                }
            }
        }
        Ok(())
    }
}

/// Parses and validates a manifest. Entries that fail validation make the whole manifest fail:
/// a half-trusted catalog is worse than falling back to the next source.
pub fn parse_manifest(json: &str) -> Result<Manifest, CatalogError> {
    let m: Manifest = serde_json::from_str(json).map_err(|e| CatalogError::Json(e.to_string()))?;
    if m.schema == 0 || m.schema > SCHEMA {
        return Err(CatalogError::Schema(m.schema));
    }
    let mut seen = std::collections::BTreeSet::new();
    for a in &m.apps {
        a.validate().map_err(|why| CatalogError::Invalid { id: a.id.clone(), why })?;
        if !seen.insert(a.id.as_str()) {
            return Err(CatalogError::Invalid { id: a.id.clone(), why: "duplicate" }); // i18n-ok: reason code
        }
    }
    Ok(m)
}

/// The catalog compiled into the launcher (a snapshot of apps.json).
pub fn bundled() -> Manifest {
    parse_manifest(BUNDLED).expect("bundled catalog is valid")
}

pub fn to_json(m: &Manifest) -> String {
    serde_json::to_string_pretty(m).unwrap_or_default()
}

/// A GitHub `releases/latest` body. Drafts and pre-releases are skipped.
pub fn parse_github_release(json: &str) -> Result<Option<Release>, CatalogError> {
    #[derive(Deserialize)]
    struct GhAsset {
        name: String,
        #[serde(default)]
        size: u64,
        browser_download_url: String,
        #[serde(default)]
        digest: Option<String>,
    }
    #[derive(Deserialize)]
    struct Gh {
        tag_name: String,
        html_url: String,
        #[serde(default)]
        published_at: Option<String>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        draft: bool,
        #[serde(default)]
        prerelease: bool,
        #[serde(default)]
        assets: Vec<GhAsset>,
    }
    let g: Gh = serde_json::from_str(json).map_err(|e| CatalogError::Json(e.to_string()))?;
    if g.draft || g.prerelease || Version::parse(&g.tag_name).is_none() {
        return Ok(None);
    }
    Ok(Some(Release {
        tag: g.tag_name,
        published_at: g.published_at,
        url: g.html_url,
        notes: g.body.filter(|b| !b.trim().is_empty()),
        assets: g.assets.into_iter().map(|a| Asset { name: a.name, size: a.size, url: a.browser_download_url, digest: a.digest }).collect(),
    }))
}

/// Puts a GitHub release on an entry if it is valid for it and at least as new as what the
/// entry already has. Returns whether it was taken.
pub fn apply_release(entry: &mut AppEntry, release: Release) -> bool {
    let newer_or_same = match (entry.latest_version(), Version::parse(&release.tag)) {
        (Some(have), Some(new)) => new >= have,
        (None, Some(_)) => true,
        _ => false,
    };
    if !newer_or_same {
        return false;
    }
    let old = entry.release.replace(release);
    if entry.validate().is_err() {
        entry.release = old;
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_parses_and_has_us() {
        let m = bundled();
        assert!(m.apps.iter().any(|a| a.is_self()));
        for a in &m.apps {
            assert!(!pick(&a.tagline, "en").is_empty(), "{} tagline", a.id);
            for lang in ["en", "es", "fr", "pt"] {
                assert!(a.description.contains_key(lang), "{} description {lang}", a.id);
            }
        }
    }

    #[test]
    fn picks_language() {
        let mut t = Localized::new();
        t.insert("en".into(), "Hello".into());
        t.insert("es".into(), "Hola".into());
        assert_eq!(pick(&t, "es"), "Hola");
        assert_eq!(pick(&t, "pt"), "Hello");
        assert_eq!(pick(&Localized::new(), "es"), "");
    }
}
