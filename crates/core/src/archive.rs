//! Unpacking release archives (`.zip`, `.tar.gz`) into a folder we own. Entries that would land
//! outside it (absolute paths, `..`, symlinks pointing out) are refused, not skipped.
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub enum ArchiveError {
    Io(io::Error),
    Format(String),
    /// An entry tried to escape the destination.
    Unsafe(String),
    Unsupported(String),
}

impl From<io::Error> for ArchiveError {
    fn from(e: io::Error) -> Self {
        ArchiveError::Io(e)
    }
}

impl std::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArchiveError::Io(e) => f.write_str(&i18n::t!("core.archive.io", detail = e)),
            ArchiveError::Format(e) => f.write_str(&i18n::t!("core.archive.format", detail = e)),
            ArchiveError::Unsafe(p) => f.write_str(&i18n::t!("core.archive.unsafe", path = p)),
            ArchiveError::Unsupported(n) => f.write_str(&i18n::t!("core.archive.unsupported", name = n)),
        }
    }
}

/// macOS metadata that `ditto`/Finder may add to zips; never part of the app.
fn is_junk(p: &Path) -> bool {
    p.components().any(|c| matches!(c, Component::Normal(s) if s == "__MACOSX" || s.to_string_lossy().starts_with("._") || s == ".DS_Store"))
}

/// A relative path with only normal components.
fn clean(p: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => out.push(s),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// A symlink at `link` (relative to the archive root) pointing at `target` stays inside.
fn link_stays_inside(link: &Path, target: &Path) -> bool {
    if target.is_absolute() {
        return false;
    }
    let mut depth: i32 = link.components().count() as i32 - 1;
    for c in target.components() {
        match c {
            Component::ParentDir => depth -= 1,
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => return false,
        }
        if depth < 0 {
            return false;
        }
    }
    true
}

pub fn extract(archive: &Path, dest: &Path) -> Result<(), ArchiveError> {
    let name = archive.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    fs::create_dir_all(dest)?;
    if name.ends_with(".zip") {
        extract_zip(archive, dest)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        extract_tar_gz(archive, dest)
    } else {
        Err(ArchiveError::Unsupported(name))
    }
}

fn extract_zip(archive: &Path, dest: &Path) -> Result<(), ArchiveError> {
    let mut zip = zip::ZipArchive::new(fs::File::open(archive)?).map_err(|e| ArchiveError::Format(e.to_string()))?;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| ArchiveError::Format(e.to_string()))?;
        let raw = f.name().to_string();
        let Some(rel) = f.enclosed_name().and_then(|p| clean(&p)) else { return Err(ArchiveError::Unsafe(raw)) };
        if is_junk(&rel) {
            continue;
        }
        let out = dest.join(&rel);
        if f.is_dir() {
            fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        if f.is_symlink() {
            let mut target = String::new();
            io::Read::read_to_string(&mut f, &mut target)?;
            if !link_stays_inside(&rel, Path::new(&target)) {
                return Err(ArchiveError::Unsafe(raw));
            }
            symlink(Path::new(&target), &out)?;
            continue;
        }
        let mut w = fs::File::create(&out)?;
        io::copy(&mut f, &mut w)?;
        #[cfg(unix)]
        if let Some(mode) = f.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&out, fs::Permissions::from_mode(mode & 0o777))?;
        }
    }
    Ok(())
}

fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<(), ArchiveError> {
    let gz = flate2::read::GzDecoder::new(fs::File::open(archive)?);
    let mut tar = tar::Archive::new(gz);
    tar.set_preserve_permissions(true);
    tar.set_overwrite(true);
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let raw = path.display().to_string();
        let Some(rel) = clean(&path) else { return Err(ArchiveError::Unsafe(raw)) };
        if is_junk(&rel) {
            continue;
        }
        let kind = entry.header().entry_type();
        if kind.is_symlink() || kind.is_hard_link() {
            let target = entry.link_name()?.map(|t| t.into_owned()).unwrap_or_default();
            let ok = if kind.is_hard_link() { clean(&target).is_some() } else { link_stays_inside(&rel, &target) };
            if !ok {
                return Err(ArchiveError::Unsafe(raw));
            }
        }
        // unpack_in refuses paths outside `dest`, including through symlinked parents
        if !entry.unpack_in(dest)? {
            return Err(ArchiveError::Unsafe(raw));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_checks() {
        assert!(link_stays_inside(Path::new("A.app/Contents/Frameworks/x"), Path::new("Versions/Current/x")));
        assert!(link_stays_inside(Path::new("a/b/c"), Path::new("../d")));
        assert!(!link_stays_inside(Path::new("a/c"), Path::new("../../etc")));
        assert!(!link_stays_inside(Path::new("c"), Path::new("/etc/passwd")));
        assert!(clean(Path::new("a/../b")).is_none());
        assert!(is_junk(Path::new("__MACOSX/A.app/._x")));
        assert!(!is_junk(Path::new("A.app/Contents/Info.plist")));
    }
}
