//! Install / update / uninstall against temp folders standing in for /Applications,
//! ~/.local/opt and %LOCALAPPDATA%. The real Trash is never touched: a fake one moves things into
//! a temp folder.
use otherwise_core::catalog::{self, AppEntry};
use otherwise_core::detect;
use otherwise_core::digest;
use otherwise_core::install::{self, InstallError, Request};
use otherwise_core::layout::Layout;
use otherwise_core::pipeline::{self, FetchError};
use otherwise_core::platform::Os;
use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;

fn rama() -> AppEntry {
    catalog::bundled().apps.into_iter().find(|a| a.id == "rama").unwrap()
}

fn plist(id: &str, version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>{id}</string>
  <key>CFBundleExecutable</key><string>rama</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
</dict>
</plist>"#
    )
}

/// A zip laid out like `ditto -c -k --keepParent rama.app`.
fn fake_app_zip(dir: &Path, bundle_id: &str, version: &str, extra: &[(&str, &str)]) -> PathBuf {
    let path = dir.join(format!("rama-{version}-macos-universal.zip"));
    let mut z = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    let opts = SimpleFileOptions::default();
    z.add_directory("rama.app/", opts).unwrap();
    z.add_directory("rama.app/Contents/", opts).unwrap();
    z.add_directory("rama.app/Contents/MacOS/", opts).unwrap();
    z.start_file("rama.app/Contents/Info.plist", opts).unwrap();
    z.write_all(plist(bundle_id, version).as_bytes()).unwrap();
    z.start_file("rama.app/Contents/MacOS/rama", opts.unix_permissions(0o755)).unwrap();
    z.write_all(format!("#!/bin/sh\necho rama {version}\n").as_bytes()).unwrap();
    z.add_symlink("rama.app/Contents/Current", "MacOS", opts).unwrap();
    z.start_file("__MACOSX/rama.app/._Info.plist", opts).unwrap();
    z.write_all(b"junk").unwrap();
    for (name, body) in extra {
        z.start_file(*name, opts).unwrap();
        z.write_all(body.as_bytes()).unwrap();
    }
    z.finish().unwrap();
    path
}

/// A .tar.gz laid out like the Linux release job: `rama-linux-x86_64/{rama, rama.desktop, rama.png}`.
fn fake_linux_tar(dir: &Path, version: &str) -> PathBuf {
    let path = dir.join(format!("rama-{version}-linux-x86_64.tar.gz"));
    let gz = flate2::write::GzEncoder::new(fs::File::create(&path).unwrap(), flate2::Compression::fast());
    let mut tar = tar::Builder::new(gz);
    let mut add = |name: &str, body: &[u8], mode: u32| {
        let mut h = tar::Header::new_gnu();
        h.set_size(body.len() as u64);
        h.set_mode(mode);
        h.set_cksum();
        tar.append_data(&mut h, name, body).unwrap();
    };
    add("rama-linux-x86_64/rama", format!("#!/bin/sh\necho {version}\n").as_bytes(), 0o755);
    add("rama-linux-x86_64/rama.desktop", b"[Desktop Entry]\nType=Application\nName=rama\nExec=rama %f\nIcon=rama\nTerminal=false\nCategories=Development;\n", 0o644);
    add("rama-linux-x86_64/rama.png", b"\x89PNG", 0o644);
    add("rama-linux-x86_64/README.md", b"readme", 0o644);
    tar.into_inner().unwrap().finish().unwrap();
    path
}

fn fake_windows_zip(dir: &Path, version: &str) -> PathBuf {
    let path = dir.join(format!("rama-{version}-windows-x86_64.zip"));
    let mut z = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    let opts = SimpleFileOptions::default();
    z.start_file("rama-windows-x86_64/rama.exe", opts).unwrap();
    z.write_all(format!("MZ {version}").as_bytes()).unwrap();
    z.start_file("rama-windows-x86_64/README.md", opts).unwrap();
    z.write_all(b"readme").unwrap();
    z.finish().unwrap();
    path
}

