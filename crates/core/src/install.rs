//! Installing a verified archive: unpack next to the destination, check it is the app we expect,
//! swap it in with renames (the old copy goes to the Trash), and add the menu entry on Windows /
//! Linux. Nothing here deletes an installed app or anything in the app's data folder; the only
//! folders removed outright are the temporary ones we unpacked into.
use crate::archive::{self, ArchiveError};
use crate::catalog::AppEntry;
use crate::detect::{self, Marker};
use crate::layout::Layout;
use crate::platform::Os;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub type Trash<'a> = &'a dyn Fn(&Path) -> io::Result<()>;

/// Moves to the system Trash / Recycle Bin.
pub fn system_trash(p: &Path) -> io::Result<()> {
    trash::delete(p).map_err(|e| io::Error::other(e.to_string()))
}

#[derive(Debug)]
pub enum InstallError {
    Io { what: PathBuf, err: io::Error },
    Archive(ArchiveError),
    /// The archive has no `<Name>.app` / binary where we expect it.
    NoPayload,
    /// The bundle in the archive is a different app.
    WrongBundle { expected: String, found: String },
    NotWritable(PathBuf),
    /// Not one of the places this app is installed to; refusing to touch it.
    NotOurs(PathBuf),
    Trash { what: PathBuf, err: io::Error },
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            InstallError::Io { what, err } => i18n::t!("core.install.io", path = what.display(), detail = err),
            InstallError::Archive(e) => e.to_string(),
            InstallError::NoPayload => i18n::t!("core.install.no_payload"),
            InstallError::WrongBundle { expected, found } => i18n::t!("core.install.wrong_bundle", expected = expected, found = found),
            InstallError::NotWritable(p) => i18n::t!("core.install.not_writable", path = p.display()),
            InstallError::NotOurs(p) => i18n::t!("core.install.not_ours", path = p.display()),
            InstallError::Trash { what, err } => i18n::t!("core.install.trash", path = what.display(), detail = err),
        };
        f.write_str(&s)
    }
}

fn io_at(what: &Path) -> impl FnOnce(io::Error) -> InstallError + '_ {
    move |err| InstallError::Io { what: what.to_path_buf(), err }
}

pub struct Request<'a> {
    pub entry: &'a AppEntry,
    pub layout: &'a Layout,
    /// Already downloaded and checked against the published SHA-256.
    pub archive: &'a Path,
    pub tag: &'a str,
    pub asset: &'a str,
    pub sha256: &'a str,
    /// The current install, if any (replaced in place).
    pub existing: Option<&'a Path>,
    /// Windows self-update: the running .exe can be renamed but its folder can't, so the new
    /// files are copied over the old ones instead of swapping folders.
    pub overlay: bool,
    pub now: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub path: PathBuf,
    pub executable: PathBuf,
    /// The previous copy, when it could not be moved to the Trash (left here, untouched).
    pub leftover: Option<PathBuf>,
}

