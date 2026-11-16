#!/usr/bin/env bash
# Compiles shaders/*.hlsl to DXIL in assets/shaders/ with the Windows SDK's dxc.
# The .dxil files are committed, so building the client does not need dxc.
# Usage: bash tools/compile_shaders.sh   (from the repository root)
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
dxc="${DXC:-}"
if [ -z "$dxc" ]; then
  dxc="$(ls -d "/c/Program Files (x86)/Windows Kits/10/bin/"*/x64/dxc.exe 2>/dev/null | sort -V | tail -1)"
fi
if [ -z "$dxc" ] || [ ! -x "$dxc" ]; then
  echo "dxc.exe not found; set DXC=/path/to/dxc" >&2
  exit 1
fi

mkdir -p "$root/assets/shaders"
for source in "$root"/shaders/*.hlsl; do
  name="$(basename "$source" .hlsl)"
  case "$name" in
    *.vert) profile=vs_6_0 ;;
    *.frag) profile=ps_6_0 ;;
    *) echo "skipping $name (needs .vert or .frag)"; continue ;;
  esac
  "$dxc" -nologo -T "$profile" -E main -O3 "$source" -Fo "$root/assets/shaders/$name.dxil"
  echo "compiled $name.dxil"
done
