//! Catalog parsing and validation, GitHub release parsing, asset selection, statuses and the
//! source fallback order.
use otherwise_core::catalog::{self, AppEntry, Release};
use otherwise_core::detect::Installed;
use otherwise_core::net::NetError;
use otherwise_core::pipeline::{self, Status};
use otherwise_core::platform::{Arch, Os, Target};
use otherwise_core::sources::{self, Source};
use std::path::PathBuf;

const MANIFEST: &str = r#"{
  "schema": 1,
  "generatedAt": "2026-11-20T12:00:00Z",
  "apps": [{
    "id": "rama", "name": "rama", "repo": "otherwisetech/rama-rs",
    "tagline": { "en": "Your git history, as a graph you can read.", "es": "Tu historia de git." },
    "description": { "en": "rama shows every branch." },
    "icon": "https://otherwisetech.com/apps/rama.png",
    "homepage": "https://otherwisetech.com/apps/rama",
    "bundleId": "io.github.otherwisetech.rama", "installName": "rama",
    "platforms": {
      "macos": [ { "pattern": "rama-macos-universal.zip", "arch": "universal" }, { "pattern": "rama-macos-aarch64.zip", "arch": "aarch64" } ],
      "windows": [ { "pattern": "rama-windows-x86_64.zip", "arch": "x86_64" } ],
      "linux": [ { "pattern": "rama-linux-x86_64.tar.gz", "arch": "x86_64" } ]
    },
    "minimumOs": { "macos": "11.0" },
    "futureField": true,
    "release": {
      "tag": "v0.2.0", "publishedAt": "2026-11-19T22:37:42Z",
      "url": "https://github.com/otherwisetech/rama-rs/releases/tag/v0.2.0",
      "assets": [
        { "name": "rama-macos-universal.zip", "size": 10, "url": "https://github.com/otherwisetech/rama-rs/releases/download/v0.2.0/rama-macos-universal.zip", "digest": "sha256:3b0e750923554a303c376a3b56df3439aafc0badfb9ee90c9479da60b863d8b4" },
        { "name": "rama-macos-aarch64.zip", "size": 10, "url": "https://github.com/otherwisetech/rama-rs/releases/download/v0.2.0/rama-macos-aarch64.zip" },
        { "name": "rama-windows-x86_64.zip", "size": 10, "url": "https://github.com/otherwisetech/rama-rs/releases/download/v0.2.0/rama-windows-x86_64.zip" },
        { "name": "rama-linux-x86_64.tar.gz", "size": 10, "url": "https://github.com/otherwisetech/rama-rs/releases/download/v0.2.0/rama-linux-x86_64.tar.gz" }
      ]
    }
  }]
}"#;

fn rama() -> AppEntry {
    catalog::parse_manifest(MANIFEST).unwrap().apps.remove(0)
}

fn t(os: Os, arch: Arch) -> Target {
    Target { os, arch }
}

#[test]
fn parses_manifest() {
    let m = catalog::parse_manifest(MANIFEST).unwrap();
    assert_eq!(m.schema, 1);
    let a = &m.apps[0];
    assert_eq!(a.app_name(), "rama");
    assert_eq!(catalog::pick(&a.tagline, "es"), "Tu historia de git.");
    assert_eq!(catalog::pick(&a.description, "fr"), "rama shows every branch.");
    assert_eq!(a.latest_version().unwrap().to_string(), "0.2.0");
    assert_eq!(a.releases_url(), "https://github.com/otherwisetech/rama-rs/releases");
    // round-trips through our own serializer (the saved catalog)
    assert_eq!(catalog::parse_manifest(&catalog::to_json(&m)).unwrap(), m);
}