struct Env {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    trash_dir: PathBuf,
    trashed: RefCell<Vec<PathBuf>>,
}

impl Env {
    fn new() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let trash_dir = root.join("Trash");
        fs::create_dir_all(&trash_dir).unwrap();
        fs::create_dir_all(root.join("downloads")).unwrap();
        Env { _tmp: tmp, root, trash_dir, trashed: RefCell::new(vec![]) }
    }

    fn trash(&self, p: &Path) -> std::io::Result<()> {
        let n = self.trashed.borrow().len();
        let dest = self.trash_dir.join(format!("{n}-{}", p.file_name().unwrap().to_string_lossy()));
        fs::rename(p, &dest)?;
        self.trashed.borrow_mut().push(dest);
        Ok(())
    }

    fn mac(&self) -> Layout {
        let home = self.root.join("home");
        let mut l = Layout::for_os(Os::Macos, home.clone(), None);
        fs::create_dir_all(self.root.join("Applications")).unwrap();
        l.mac_dirs = vec![self.root.join("Applications"), home.join("Applications")];
        l
    }

    fn downloads(&self) -> PathBuf {
        self.root.join("downloads")
    }
}

fn req<'a>(e: &'a AppEntry, l: &'a Layout, archive: &'a Path, tag: &'a str, existing: Option<&'a Path>) -> Request<'a> {
    Request { entry: e, layout: l, archive, tag, asset: "rama-macos-universal.zip", sha256: "00", existing, overlay: false, now: 1_800_000_000 }
}

#[test]
#[cfg_attr(windows, ignore = "the fake bundle has a symlink; creating one needs privileges on Windows")]
fn macos_install_update_uninstall() {
    let env = Env::new();
    let e = rama();
    let l = env.mac();
    let trash = |p: &Path| env.trash(p);
    assert!(detect::detect(&e, &l).is_none());

    // fresh install
    let v1 = fake_app_zip(&env.downloads(), "io.github.otherwisetech.rama", "0.1.0", &[]);
    let out = install::install(&req(&e, &l, &v1, "v0.1.0", None), &trash).unwrap();
    assert_eq!(out.path, env.root.join("Applications/rama.app"));
    assert_eq!(out.executable, env.root.join("Applications/rama.app/Contents/MacOS/rama"));
    assert!(out.leftover.is_none());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&out.executable).unwrap().permissions().mode() & 0o111, 0o111, "executable bit kept");
        assert_eq!(fs::read_link(out.path.join("Contents/Current")).unwrap(), PathBuf::from("MacOS"));
    }
    assert!(!out.path.join("__MACOSX").exists() && !env.root.join("Applications/__MACOSX").exists());
    let found = detect::detect(&e, &l).unwrap();
    assert_eq!(found.version.as_deref(), Some("0.1.0"));
    assert!(env.trashed.borrow().is_empty());

    // the app's data lives elsewhere and must survive everything below
    let data = l.data_dir(&e);
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("settings.json"), "{\"language\": \"es\"}").unwrap();

    // update: the new copy is in place, the old one went to the (fake) Trash under its own name
    let v2 = fake_app_zip(&env.downloads(), "io.github.otherwisetech.rama", "0.2.0", &[]);
    let out = install::install(&req(&e, &l, &v2, "v0.2.0", Some(&found.path)), &trash).unwrap();
    assert_eq!(detect::detect(&e, &l).unwrap().version.as_deref(), Some("0.2.0"));
    let trashed = env.trashed.borrow().clone();
    assert_eq!(trashed.len(), 1);
    assert!(trashed[0].file_name().unwrap().to_string_lossy().ends_with("rama.app"));
    assert_eq!(detect::plist_string(&detect::read_info_plist(&trashed[0]).unwrap(), "CFBundleShortVersionString").as_deref(), Some("0.1.0"));
    // no staging folders left behind
    let leftovers: Vec<_> = fs::read_dir(env.root.join("Applications")).unwrap().flatten().map(|d| d.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(leftovers, vec!["rama.app"]);
    assert_eq!(fs::read_to_string(data.join("settings.json")).unwrap(), "{\"language\": \"es\"}");

    // a different app in the archive is refused and the installed copy stays
    let evil = fake_app_zip(&env.downloads(), "com.example.notrama", "9.9.9", &[]);
    let err = install::install(&req(&e, &l, &evil, "v9.9.9", Some(&out.path)), &trash).unwrap_err();
    assert!(matches!(err, InstallError::WrongBundle { .. }), "{err:?}");
    assert_eq!(detect::detect(&e, &l).unwrap().version.as_deref(), Some("0.2.0"));

    // uninstall only accepts our own locations, and leaves the data folder
    let stranger = env.root.join("Applications/Other.app");
    fs::create_dir_all(&stranger).unwrap();
    assert!(matches!(install::uninstall(&e, &l, &stranger, &trash), Err(InstallError::NotOurs(_))));
    assert!(stranger.exists());
    let removed = install::uninstall(&e, &l, &out.path, &trash).unwrap();
    assert_eq!(removed.trashed, vec![out.path.clone()]);
    assert_eq!(removed.data_dir, data);
    assert!(!out.path.exists());
    assert!(data.join("settings.json").exists());
    assert!(detect::detect(&e, &l).is_none());
}

