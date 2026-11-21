//! Finding installed apps and their versions: `Info.plist` on macOS, the marker file we write
//! next to the binary on Windows and Linux.
use crate::catalog::AppEntry;
use crate::layout::Layout;
use crate::platform::Os;
use otherwise_link::apps::{AppSpec, Locations, Os as LinkOs};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MARKER: &str = otherwise_link::apps::MARKER;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub path: PathBuf,
    /// None when the bundle has no readable version.
    pub version: Option<String>,
    /// macOS: the bundle's identifier, to tell our app from a same-named one.
    pub bundle_id: Option<String>,
}

/// Written into Windows / Linux installs: those have no Info.plist to read a version from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Marker {
    pub id: String,
    pub version: String,
    pub tag: String,
    pub asset: String,
    pub sha256: String,
    pub installed_at: u64,
}

pub fn read_marker(dir: &Path) -> Option<Marker> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(MARKER)).ok()?).ok()
}

pub fn write_marker(dir: &Path, m: &Marker) -> std::io::Result<()> {
    std::fs::write(dir.join(MARKER), serde_json::to_string_pretty(m).unwrap_or_default())
}

pub use otherwise_link::apps::{plist_string, read_info_plist};

/// The catalog entry as otherwise-link's app description (its detection is shared with every
/// app that looks for its siblings).
pub fn spec(e: &AppEntry) -> AppSpec {
    let known = otherwise_link::apps::known(&e.id).map(|k| k.spec());
    AppSpec {
        id: e.id.clone(),
        name: e.name.clone(),
        bundle_id: e.bundle_id.clone(),
        app_name: e.app_name().to_string(),
        install_name: e.install_name.clone(),
        binary: e.install_name.clone(),
        cli: known.as_ref().map(|k| k.cli.clone()).unwrap_or_default(),
        mcp_args: known.as_ref().map(|k| k.mcp_args.clone()).unwrap_or_default(),
        mcp_write_arg: known.as_ref().and_then(|k| k.mcp_write_arg.clone()),
        open_source: true,
        capabilities: known.map(|k| k.capabilities).unwrap_or_default(),
    }
}

/// Where `layout` looks, as otherwise-link [`Locations`] (no PATH: the launcher manages installs).
pub fn locations(layout: &Layout) -> Locations {
    let os = match layout.os {
        Os::Macos => LinkOs::Macos,
        Os::Windows => LinkOs::Windows,
        Os::Linux => LinkOs::Linux,
    };
    Locations { os, mac_dirs: layout.mac_dirs.clone(), roots: vec![layout.root.clone()], path: vec![] }
}

/// The first install of `e` found in `layout`'s locations.
pub fn detect(e: &AppEntry, layout: &Layout) -> Option<Installed> {
    let found = otherwise_link::apps::detect_install(&spec(e), &locations(layout))?;
    let path = found.path?;
    let version = match layout.os {
        Os::Macos => found.version,
        Os::Windows | Os::Linux => read_marker(&path).filter(|m| m.id == e.id).map(|m| crate::version::display(&m.version)),
    };
    Some(Installed { path, version, bundle_id: found.bundle_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>rama</string>
  <key>CFBundleIdentifier</key><string>io.github.otherwisetech.rama</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>NSHumanReadableCopyright</key><string>Otherwise &amp; co &lt;3</string>
</dict>
</plist>"#;

    #[test]
    fn plist() {
        assert_eq!(plist_string(PLIST, "CFBundleShortVersionString").as_deref(), Some("0.1.0"));
        assert_eq!(plist_string(PLIST, "CFBundleIdentifier").as_deref(), Some("io.github.otherwisetech.rama"));
        assert_eq!(plist_string(PLIST, "NSHumanReadableCopyright").as_deref(), Some("Otherwise & co <3"));
        assert_eq!(plist_string(PLIST, "CFBundleVersion"), None);
        // a key whose value is not a string
        assert_eq!(plist_string("<key>A</key><true/>", "A"), None);
    }
}
