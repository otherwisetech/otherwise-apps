//! Where apps get installed, and where the launcher keeps its own files.
//!
//! - macOS: `/Applications/<Name>.app`, or `~/Applications` when `/Applications` isn't writable.
//! - Windows: `%LOCALAPPDATA%\Programs\Otherwise\<app>\` plus a Start menu shortcut in
//!   `Start Menu\Programs\Otherwise\<Name>.lnk`.
//! - Linux: `~/.local/opt/otherwise/<app>/` plus `~/.local/share/applications/<bundle id>.desktop`.
//!
//! Every path is a field so tests can point the whole layout at a temp dir.
use crate::catalog::AppEntry;
use crate::platform::Os;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub os: Os,
    pub home: PathBuf,
    /// macOS: where `.app` bundles are looked for, in order; new installs go to the first
    /// writable one.
    pub mac_dirs: Vec<PathBuf>,
    /// Windows / Linux: the folder holding one folder per app.
    pub root: PathBuf,
    /// Linux: `.desktop` files. Windows: the Start menu folder for our shortcuts.
    pub menu_dir: PathBuf,
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn home() -> PathBuf {
    env_dir("HOME").or_else(|| env_dir("USERPROFILE")).unwrap_or_else(|| PathBuf::from("."))
}

impl Layout {
    /// The real layout for this machine; `root` overrides the Windows / Linux install folder.
    pub fn system(root: Option<PathBuf>) -> Layout {
        let os = Os::current();
        let home = home();
        Layout::for_os(os, home, root)
    }

    pub fn for_os(os: Os, home: PathBuf, root: Option<PathBuf>) -> Layout {
        match os {
            Os::Macos => Layout { os, mac_dirs: vec![PathBuf::from("/Applications"), home.join("Applications")], root: home.join("Applications"), menu_dir: home.join("Applications"), home },
            Os::Windows => {
                let local = env_dir("LOCALAPPDATA").unwrap_or_else(|| home.join("AppData").join("Local"));
                let roaming = env_dir("APPDATA").unwrap_or_else(|| home.join("AppData").join("Roaming"));
                Layout {
                    os,
                    mac_dirs: vec![],
                    root: root.unwrap_or_else(|| local.join("Programs").join("Otherwise")),
                    menu_dir: roaming.join("Microsoft").join("Windows").join("Start Menu").join("Programs").join("Otherwise"),
                    home,
                }
            }
            Os::Linux => {
                let data = env_dir("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local").join("share"));
                Layout { os, mac_dirs: vec![], root: root.unwrap_or_else(|| home.join(".local").join("opt").join("otherwise")), menu_dir: data.join("applications"), home }
            }
        }
    }

    /// Default Windows / Linux install folder (shown in settings as the placeholder).
    pub fn default_root(os: Os) -> PathBuf {
        Layout::for_os(os, home(), None).root
    }

    /// Every place the app could already be installed, in lookup order.
    pub fn candidates(&self, e: &AppEntry) -> Vec<PathBuf> {
        match self.os {
            Os::Macos => self.mac_dirs.iter().map(|d| d.join(format!("{}.app", e.app_name()))).collect(),
            Os::Windows | Os::Linux => vec![self.root.join(&e.install_name)],
        }
    }

    /// Where a fresh install goes: the existing copy if there is one, else the first writable
    /// location.
    pub fn install_target(&self, e: &AppEntry, existing: Option<&Path>) -> PathBuf {
        if let Some(p) = existing {
            return p.to_path_buf();
        }
        match self.os {
            Os::Macos => {
                let dir = self.mac_dirs.iter().find(|d| writable(d)).or(self.mac_dirs.last()).cloned().unwrap_or_else(|| self.home.join("Applications"));
                dir.join(format!("{}.app", e.app_name()))
            }
            Os::Windows | Os::Linux => self.root.join(&e.install_name),
        }
    }

    /// The executable inside an install.
    pub fn executable(&self, e: &AppEntry, install: &Path) -> PathBuf {
        match self.os {
            Os::Macos => install.join("Contents").join("MacOS").join(&e.install_name),
            Os::Windows => install.join(format!("{}.exe", e.install_name)),
            Os::Linux => install.join(&e.install_name),
        }
    }

    /// Linux: the menu entry we write. Windows: the Start menu shortcut.
    pub fn menu_entry(&self, e: &AppEntry) -> Option<PathBuf> {
        match self.os {
            Os::Macos => None,
            Os::Windows => Some(self.menu_dir.join(format!("{}.lnk", e.name))),
            Os::Linux => Some(self.menu_dir.join(format!("{}.desktop", e.bundle_id))),
        }
    }

    /// Where the app keeps its settings and data (never touched by install / uninstall).
    pub fn data_dir(&self, e: &AppEntry) -> PathBuf {
        match self.os {
            Os::Macos => self.home.join("Library").join("Application Support").join(&e.install_name),
            Os::Windows => env_dir("APPDATA").unwrap_or_else(|| self.home.join("AppData").join("Roaming")).join(&e.install_name),
            Os::Linux => env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| self.home.join(".config")).join(&e.install_name),
        }
    }
}

/// Whether we can create entries in `dir` (it exists and accepts a probe file).
pub fn writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(format!(".otherwise-apps-probe-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// The launcher's own folders (settings, cached catalog, icons, downloads).
pub fn config_dir() -> PathBuf {
    let home = home();
    match Os::current() {
        Os::Macos => home.join("Library/Application Support/otherwise-apps"),
        Os::Windows => env_dir("APPDATA").unwrap_or_else(|| home.join("AppData/Roaming")).join("otherwise-apps"),
        Os::Linux => env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")).join("otherwise-apps"),
    }
}

pub fn cache_dir() -> PathBuf {
    let home = home();
    match Os::current() {
        Os::Macos => home.join("Library/Caches/otherwise-apps"),
        Os::Windows => env_dir("LOCALAPPDATA").unwrap_or_else(|| home.join("AppData/Local")).join("otherwise-apps").join("cache"),
        Os::Linux => env_dir("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache")).join("otherwise-apps"),
    }
}