pub fn install(req: &Request, trash: Trash) -> Result<Outcome, InstallError> {
    let e = req.entry;
    let dest = req.layout.install_target(e, req.existing);
    if let Some(existing) = req.existing
        && !req.layout.candidates(e).iter().any(|c| c == existing)
    {
        return Err(InstallError::NotOurs(existing.to_path_buf()));
    }
    let parent = dest.parent().ok_or_else(|| InstallError::NotWritable(dest.clone()))?.to_path_buf();
    fs::create_dir_all(&parent).map_err(|_| InstallError::NotWritable(parent.clone()))?;
    if !crate::layout::writable(&parent) {
        return Err(InstallError::NotWritable(parent));
    }
    // unpacked next to the destination so the final move is a same-volume rename
    let staging = parent.join(format!(".otherwise-apps-staging-{}-{}-{}", e.id, std::process::id(), req.now));
    let unpacked = staging.join("new");
    let result = (|| {
        archive::extract(req.archive, &unpacked).map_err(InstallError::Archive)?;
        let payload = find_payload(&unpacked, e, req.layout.os)?;
        match req.layout.os {
            Os::Macos => strip_quarantine(&payload),
            Os::Windows | Os::Linux => {
                let m = Marker { id: e.id.clone(), version: crate::version::display(req.tag), tag: req.tag.into(), asset: req.asset.into(), sha256: req.sha256.into(), installed_at: req.now };
                detect::write_marker(&payload, &m).map_err(io_at(&payload))?;
            }
        }
        let leftover = if req.overlay && dest.exists() { overlay(&payload, &dest, &e.install_name)? } else { swap(&payload, &dest, &staging, trash)? };
        Ok(leftover)
    })();
    // our own temporary unpack folder; the previous install is never inside it unless the Trash
    // refused it, and then `staging` is kept
    let _ = fs::remove_dir_all(&unpacked);
    // non-recursive: these only go when empty
    let tidy = || {
        let _ = fs::remove_dir(staging.join("old"));
        let _ = fs::remove_dir(&staging);
    };
    let leftover = result.inspect_err(|_| tidy())?;
    if leftover.is_none() {
        tidy();
    }
    let executable = req.layout.executable(e, &dest);
    match req.layout.os {
        Os::Linux => write_desktop_entry(e, req.layout, &dest, &executable)?,
        Os::Windows => {
            if let Some(lnk) = req.layout.menu_entry(e) {
                // a missing shortcut doesn't make the install fail; the app still runs
                let _ = create_shortcut(&lnk, &executable, &dest, crate::catalog::pick(&e.tagline, "en"));
            }
        }
        Os::Macos => {}
    }
    Ok(Outcome { path: dest, executable, leftover })
}

/// The `.app` (macOS) or the folder holding the binary (Windows / Linux) inside an unpacked
/// archive: at the top or one folder down.
pub fn find_payload(root: &Path, e: &AppEntry, os: Os) -> Result<PathBuf, InstallError> {
    let mut dirs = vec![root.to_path_buf()];
    if let Ok(rd) = fs::read_dir(root) {
        let mut subs: Vec<PathBuf> = rd.flatten().map(|d| d.path()).filter(|p| p.is_dir()).collect();
        subs.sort();
        dirs.extend(subs);
    }
    match os {
        Os::Macos => {
            let name = format!("{}.app", e.app_name());
            let app = dirs.iter().map(|d| d.join(&name)).find(|p| p.join("Contents").is_dir()).ok_or(InstallError::NoPayload)?;
            let found = detect::read_info_plist(&app).and_then(|p| detect::plist_string(&p, "CFBundleIdentifier")).unwrap_or_default();
            if found != e.bundle_id {
                return Err(InstallError::WrongBundle { expected: e.bundle_id.clone(), found });
            }
            if !app.join("Contents").join("MacOS").join(&e.install_name).is_file() {
                return Err(InstallError::NoPayload);
            }
            Ok(app)
        }
        Os::Windows | Os::Linux => {
            let exe = if os == Os::Windows { format!("{}.exe", e.install_name) } else { e.install_name.clone() };
            dirs.into_iter().find(|d| d.join(&exe).is_file()).ok_or(InstallError::NoPayload)
        }
    }
}

/// Removes the download quarantine flag from the bundle we just verified, and only from it.
fn strip_quarantine(app: &Path) {
    if cfg!(target_os = "macos") {
        let _ = std::process::Command::new("xattr").args(["-dr", "com.apple.quarantine"]).arg(app).status();
    }
}

/// Renames the old install aside, renames the new one in, then sends the old one to the Trash.
/// If the second rename fails the old one is put back.
fn swap(payload: &Path, dest: &Path, staging: &Path, trash: Trash) -> Result<Option<PathBuf>, InstallError> {
    if !dest.exists() {
        fs::rename(payload, dest).map_err(io_at(dest))?;
        return Ok(None);
    }
    let old_dir = staging.join("old");
    fs::create_dir_all(&old_dir).map_err(io_at(&old_dir))?;
    // keeps its own name, so the Trash shows "rama.app", not a temp name
    let old = old_dir.join(dest.file_name().ok_or_else(|| InstallError::NotWritable(dest.to_path_buf()))?);
    fs::rename(dest, &old).map_err(io_at(dest))?;
    if let Err(err) = fs::rename(payload, dest) {
        let _ = fs::rename(&old, dest);
        return Err(InstallError::Io { what: dest.to_path_buf(), err });
    }
    match trash(&old) {
        Ok(()) => Ok(None),
        Err(_) => Ok(Some(old)),
    }
}