#[test]
fn rejects_bad_manifests() {
    let bad = |from: &str, to: &str| catalog::parse_manifest(&MANIFEST.replacen(from, to, 1)).is_err();
    assert!(bad("\"schema\": 1", "\"schema\": 2"), "future schema");
    assert!(bad("otherwisetech/rama-rs\"", "someone-else/rama-rs\""), "repo outside the org");
    assert!(bad("https://github.com/otherwisetech/rama-rs/releases/download/v0.2.0/rama-macos-universal.zip", "https://evil.example/rama.zip"), "asset hosted elsewhere");
    assert!(bad("https://github.com/otherwisetech/rama-rs/releases/download/v0.2.0/rama-linux", "https://github.com/otherwisetech/diskmap-rs/releases/download/v0.2.0/rama-linux"), "asset of another repo");
    assert!(bad("\"installName\": \"rama\"", "\"installName\": \"../rama\""), "path in install name");
    assert!(bad("\"id\": \"rama\"", "\"id\": \"\""), "empty id");
    assert!(bad("https://otherwisetech.com/apps/rama.png", "http://otherwisetech.com/apps/rama.png"), "plain http");
    assert!(catalog::parse_manifest("{").is_err());
    let twice = MANIFEST.replacen("\"apps\": [{", "\"apps\": [{ \"id\": \"rama\", \"name\": \"x\", \"repo\": \"otherwisetech/rama-rs\", \"bundleId\": \"a\", \"installName\": \"a\" }, {", 1);
    assert!(catalog::parse_manifest(&twice).is_err(), "duplicate id");
}

#[test]
fn parses_github_release() {
    let json = r###"{ "tag_name": "v0.3.0", "html_url": "https://github.com/otherwisetech/rama-rs/releases/tag/v0.3.0",
        "published_at": "2026-12-01T00:00:00Z", "body": "## Fixes", "draft": false, "prerelease": false,
        "assets": [ { "name": "rama-macos-universal.zip", "size": 5, "digest": "sha256:00",
                      "browser_download_url": "https://github.com/otherwisetech/rama-rs/releases/download/v0.3.0/rama-macos-universal.zip" } ] }"###;
    let r = catalog::parse_github_release(json).unwrap().unwrap();
    assert_eq!(r.tag, "v0.3.0");
    assert_eq!(r.notes.as_deref(), Some("## Fixes"));
    assert_eq!(r.assets[0].digest.as_deref(), Some("sha256:00"));
    assert!(catalog::parse_github_release(&json.replace("\"prerelease\": false", "\"prerelease\": true")).unwrap().is_none());
    assert!(catalog::parse_github_release(&json.replace("\"draft\": false", "\"draft\": true")).unwrap().is_none());

    let mut a = rama();
    assert!(catalog::apply_release(&mut a, r.clone()));
    assert_eq!(a.release.as_ref().unwrap().tag, "v0.3.0");
    // older releases and releases with foreign URLs don't replace what we have
    let mut old = r.clone();
    old.tag = "v0.1.0".into();
    assert!(!catalog::apply_release(&mut a, old));
    let mut foreign = Release { tag: "v0.4.0".into(), ..r };
    foreign.assets[0].url = "https://evil.example/x.zip".into();
    assert!(!catalog::apply_release(&mut a, foreign));
    assert_eq!(a.release.as_ref().unwrap().tag, "v0.3.0");
}

#[test]
fn selects_assets() {
    let a = rama();
    let assets = &a.release.as_ref().unwrap().assets;
    let pick = |target| otherwise_core::platform::select_asset(&a, assets, target).map(|(_, x)| x.name.as_str());
    assert_eq!(pick(t(Os::Macos, Arch::Aarch64)), Some("rama-macos-aarch64.zip"), "native build first");
    assert_eq!(pick(t(Os::Macos, Arch::X86_64)), Some("rama-macos-universal.zip"));
    assert_eq!(pick(t(Os::Windows, Arch::X86_64)), Some("rama-windows-x86_64.zip"));
    assert_eq!(pick(t(Os::Windows, Arch::Aarch64)), Some("rama-windows-x86_64.zip"), "x64 emulation on Windows on ARM");
    assert_eq!(pick(t(Os::Linux, Arch::X86_64)), Some("rama-linux-x86_64.tar.gz"));
    assert_eq!(pick(t(Os::Linux, Arch::Aarch64)), None, "no emulation on Linux");
    // the pattern is listed but the release lacks the file
    let mut b = a.clone();
    b.release.as_mut().unwrap().assets.retain(|x| !x.name.contains("windows"));
    assert!(otherwise_core::platform::select_asset(&b, &b.release.as_ref().unwrap().assets, t(Os::Windows, Arch::X86_64)).is_none());
}