#[test]
#[cfg_attr(windows, ignore = "the fake bundle has a symlink; creating one needs privileges on Windows")]
fn macos_trash_failure_keeps_old_copy() {
    let env = Env::new();
    let e = rama();
    let l = env.mac();
    let ok = |p: &Path| env.trash(p);
    let v1 = fake_app_zip(&env.downloads(), "io.github.otherwisetech.rama", "0.1.0", &[]);
    let first = install::install(&req(&e, &l, &v1, "v0.1.0", None), &ok).unwrap();
    let v2 = fake_app_zip(&env.downloads(), "io.github.otherwisetech.rama", "0.2.0", &[]);
    let refuse = |_: &Path| Err(std::io::Error::other("no Trash here"));
    let out = install::install(&req(&e, &l, &v2, "v0.2.0", Some(&first.path)), &refuse).unwrap();
    let old = out.leftover.expect("old copy kept, not deleted");
    assert!(old.join("Contents/Info.plist").exists());
    assert_eq!(detect::detect(&e, &l).unwrap().version.as_deref(), Some("0.2.0"));
}

#[test]
fn refuses_unsafe_archives() {
    let env = Env::new();
    let e = rama();
    let l = env.mac();
    let trash = |p: &Path| env.trash(p);
    let bad = env.downloads().join("bad.zip");
    let mut z = zip::ZipWriter::new(fs::File::create(&bad).unwrap());
    z.start_file("rama.app/../../escape.txt", SimpleFileOptions::default()).unwrap();
    z.write_all(b"x").unwrap();
    z.finish().unwrap();
    let err = install::install(&req(&e, &l, &bad, "v0.1.0", None), &trash).unwrap_err();
    assert!(matches!(err, InstallError::Archive(_)), "{err:?}");
    assert!(!env.root.join("escape.txt").exists());

    let link = env.downloads().join("link.zip");
    let mut z = zip::ZipWriter::new(fs::File::create(&link).unwrap());
    z.add_symlink("rama.app/Contents/evil", "../../../../etc", SimpleFileOptions::default()).unwrap();
    z.finish().unwrap();
    let r = install::install(&req(&e, &l, &link, "v0.1.0", None), &trash);
    assert!(matches!(r, Err(InstallError::Archive(_))), "{r:?}");

    // archive without the app
    let empty = fake_app_zip(&env.downloads(), "io.github.otherwisetech.rama", "0.1.0", &[]);
    let mut other = e.clone();
    other.app_name = Some("Something Else".into());
    assert!(matches!(install::install(&req(&other, &l, &empty, "v0.1.0", None), &trash), Err(InstallError::NoPayload)));
    let left: Vec<_> = fs::read_dir(env.root.join("Applications")).unwrap().flatten().collect();
    assert!(left.is_empty(), "staging cleaned up after failures: {left:?}");
}

