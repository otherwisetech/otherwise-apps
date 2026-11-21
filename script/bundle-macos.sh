#!/usr/bin/env bash
# Wraps the `otherwise-apps` binary in "Otherwise Apps.app" so Finder and the Dock show the icon.
# Usage: script/bundle-macos.sh [debug|release]   (default: release; builds first)
#        UNIVERSAL=1 script/bundle-macos.sh       (release for Apple silicon + Intel, joined with lipo)
# Honours CARGO_TARGET_DIR (binary read from, and the .app written to, that directory).
set -euo pipefail
profile="${1:-release}"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
target="${CARGO_TARGET_DIR:-target}"
bin=otherwise-apps
if [ "${UNIVERSAL:-0}" = 1 ]; then
  profile=release
  for t in aarch64-apple-darwin x86_64-apple-darwin; do
    rustup target add "$t" >/dev/null
    cargo build -p otherwise-apps-app --release --target "$t"
  done
  mkdir -p "$target/release"
  lipo -create -output "$target/release/$bin" "$target/aarch64-apple-darwin/release/$bin" "$target/x86_64-apple-darwin/release/$bin"
elif [ "$profile" = release ]; then cargo build -p otherwise-apps-app --release; else cargo build -p otherwise-apps-app; fi
version="$(cargo pkgid -p otherwise-apps-app | sed 's/.*[#@]//')"
app="$target/$profile/Otherwise Apps.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$target/$profile/$bin" "$app/Contents/MacOS/$bin"
cp crates/app/assets/logo/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Otherwise Apps</string>
  <key>CFBundleDisplayName</key><string>Otherwise Apps</string>
  <key>CFBundleIdentifier</key><string>io.github.otherwisetech.otherwise-apps</string>
  <key>CFBundleExecutable</key><string>$bin</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key><string>io.github.otherwisetech.otherwise-apps</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>CFBundleURLSchemes</key><array><string>otherwise</string></array>
    </dict>
  </array>
  <key>NSHumanReadableCopyright</key><string>Otherwise Technologies LLC · MIT OR Apache-2.0</string>
</dict>
</plist>
PLIST
plutil -lint -s "$app/Contents/Info.plist"
# Make LaunchServices pick up the (possibly changed) icon.
touch "$app"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$app" || true
echo "$app"
