#!/usr/bin/env bash
# Regenerates golden test output from CardTable's original Lua code, so the Rust port's tests compare
# against what the original actually produces.
#
# Usage: tools/lua_golden/run.sh <generator>     e.g.  tools/lua_golden/run.sh models
#
# Needs LÖVE 11.5 (lovec.exe) and the original source at F:/Projects/cardflux-love. Override with
# LOVEC=... and CARDTABLE_SOURCE=... if they live elsewhere.
set -euo pipefail

generator="$1"
workspace_root="$(cd "$(dirname "$0")/../.." && pwd)"
lovec="${LOVEC:-/c/Program Files/LOVE/lovec.exe}"
cardtable_source="${CARDTABLE_SOURCE:-/f/Projects/cardflux-love}"
fixtures_directory="$workspace_root/crates/gametable-core/tests/fixtures/$generator"

# Assemble a throwaway LÖVE project: the generator scripts plus the original lib/ and src/core/.
staging_directory="$(mktemp -d)"
trap 'rm -rf "$staging_directory"' EXIT
cp "$workspace_root"/tools/lua_golden/*.lua "$staging_directory/"
mkdir -p "$staging_directory/src"
cp -r "$cardtable_source/lib" "$staging_directory/lib"
cp -r "$cardtable_source/src/core" "$staging_directory/src/core"
cp -r "$cardtable_source/src/game" "$staging_directory/src/game"

mkdir -p "$fixtures_directory/expected"
rm -f "$fixtures_directory"/expected/*
list_file="$staging_directory/inputs.txt"
ls "$fixtures_directory/inputs" > "$list_file"

to_native_path() { if command -v cygpath > /dev/null; then cygpath -m "$1"; else echo "$1"; fi; }
"$lovec" "$(to_native_path "$staging_directory")" "$generator" \
  "$(to_native_path "$fixtures_directory")" "$(to_native_path "$list_file")"
