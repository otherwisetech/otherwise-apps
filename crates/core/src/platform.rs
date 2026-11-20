//! Which operating system and CPU we run on, and which release file fits them.
use crate::catalog::{AppEntry, Asset, AssetPattern};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Os {
    Macos,
    Windows,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arch {
    X86_64,
    Aarch64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Target {
    pub os: Os,
    pub arch: Arch,
}

impl Os {
    /// The key used in the manifest's `platforms` / `minimumOs` maps.
    pub fn key(self) -> &'static str {
        match self {
            Os::Macos => "macos",
            Os::Windows => "windows",
            Os::Linux => "linux",
        }
    }

    pub fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Macos
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

impl Arch {
    pub fn key(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }

    pub fn current() -> Arch {
        if cfg!(target_arch = "aarch64") { Arch::Aarch64 } else { Arch::X86_64 }
    }
}

impl Target {
    pub fn current() -> Target {
        Target { os: Os::current(), arch: Arch::current() }
    }
}

/// Lower is better; None means the build can't run here. Native first, then universal, then
/// x86_64 under emulation (Rosetta on macOS, Prism on Windows; Linux has none).
fn rank(pattern_arch: &str, t: Target) -> Option<u8> {
    match (pattern_arch, t.arch) {
        (a, arch) if a == arch.key() => Some(0),
        ("arm64", Arch::Aarch64) => Some(0),
        ("universal", _) if t.os == Os::Macos => Some(1),
        ("x86_64", Arch::Aarch64) if t.os != Os::Linux => Some(2),
        _ => None,
    }
}

/// `*` matches any run of characters (`rama-*-macos-universal.zip`); everything else literally.
pub fn glob(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !name.starts_with(first) || name.len() < first.len() + last.len() || !name.ends_with(last) {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    true
}

/// The pattern and release file to install on `t`, if the app ships one for it.
pub fn select_asset<'a>(entry: &'a AppEntry, assets: &'a [Asset], t: Target) -> Option<(&'a AssetPattern, &'a Asset)> {
    let patterns = entry.platforms.get(t.os.key())?;
    patterns
        .iter()
        .filter_map(|p| Some((rank(&p.arch, t)?, p)))
        .filter_map(|(r, p)| assets.iter().find(|a| glob(&p.pattern, &a.name)).map(|a| (r, p, a)))
        .min_by_key(|(r, _, _)| *r)
        .map(|(_, p, a)| (p, a))
}

/// Whether the catalog lists a build for this OS at all (even if not released yet).
pub fn supports(entry: &AppEntry, t: Target) -> bool {
    entry.platforms.get(t.os.key()).is_some_and(|ps| ps.iter().any(|p| rank(&p.arch, t).is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globbing() {
        assert!(glob("rama-macos-universal.zip", "rama-macos-universal.zip"));
        assert!(!glob("rama-macos-universal.zip", "rama-macos-universal.zip.sha256"));
        assert!(glob("rama-*-macos.zip", "rama-0.2.0-macos.zip"));
        assert!(glob("*.tar.gz", "x.tar.gz"));
        assert!(glob("a*b*c", "a-1-b-2-c"));
        assert!(!glob("a*b*c", "a-1-c-2-b"));
        assert!(!glob("ab*ba", "aba"));
    }
}
