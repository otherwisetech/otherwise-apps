//! Against the real GitHub releases (network): `cargo test -p otherwise-apps-core --test live -- --ignored`.
//! Installs into a temp folder only; nothing is launched and the real Trash isn't used.
use otherwise_core::catalog;
use otherwise_core::detect;
use otherwise_core::install::{self, Request};
use otherwise_core::layout::Layout;
use otherwise_core::net::{self, NetError};
use otherwise_core::pipeline;
use otherwise_core::platform::{Os, Target};
use otherwise_core::sources::{self, Source};
use std::path::Path;
use std::sync::atomic::AtomicBool;

#[test]
#[ignore = "network"]
fn real_release_downloads_verifies_and_installs() {
    let website_down = || Err::<catalog::Parsed, _>(NetError::Transport("skipped".into()));
    let l = sources::load(website_down, net::fetch_latest_release, (catalog::bundled(), Source::Bundled));
    assert!(l.sources.contains(&Source::GitHub), "{:?}", l.errors);
    let e = l.manifest.apps.iter().find(|a| a.id == "rama").unwrap().clone();
    let target = Target::current();
    let plan = pipeline::plan(&e, target).expect("rama ships for this OS");
    let digest = pipeline::fresh_digest(&e, &plan);
    assert!(digest.as_deref().is_some_and(|d| d.starts_with("sha256:")));

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let cancel = AtomicBool::new(false);
    let (archive, sha) = pipeline::fetch_verified(&plan, digest.as_deref(), &root.join("dl"), &|_, _| {}, &cancel).unwrap();

    // the same file against a wrong digest is refused (and deleted)
    let copy = root.join("copy.zip");
    std::fs::copy(&archive, &copy).unwrap();
    let actual = otherwise_core::digest::file_sha256(&copy).unwrap();
    assert!(pipeline::verify_file(&copy, &actual, Some(&format!("sha256:{}", "0".repeat(64)))).is_err());
    assert!(!copy.exists());

    let mut layout = Layout::for_os(target.os, root.join("home"), Some(root.join("opt")));
    if target.os == Os::Macos {
        std::fs::create_dir_all(root.join("Applications")).unwrap();
        layout.mac_dirs = vec![root.join("Applications")];
    }
    layout.menu_dir = root.join("menu");
    let trash = |p: &Path| std::fs::rename(p, root.join("trashed"));
    let req = Request { entry: &e, layout: &layout, archive: &archive, tag: &plan.tag, asset: &plan.asset.name, sha256: &sha, existing: None, overlay: false, now: 1 };
    let out = install::install(&req, &trash).unwrap();
    assert!(out.executable.is_file(), "{}", out.executable.display());
    let found = detect::detect(&e, &layout).unwrap();
    assert_eq!(found.version.as_deref(), Some(otherwise_core::version::display(&plan.tag).as_str()));
}
