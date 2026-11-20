#!/usr/bin/env bash
# Rebuilds every raster icon from the SVG masters in crates/app/assets/logo:
#   otherwise-apps-icon.svg        → PNG 1024/512/256 (README, Linux, About, website)
#   otherwise-apps-icon-macos.svg  → AppIcon.icns (full bleed; macOS masks it)
#   otherwise-apps-icon.svg        → AppIcon.ico (Windows .exe)
# Needs: cargo (resvg via tools/icon), iconutil (macOS), python3 with Pillow.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
logo="$root/crates/app/assets/logo"
tool="$root/tools/icon"
cargo build --release --manifest-path "$tool/Cargo.toml" --target-dir "$tool/target" -q
icon="$tool/target/release/otherwise-apps-icon"
for s in 1024 512 256; do "$icon" "$logo/otherwise-apps-icon.svg" "$logo/otherwise-apps-icon-$s.png" "$s"; done
work="$(mktemp -d)"
set="$work/AppIcon.iconset"
mkdir -p "$set"
for s in 16 32 128 256 512; do
  "$icon" "$logo/otherwise-apps-icon-macos.svg" "$set/icon_${s}x${s}.png" "$s"
  "$icon" "$logo/otherwise-apps-icon-macos.svg" "$set/icon_${s}x${s}@2x.png" "$((s * 2))"
done
iconutil -c icns -o "$logo/AppIcon.icns" "$set"
"$icon" "$logo/otherwise-apps-icon.svg" "$work/ico-256.png" 256
python3 -c 'import sys; from PIL import Image; Image.open(sys.argv[1]).save(sys.argv[2], sizes=[(16,16),(24,24),(32,32),(48,48),(64,64),(128,128),(256,256)])' "$work/ico-256.png" "$logo/AppIcon.ico"
rm -r "$work"
ls -la "$logo"