#[test]
fn digest_mismatch_is_refused() {
    let env = Env::new();
    let file = env.downloads().join("rama-macos-universal.zip");
    fs::write(&file, b"tampered").unwrap();
    let sha = digest::file_sha256(&file).unwrap();
    let good = format!("sha256:{}", digest::hex(&sha));
    assert!(pipeline::verify_file(&file, &sha, Some(&good)).is_ok());
    assert!(file.exists());

    let wrong = format!("sha256:{}", "ab".repeat(32));
    let err = pipeline::verify_file(&file, &sha, Some(&wrong)).unwrap_err();
    assert!(matches!(err, FetchError::Mismatch { .. }));
    assert!(!file.exists(), "a refused download is deleted");

    fs::write(&file, b"x").unwrap();
    assert!(matches!(pipeline::verify_file(&file, &sha, None), Err(FetchError::NoDigest)));
    assert!(!file.exists());

    // no digest published: refused before downloading anything
    let e = rama();
    let target = otherwise_core::platform::Target { os: Os::Macos, arch: otherwise_core::platform::Arch::Aarch64 };
    let plan = pipeline::plan(&e, target).unwrap();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    assert!(matches!(pipeline::fetch_verified(&plan, None, &env.downloads(), &|_, _| {}, &cancel), Err(FetchError::NoDigest)));
}

#[test]
#[cfg(not(windows))]
fn linux_install_writes_menu_entry() {
    let env = Env::new();
    let e = rama();
    let home = env.root.join("home");
    let mut l = Layout::for_os(Os::Linux, home.clone(), Some(env.root.join("opt")));
    l.menu_dir = home.join(".local/share/applications");
    let trash = |p: &Path| env.trash(p);
    let v1 = fake_linux_tar(&env.downloads(), "0.1.0");
    let r = Request { entry: &e, layout: &l, archive: &v1, tag: "v0.1.0", asset: "rama-linux-x86_64.tar.gz", sha256: "abc", existing: None, overlay: false, now: 1 };
    let out = install::install(&r, &trash).unwrap();
    assert_eq!(out.path, env.root.join("opt/rama"));
    assert!(out.executable.is_file());
    let found = detect::detect(&e, &l).unwrap();
    assert_eq!(found.version.as_deref(), Some("0.1.0"));
    let marker = detect::read_marker(&out.path).unwrap();
    assert_eq!((marker.tag.as_str(), marker.sha256.as_str()), ("v0.1.0", "abc"));

    let desktop = fs::read_to_string(l.menu_entry(&e).unwrap()).unwrap();
    assert!(desktop.contains(&format!("Exec={} %f", out.executable.display())), "{desktop}");
    assert!(desktop.contains(&format!("Icon={}", out.path.join("rama.png").display())), "{desktop}");
    assert!(desktop.contains("X-Otherwise-App=rama"));
    assert!(desktop.contains("Categories=Development;"));

    let v2 = fake_linux_tar(&env.downloads(), "0.2.0");
    let r = Request { archive: &v2, tag: "v0.2.0", existing: Some(&found.path), ..r };
    install::install(&r, &trash).unwrap();
    assert_eq!(detect::detect(&e, &l).unwrap().version.as_deref(), Some("0.2.0"));
    assert_eq!(env.trashed.borrow().len(), 1);

    let removed = install::uninstall(&e, &l, &found.path, &trash).unwrap();
    assert_eq!(removed.trashed.len(), 2, "folder and .desktop file");
    assert!(!l.menu_entry(&e).unwrap().exists());
}