/// Windows self-update: renames the running exe to `.old` (removed on next start) and copies the
/// new files over.
fn overlay(payload: &Path, dest: &Path, install_name: &str) -> Result<Option<PathBuf>, InstallError> {
    let exe = dest.join(format!("{install_name}.exe"));
    if exe.exists() {
        let old = dest.join(format!("{install_name}.exe.old"));
        let _ = fs::remove_file(&old);
        fs::rename(&exe, &old).map_err(io_at(&exe))?;
    }
    copy_tree(payload, dest)?;
    Ok(None)
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), InstallError> {
    fs::create_dir_all(to).map_err(io_at(to))?;
    for entry in fs::read_dir(from).map_err(io_at(from))?.flatten() {
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            fs::copy(&src, &dst).map_err(io_at(&dst))?;
        }
    }
    Ok(())
}

/// Deletes the `.exe.old` left by a Windows self-update (our own renamed binary).
pub fn cleanup_overlay(dir: &Path, install_name: &str) {
    let _ = fs::remove_file(dir.join(format!("{install_name}.exe.old")));
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    pub trashed: Vec<PathBuf>,
    /// Settings and data, left in place.
    pub data_dir: PathBuf,
}

/// Moves the app (and its menu entry) to the Trash. Only paths this app installs to are accepted.
pub fn uninstall(e: &AppEntry, layout: &Layout, installed: &Path, trash: Trash) -> Result<Removed, InstallError> {
    if !layout.candidates(e).iter().any(|c| c == installed) {
        return Err(InstallError::NotOurs(installed.to_path_buf()));
    }
    trash(installed).map_err(|err| InstallError::Trash { what: installed.to_path_buf(), err })?;
    let mut trashed = vec![installed.to_path_buf()];
    if let Some(menu) = layout.menu_entry(e).filter(|m| m.exists()) {
        if trash(&menu).is_ok() {
            trashed.push(menu);
        }
    }
    Ok(Removed { trashed, data_dir: layout.data_dir(e) })
}

// ── Linux menu entry ─────────────────────────────────────────────────────────────────────────

