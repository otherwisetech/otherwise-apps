//! Putting the catalog together: otherwisetech.com's manifest first, then GitHub's latest
//! release for every app (fresher, and it carries the SHA-256 digests), else the last catalog we
//! saved, else the one bundled with the launcher.
use crate::catalog::{self, Manifest, Parsed, Release, SELF_ID};
use crate::net::NetError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Website,
    GitHub,
    /// The catalog saved after the last successful check.
    Saved,
    Bundled,
}

#[derive(Debug)]
pub struct Loaded {
    pub manifest: Manifest,
    pub sources: Vec<Source>,
    pub errors: Vec<String>,
}

/// `fallback` is the saved catalog (with `Source::Saved`) or the bundled one.
pub fn load(website: impl FnOnce() -> Result<Parsed, NetError>, github: impl Fn(&str) -> Result<Option<Release>, NetError>, fallback: (Manifest, Source)) -> Loaded {
    let mut sources = Vec::new();
    let mut errors = Vec::new();
    let mut manifest = match website() {
        Ok(p) => {
            sources.push(Source::Website);
            errors.extend(p.skipped.iter().map(ToString::to_string));
            p.manifest
        }
        Err(e) => {
            errors.push(e.to_string());
            fallback.0.clone()
        }
    };
    // the launcher always lists itself, even before the website publishes it
    if !manifest.apps.iter().any(|a| a.id == SELF_ID)
        && let Some(me) = catalog::bundled().apps.into_iter().find(|a| a.id == SELF_ID)
    {
        manifest.apps.push(me);
    }
    let mut github_ok = false;
    for app in &mut manifest.apps {
        match github(&app.repo) {
            Ok(Some(r)) => {
                catalog::apply_release(app, r);
                github_ok = true;
            }
            Ok(None) => github_ok = true,
            Err(e) => {
                let limited = matches!(e, NetError::Status(403 | 429));
                errors.push(e.to_string());
                // out of anonymous API quota: the other repos would fail the same way
                if limited {
                    break;
                }
            }
        }
    }
    if github_ok {
        sources.push(Source::GitHub);
    }
    if sources.is_empty() {
        sources.push(fallback.1);
    }
    Loaded { manifest, sources, errors }
}

/// Whether the loaded catalog came from the network and is worth saving.
pub fn is_live(sources: &[Source]) -> bool {
    sources.iter().any(|s| matches!(s, Source::Website | Source::GitHub))
}
