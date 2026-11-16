#!/usr/bin/env bash
# Regenerates assets/fonts/love_roboto_advances.txt from LÖVE 11.5.
# Usage: bash tools/lua_golden/font_advances/run.sh
set -euo pipefail
root="$(cd "$(dirname "$0")/../../.." && pwd)"
lovec="${LOVEC:-/c/Program Files/LOVE/lovec.exe}"
staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT
cp "$root"/tools/lua_golden/font_advances/*.lua "$staging/"
cp "$root"/assets/fonts/Roboto-*.ttf "$staging/"
save_folder="$("$lovec" "$staging" | tail -1)"
cp "$save_folder/advances.txt" "$root/assets/fonts/love_roboto_advances.txt"
rm -rf "$save_folder"
echo "wrote assets/fonts/love_roboto_advances.txt"
