//! Finding installed apps and their versions: `Info.plist` on macOS, the marker file we write
//! next to the binary on Windows and Linux.
use crate::catalog::AppEntry;
use crate::layout::Layout;
use crate::platform::Os;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MARKER: &str = ".otherwise-install.json";

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

/// The `<string>` value of `key` in an XML property list's top-level dict.
pub fn plist_string(xml: &str, key: &str) -> Option<String> {
    let needle = format!("<key>{key}</key>");
    let rest = &xml[xml.find(&needle)? + needle.len()..];
    let rest = rest.trim_start();
    let body = rest.strip_prefix("<string>")?;
    let end = body.find("</string>")?;
    Some(unescape(body[..end].trim()))
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// Reads `Contents/Info.plist` as XML; binary plists are converted with `plutil` first.
pub fn read_info_plist(app: &Path) -> Option<String> {
    let path = app.join("Contents").join("Info.plist");
    let bytes = std::fs::read(&path).ok()?;
    if bytes.starts_with(b"bplist") {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let out = std::process::Command::new("plutil").args(["-convert", "xml1", "-o", "-"]).arg(&path).output().ok()?;
        return out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned());
    }
    String::from_utf8(bytes).ok()
}

/// The first install of `e` found in `layout`'s locations.
pub fn detect(e: &AppEntry, layout: &Layout) -> Option<Installed> {
    for path in layout.candidates(e) {
        match layout.os {
            Os::Macos => {
                if !path.join("Contents").is_dir() {
                    continue;
                }
                let plist = read_info_plist(&path);
                let bundle_id = plist.as_deref().and_then(|p| plist_string(p, "CFBundleIdentifier"));
                // a different app that happens to share the name is not ours to manage
                if bundle_id.as_deref().is_some_and(|b| b != e.bundle_id) {
                    continue;
                }
                let version = plist.as_deref().and_then(|p| plist_string(p, "CFBundleShortVersionString"));
                return Some(Installed { path, version, bundle_id });
            }
            Os::Windows | Os::Linux => {
                if !layout.executable(e, &path).is_file() {
                    continue;
                }
                let version = read_marker(&path).filter(|m| m.id == e.id).map(|m| crate::version::display(&m.version));
                return Some(Installed { path, version, bundle_id: None });
            }
        }
    }
    None
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
