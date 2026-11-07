#!/usr/bin/env bash
# Builds schlau.app in target/release/bundle: the release binary, the icon from assets/icon/schlau.icns
# (made by scripts/app-icon.py) and an Info.plist, signed ad hoc. With --install, copies it to
# ~/Applications as well.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
install=false
[[ "${1:-}" == "--install" ]] && install=true

cargo build --release -q -p schlau-app --manifest-path "$root/Cargo.toml"
version="$(cargo metadata --no-deps --format-version 1 --manifest-path "$root/Cargo.toml" |
  python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "schlau-app"))')"

app="$root/target/release/bundle/schlau.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$root/target/release/schlau" "$app/Contents/MacOS/schlau"
cp "$root/assets/icon/schlau.icns" "$app/Contents/Resources/schlau.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>schlau</string>
  <key>CFBundleDisplayName</key><string>schlau</string>
  <key>CFBundleIdentifier</key><string>io.github.denisshepelin.schlau</string>
  <key>CFBundleExecutable</key><string>schlau</string>
  <key>CFBundleIconFile</key><string>schlau</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
codesign --force --sign - "$app" >/dev/null 2>&1
echo "$app"

if $install; then
  mkdir -p "$HOME/Applications"
  rm -rf "$HOME/Applications/schlau.app"
  cp -R "$app" "$HOME/Applications/schlau.app"
  echo "$HOME/Applications/schlau.app"
fi
