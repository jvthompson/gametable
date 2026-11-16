#!/usr/bin/env bash
# Regenerates crates/gametable/tests/fixtures/love_fonts.txt: LÖVE 11.5's own font metrics,
# widths, wraps and glyph data for the bundled fonts at several sizes and UI scales.
# Usage: bash tools/lua_golden/fonts/run.sh
set -euo pipefail
root="$(cd "$(dirname "$0")/../../.." && pwd)"
lovec="${LOVEC:-/c/Program Files/LOVE/lovec.exe}"
staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT
cp "$root"/tools/lua_golden/fonts/*.lua "$staging/"
cp "$root"/assets/fonts/*.ttf "$staging/"
save_folder="$("$lovec" "$staging" | tail -1)"
cp "$save_folder/golden.txt" "$root/crates/gametable/tests/fixtures/love_fonts.txt"
rm -rf "$save_folder"
echo "wrote crates/gametable/tests/fixtures/love_fonts.txt"