#[test]
fn windows_layout_and_overlay() {
    let env = Env::new();
    let e = rama();
    let mut l = Layout::for_os(Os::Windows, env.root.join("home"), Some(env.root.join("Programs/Otherwise")));
    l.menu_dir = env.root.join("StartMenu");
    let trash = |p: &Path| env.trash(p);
    let v1 = fake_windows_zip(&env.downloads(), "0.1.0");
    let r = Request { entry: &e, layout: &l, archive: &v1, tag: "v0.1.0", asset: "rama-windows-x86_64.zip", sha256: "abc", existing: None, overlay: false, now: 1 };
    let out = install::install(&r, &trash).unwrap();
    assert_eq!(out.executable, env.root.join("Programs/Otherwise/rama/rama.exe"));
    assert_eq!(detect::detect(&e, &l).unwrap().version.as_deref(), Some("0.1.0"));

    // self-update style: the running exe is renamed, files are copied over the same folder
    let v2 = fake_windows_zip(&env.downloads(), "0.2.0");
    let r = Request { archive: &v2, tag: "v0.2.0", existing: Some(&out.path), overlay: true, ..r };
    install::install(&r, &trash).unwrap();
    assert_eq!(fs::read_to_string(&out.executable).unwrap(), "MZ 0.2.0");
    assert_eq!(fs::read_to_string(out.path.join("rama.exe.old")).unwrap(), "MZ 0.1.0");
    assert_eq!(detect::detect(&e, &l).unwrap().version.as_deref(), Some("0.2.0"));
    install::cleanup_overlay(&out.path, "rama");
    assert!(!out.path.join("rama.exe.old").exists());
    assert!(env.trashed.borrow().is_empty());
}

#[test]
fn desktop_entry_generation() {
    let e = rama();
    let exe = Path::new("/home/ana maría/.local/opt/otherwise/rama/rama");
    let icon = Path::new("/home/ana maría/.local/opt/otherwise/rama/rama.png");
    let shipped = "[Desktop Entry]\nType=Application\nName=rama\nExec=rama %f\nTryExec=rama\nIcon=rama\nMimeType=inode/directory;\n\n[Desktop Action new]\nName=New window\nExec=rama --new\n";
    let out = install::desktop_entry(&e, Some(shipped), exe, Some(icon));
    assert_eq!(
        out,
        "[Desktop Entry]\nType=Application\nName=rama\nExec=\"/home/ana maría/.local/opt/otherwise/rama/rama\" %f\nIcon=/home/ana maría/.local/opt/otherwise/rama/rama.png\nMimeType=inode/directory;\nTryExec=/home/ana maría/.local/opt/otherwise/rama/rama\nX-Otherwise-App=rama\n\n[Desktop Action new]\nName=New window\nExec=rama --new\n"
    );
    let minimal = install::desktop_entry(&e, None, Path::new("/opt/rama/rama"), None);
    assert!(minimal.starts_with("[Desktop Entry]\nType=Application\nName=rama\nComment="));
    assert!(minimal.contains("Exec=/opt/rama/rama\n"));
    assert!(minimal.ends_with("TryExec=/opt/rama/rama\nX-Otherwise-App=rama\n"));
}

#[test]
fn macos_detect_ignores_foreign_bundles() {
    let env = Env::new();
    let e = rama();
    let l = env.mac();
    let app = env.root.join("Applications/rama.app/Contents");
    fs::create_dir_all(&app).unwrap();
    fs::write(app.join("Info.plist"), plist("com.someone.rama", "5.0.0")).unwrap();
    assert!(detect::detect(&e, &l).is_none());
    // the user-level ~/Applications is the second place we look
    let user = l.mac_dirs[1].join("rama.app/Contents");
    fs::create_dir_all(&user).unwrap();
    fs::write(user.join("Info.plist"), plist("io.github.otherwisetech.rama", "0.1.0")).unwrap();
    assert_eq!(detect::detect(&e, &l).unwrap().path, l.mac_dirs[1].join("rama.app"));
}

#[test]
fn detection_is_otherwise_links() {
    let e = rama();
    let s = detect::spec(&e);
    assert_eq!((s.id.as_str(), s.binary.as_str(), s.app_name.as_str()), ("rama", "rama", e.app_name()));
    assert!(s.has(otherwise_link::apps::Capability::DeepLinks), "capabilities come from otherwise-link's table");
    let env = Env::new();
    let l = env.mac();
    let loc = detect::locations(&l);
    assert_eq!(loc.mac_dirs, l.mac_dirs);
    assert!(loc.path.is_empty(), "the launcher only manages its own install folders");
}