#[test]
fn statuses() {
    let a = rama();
    let mac = t(Os::Macos, Arch::Aarch64);
    let at = |v: Option<&str>| Installed { path: PathBuf::from("/Applications/rama.app"), version: v.map(str::to_string), bundle_id: None };
    assert_eq!(pipeline::status(&a, None, mac), Status::NotInstalled);
    assert_eq!(pipeline::status(&a, Some(&at(Some("0.1.0"))), mac), Status::Update { from: "0.1.0".into(), to: "0.2.0".into() });
    assert_eq!(pipeline::status(&a, Some(&at(Some("0.2.0"))), mac), Status::UpToDate { version: Some("0.2.0".into()) });
    assert_eq!(pipeline::status(&a, Some(&at(Some("0.3.0-dev"))), mac), Status::UpToDate { version: Some("0.3.0-dev".into()) });
    assert_eq!(pipeline::status(&a, Some(&at(Some("0.2.0-rc.1"))), mac), Status::Update { from: "0.2.0-rc.1".into(), to: "0.2.0".into() });
    assert_eq!(pipeline::status(&a, Some(&at(None)), mac), Status::UpToDate { version: None });
    assert_eq!(pipeline::status(&a, None, t(Os::Linux, Arch::Aarch64)), Status::Unsupported);
    let mut none = a.clone();
    none.release = None;
    assert_eq!(pipeline::status(&none, None, mac), Status::NoRelease);
    assert_eq!(pipeline::plan(&a, mac).unwrap().asset.name, "rama-macos-aarch64.zip");
}

#[test]
fn source_fallbacks() {
    let website = || catalog::parse_manifest(MANIFEST).map_err(NetError::Catalog);
    let down = || Err::<catalog::Manifest, _>(NetError::Transport("offline".into()));
    let no_release = |_: &str| Ok(None);
    let limited = |_: &str| Err(NetError::Status(403));
    let bundled = (catalog::bundled(), Source::Bundled);

    // website + GitHub; the launcher adds itself even when the website doesn't list it yet
    let l = sources::load(website, no_release, bundled.clone());
    assert_eq!(l.sources, vec![Source::Website, Source::GitHub]);
    assert!(l.manifest.apps.iter().any(|a| a.is_self()));
    assert_eq!(l.manifest.apps.iter().find(|a| a.id == "rama").unwrap().release.as_ref().unwrap().tag, "v0.2.0");

    // a newer GitHub release wins over the website's copy
    let newer = |repo: &str| {
        Ok((repo == "otherwisetech/rama-rs").then(|| Release { tag: "v0.9.0".into(), published_at: None, url: "https://github.com/otherwisetech/rama-rs/releases/tag/v0.9.0".into(), notes: None, assets: vec![] }))
    };
    let l = sources::load(website, newer, bundled.clone());
    assert_eq!(l.manifest.apps.iter().find(|a| a.id == "rama").unwrap().release.as_ref().unwrap().tag, "v0.9.0");

    // website down, GitHub rate-limited: the fallback catalog, labelled as such
    let l = sources::load(down, limited, bundled.clone());
    assert_eq!(l.sources, vec![Source::Bundled]);
    assert!(!sources::is_live(&l.sources));
    assert_eq!(l.manifest.apps.len(), catalog::bundled().apps.len());
    assert!(!l.errors.is_empty());

    let l = sources::load(down, no_release, (catalog::bundled(), Source::Saved));
    assert_eq!(l.sources, vec![Source::GitHub]);
}

