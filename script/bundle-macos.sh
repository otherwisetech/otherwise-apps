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
    cargo build -p otherwise-apps-app -p otherwise-helper --release --target "$t"
  done
  mkdir -p "$target/release"
  for b in "$bin" otherwise-helper; do
    lipo -create -output "$target/release/$b" "$target/aarch64-apple-darwin/release/$b" "$target/x86_64-apple-darwin/release/$b"
  done
elif [ "$profile" = release ]; then cargo build -p otherwise-apps-app -p otherwise-helper --release; else cargo build -p otherwise-apps-app -p otherwise-helper; fi
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
# The launcher's own background jobs (Otherwise Helper finds them here; nothing to register).
"$app/Contents/MacOS/$bin" background manifest > "$app/Contents/Resources/otherwise-background.json"
# Otherwise Helper: a menu-bar app (no Dock icon) inside ours, its own bundle id so
# notifications and System Settings → Login Items show it by name.
helper="$app/Contents/Library/LoginItems/Otherwise Helper.app"
mkdir -p "$helper/Contents/MacOS" "$helper/Contents/Resources"
cp "$target/$profile/otherwise-helper" "$helper/Contents/MacOS/otherwise-helper"
cp crates/app/assets/logo/AppIcon.icns "$helper/Contents/Resources/AppIcon.icns"
cat > "$helper/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Otherwise Helper</string>
  <key>CFBundleDisplayName</key><string>Otherwise Helper</string>
  <key>CFBundleIdentifier</key><string>io.github.otherwisetech.otherwise-helper</string>
  <key>CFBundleExecutable</key><string>otherwise-helper</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHumanReadableCopyright</key><string>Otherwise Technologies LLC · MIT OR Apache-2.0</string>
</dict>
</plist>
PLIST
plutil -lint -s "$helper/Contents/Info.plist"
# Ad-hoc signatures (inner bundle first) so notifications and the login item are attributed.
codesign --force --sign - "$helper"
codesign --force --sign - "$app"
# Make LaunchServices pick up the (possibly changed) icon.
touch "$app"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$app" || true
echo "$app"
