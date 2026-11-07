#!/usr/bin/env bash
# Renders the app headlessly over the fixture mailbox (attachments indexed first, with EmbeddingGemma 2
# when it is downloaded), one PNG per scene in scripts/scenes, in dark
# and light: docs/screenshots/<scene>-<theme>.png.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$root/docs/screenshots}"
mkdir -p "$out"
cargo build --release -q -p schlau-app --manifest-path "$root/Cargo.toml"
bin="$root/target/release/schlau"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
for scene in "$root"/scripts/scenes/*.json; do
  name="$(basename "$scene" .json)"
  for theme in dark light; do
    db="$work/$name-$theme.sqlite"
    "$bin" --fixtures "$root/fixtures/mailbox" --db "$db" --index-attachments --script "$scene" \
      --snap "$out/$name-$theme.png" --theme "$theme" --size 1280x800 --at 6 >/dev/null 2>&1
    echo "$out/$name-$theme.png"
  done
done
