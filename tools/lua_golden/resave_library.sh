#!/usr/bin/env bash
# Re-saves a CardTable library with BOTH the original Lua models and the Rust port, into separate
# output folders, then diffs them. The library itself is only read.
#
# Usage: tools/lua_golden/resave_library.sh <libraryRoot> <outputDir>
#   e.g. tools/lua_golden/resave_library.sh /d/CardfluxLibrary "$TMP/resave"
set -euo pipefail

library_root="$(cd "$1" && pwd)"
output_dir="$2"
workspace_root="$(cd "$(dirname "$0")/../.." && pwd)"
lovec="${LOVEC:-/c/Program Files/LOVE/lovec.exe}"
cardtable_source="${CARDTABLE_SOURCE:-/f/Projects/cardflux-love}"

mkdir -p "$output_dir"
output_dir="$(cd "$output_dir" && pwd)"
case "$output_dir/" in
  "$library_root"/*) echo "refusing to write inside the library: $output_dir" >&2; exit 2 ;;
esac
lua_output="$output_dir/lua"
rust_output="$output_dir/rust"
rm -rf "$lua_output" "$rust_output"

to_native_path() { if command -v cygpath > /dev/null; then cygpath -m "$1"; else echo "$1"; fi; }

# The same discovery rules as the Rust compat_check example and src/data/game_loader.lua.
staging_directory="$(mktemp -d)"
trap 'rm -rf "$staging_directory"' EXIT
list_file="$staging_directory/files.txt"
: > "$list_file"
add() { # kind, relative path
  mkdir -p "$lua_output/$(dirname "$2")"
  printf '%s\t%s\t%s\n' "$1" "$2" "$(to_native_path "$lua_output")" >> "$list_file"
}
cd "$library_root"
for game_folder in game_library/*/; do
  game_folder="${game_folder%/}"
  gamedef="$(find "$game_folder" -maxdepth 1 -type f -iname gamedef.json | sort | head -1)"
  [ -n "$gamedef" ] || continue
  add gamedef "$gamedef"
  decks_folder="$game_folder/decks"
  [ -d "$game_folder/_decks" ] && decks_folder="$game_folder/_decks"
  if [ -d "$decks_folder" ]; then
    find "$decks_folder" -maxdepth 1 -type f -iname '*.json' | sort | while read -r f; do add deck "$f"; done
  fi
  if [ -d "$game_folder/savegames" ]; then
    find "$game_folder/savegames" -maxdepth 1 -type f -iname '*.json' | sort | while read -r f; do add save "$f"; done
  fi
done
if [ -d deck_library ]; then
  find deck_library -mindepth 2 -maxdepth 2 -type f -iname '*.json' | sort | while read -r f; do add deck "$f"; done
fi
cd - > /dev/null

cp "$workspace_root"/tools/lua_golden/*.lua "$staging_directory/"
mkdir -p "$staging_directory/src"
cp -r "$cardtable_source/lib" "$staging_directory/lib"
cp -r "$cardtable_source/src/core" "$staging_directory/src/core"
"$lovec" "$(to_native_path "$staging_directory")" resave_library \
  "$(to_native_path "$library_root")" "$(to_native_path "$list_file")"

(cd "$workspace_root" && cargo run -q -p gametable-core --example compat_check -- \
  "$library_root" --resaved-output "$rust_output" > "$output_dir/compat_report.txt") || true

echo "Lua files: $(find "$lua_output" -type f | wc -l), Rust files: $(find "$rust_output" -type f | wc -l)"
if diff -r -q "$lua_output" "$rust_output"; then
  echo "IDENTICAL: every re-saved file matches the original Lua byte for byte"
else
  echo "DIFFERENCES FOUND (see above)"
  exit 1
fi