/// A closed-source app releases from a public release-only repo whose name differs from the
/// app's id (`expense-me` from `otherwisetech/expense-me-releases`): it validates, takes its
/// GitHub releases and picks its Apple-silicon-only file, with no special case in the Hub.
#[test]
fn release_only_repo() {
    let m = r#"{ "schema": 1, "apps": [{
      "id": "expense-me", "name": "expense me", "repo": "otherwisetech/expense-me-releases",
      "bundleId": "com.otherwisetech.expense-me", "installName": "expense-me", "appName": "expense me",
      "license": "Proprietary (free to use)",
      "platforms": { "macos": [ { "pattern": "expense-me-macos-arm64.zip", "arch": "arm64" } ] },
      "release": { "tag": "v0.3.17", "url": "https://github.com/otherwisetech/expense-me-releases/releases/tag/v0.3.17",
        "assets": [
          { "name": "expense-me-macos-arm64.zip", "size": 10, "url": "https://github.com/otherwisetech/expense-me-releases/releases/download/v0.3.17/expense-me-macos-arm64.zip", "digest": "sha256:00" },
          { "name": "expense-me-macos-arm64.zip.sha256", "size": 1, "url": "https://github.com/otherwisetech/expense-me-releases/releases/download/v0.3.17/expense-me-macos-arm64.zip.sha256" }
        ] } }] }"#;
    let mut a = catalog::parse_manifest(m).unwrap().apps.remove(0);
    assert_eq!(a.releases_url(), "https://github.com/otherwisetech/expense-me-releases/releases");
    let assets = a.release.as_ref().unwrap().assets.clone();
    let pick = |t| otherwise_core::platform::select_asset(&a, &assets, t).map(|(_, x)| x.name.clone());
    assert_eq!(pick(t(Os::Macos, Arch::Aarch64)).as_deref(), Some("expense-me-macos-arm64.zip"));
    assert_eq!(pick(t(Os::Macos, Arch::X86_64)), None, "no Intel build");
    // the private source repo's files (404 for users) are refused
    let private = m.replacen("expense-me-releases/releases/download", "expense-me/releases/download", 1);
    assert!(catalog::parse_manifest(&private).is_err());

    let gh = r#"{ "tag_name": "v0.3.18", "html_url": "https://github.com/otherwisetech/expense-me-releases/releases/tag/v0.3.18",
        "assets": [ { "name": "expense-me-macos-arm64.zip", "size": 5, "digest": "sha256:11",
          "browser_download_url": "https://github.com/otherwisetech/expense-me-releases/releases/download/v0.3.18/expense-me-macos-arm64.zip" } ] }"#;
    assert!(catalog::apply_release(&mut a, catalog::parse_github_release(gh).unwrap().unwrap()));
    assert_eq!(a.release.as_ref().unwrap().tag, "v0.3.18");
}

/// Vcity (owned by Otherwise) keeps its repositories in the `owsas` org: Vcity Desk releases from
/// `owsas/vcity-desk-releases`, the one repo outside `otherwisetech` the catalog accepts
/// (`catalog::EXTRA_REPOS`). Its files must still come from that repo's own releases, and any
/// other `owsas` repo is refused.
#[test]
fn allowlisted_repo_outside_org() {
    let m = r#"{ "schema": 1, "apps": [{
      "id": "vcity-desk", "name": "Vcity Desk", "repo": "owsas/vcity-desk-releases",
      "bundleId": "io.vcity.desk", "installName": "vcity-desk", "appName": "Vcity Desk",
      "license": "Proprietary",
      "platforms": { "macos": [ { "pattern": "vcity-desk-macos-arm64.zip", "arch": "arm64" } ] },
      "release": { "tag": "v0.1.0", "url": "https://github.com/owsas/vcity-desk-releases/releases/tag/v0.1.0",
        "assets": [
          { "name": "vcity-desk-macos-arm64.zip", "size": 10, "url": "https://github.com/owsas/vcity-desk-releases/releases/download/v0.1.0/vcity-desk-macos-arm64.zip", "digest": "sha256:00" }
        ] } }] }"#;
    let a = catalog::parse_manifest(m).unwrap().apps.remove(0);
    assert_eq!(a.releases_url(), "https://github.com/owsas/vcity-desk-releases/releases");
    let assets = a.release.as_ref().unwrap().assets.clone();
    let pick = |t| otherwise_core::platform::select_asset(&a, &assets, t).map(|(_, x)| x.name.clone());
    assert_eq!(pick(t(Os::Macos, Arch::Aarch64)).as_deref(), Some("vcity-desk-macos-arm64.zip"));

    // only that exact repo: other owsas repos (e.g. the private source monorepo) are refused
    for other in ["owsas/vcity-monorepo", "owsas/vcity-desk-releases-x", "OWSAS/vcity-desk-releases", "owsas/vcity-desk-releases/"] {
        assert!(catalog::parse_manifest(&m.replace("owsas/vcity-desk-releases", other)).is_err(), "{other}");
    }
    // files from another repo are refused even for the allowlisted one
    let elsewhere = m.replacen("owsas/vcity-desk-releases/releases/download", "owsas/vcity-monorepo/releases/download", 1);
    assert!(catalog::parse_manifest(&elsewhere).is_err());
}