/// Characters that force quoting in a desktop-entry `Exec` argument.
const RESERVED: &[char] = &[' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')', '`'];

/// One `Exec` argument, quoted and escaped per the Desktop Entry spec (quoting rules first, then
/// the general backslash escaping of string values).
pub fn exec_arg(s: &str) -> String {
    if !s.contains(RESERVED) {
        return s.to_string();
    }
    let mut q = String::from("\"");
    for c in s.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            q.push('\\');
        }
        q.push(c);
    }
    q.push('"');
    q.replace('\\', "\\\\")
}

/// The `.desktop` file we install: the one shipped in the archive with `Exec`, `TryExec` and
/// `Icon` pointing at the installed files, or a minimal one if the archive has none.
pub fn desktop_entry(e: &AppEntry, shipped: Option<&str>, exe: &Path, icon: Option<&Path>) -> String {
    let exe_s = exe.to_string_lossy();
    let icon_line = icon.map(|i| format!("Icon={}", i.to_string_lossy()));
    let mut out = Vec::new();
    let mut in_main = false;
    let mut saw = (false, false);
    if let Some(src) = shipped.filter(|s| s.contains("[Desktop Entry]")) {
        for line in src.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                if in_main {
                    finish_main(&mut out, &mut saw, &icon_line, &exe_s, e);
                }
                in_main = t == "[Desktop Entry]";
                out.push(line.to_string());
                continue;
            }
            if in_main {
                if let Some(v) = t.strip_prefix("Exec=") {
                    // keep field codes such as %f after the program
                    let rest: Vec<&str> = v.split_whitespace().skip(1).collect();
                    let mut l = format!("Exec={}", exec_arg(&exe_s));
                    for r in rest {
                        l.push(' ');
                        l.push_str(r);
                    }
                    out.push(l);
                    saw.0 = true;
                    continue;
                }
                if t.starts_with("TryExec=") || t.starts_with("X-Otherwise-App=") {
                    continue;
                }
                if t.starts_with("Icon=") {
                    if let Some(l) = &icon_line {
                        out.push(l.clone());
                    } else {
                        out.push(line.to_string());
                    }
                    saw.1 = true;
                    continue;
                }
            }
            out.push(line.to_string());
        }
        if in_main {
            finish_main(&mut out, &mut saw, &icon_line, &exe_s, e);
        }
    } else {
        out.push("[Desktop Entry]".into());
        out.push("Type=Application".into());
        out.push(format!("Name={}", e.name));
        let tagline = crate::catalog::pick(&e.tagline, "en");
        if !tagline.is_empty() {
            out.push(format!("Comment={tagline}"));
        }
        out.push("Terminal=false".into());
        out.push("Categories=Utility;".into());
        finish_main(&mut out, &mut saw, &icon_line, &exe_s, e);
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

fn finish_main(out: &mut Vec<String>, saw: &mut (bool, bool), icon_line: &Option<String>, exe: &str, e: &AppEntry) {
    // insert before trailing blank lines of the section
    let mut at = out.len();
    while at > 0 && out[at - 1].trim().is_empty() {
        at -= 1;
    }
    let mut add = Vec::new();
    if !saw.0 {
        add.push(format!("Exec={}", exec_arg(exe)));
    }
    if !saw.1
        && let Some(l) = icon_line
    {
        add.push(l.clone());
    }
    add.push(format!("TryExec={exe}"));
    add.push(format!("X-Otherwise-App={}", e.id));
    for (i, l) in add.into_iter().enumerate() {
        out.insert(at + i, l);
    }
    *saw = (true, true);
}

fn write_desktop_entry(e: &AppEntry, layout: &Layout, dir: &Path, exe: &Path) -> Result<(), InstallError> {
    let Some(path) = layout.menu_entry(e) else { return Ok(()) };
    let shipped = fs::read_to_string(dir.join(format!("{}.desktop", e.install_name))).ok();
    let icon = dir.join(format!("{}.png", e.install_name));
    let text = desktop_entry(e, shipped.as_deref(), exe, icon.is_file().then_some(icon.as_path()));
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).map_err(io_at(p))?;
    }
    fs::write(&path, text).map_err(io_at(&path))?;
    Ok(())
}

// ── Windows Start menu shortcut ──────────────────────────────────────────────────────────────

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// PowerShell that writes a `.lnk` through WScript.Shell.
pub fn shortcut_script(lnk: &Path, exe: &Path, workdir: &Path, description: &str) -> String {
    format!(
        "$s = (New-Object -ComObject WScript.Shell).CreateShortcut({}); $s.TargetPath = {}; $s.WorkingDirectory = {}; $s.Description = {}; $s.Save()",
        ps_quote(&lnk.to_string_lossy()),
        ps_quote(&exe.to_string_lossy()),
        ps_quote(&workdir.to_string_lossy()),
        ps_quote(description)
    )
}

fn create_shortcut(lnk: &Path, exe: &Path, workdir: &Path, description: &str) -> io::Result<()> {
    if let Some(p) = lnk.parent() {
        fs::create_dir_all(p)?;
    }
    let mut cmd = std::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", &shortcut_script(lnk, exe, workdir, description)]);
    crate::process::no_window(&mut cmd);
    let st = cmd.status()?;
    if st.success() { Ok(()) } else { Err(io::Error::other(format!("powershell exited with {st}"))) } // i18n-ok: diagnostic
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_quoting() {
        assert_eq!(exec_arg("/home/ana/.local/opt/otherwise/rama/rama"), "/home/ana/.local/opt/otherwise/rama/rama");
        assert_eq!(exec_arg("/home/ana maría/rama"), "\"/home/ana maría/rama\"");
        // quoting escapes $ with a backslash, then string escaping doubles every backslash
        assert_eq!(exec_arg("/x/$y"), "\"/x/\\\\$y\"");
    }

    #[test]
    fn shortcut_quoting() {
        let s = shortcut_script(Path::new(r"C:\Users\O'Neil\Start\rama.lnk"), Path::new(r"C:\Apps\rama\rama.exe"), Path::new(r"C:\Apps\rama"), "It's fast");
        assert!(s.contains(r"CreateShortcut('C:\Users\O''Neil\Start\rama.lnk')"));
        assert!(s.contains("$s.Description = 'It''s fast'"));
        assert!(s.ends_with("$s.Save()"));
    }
}
