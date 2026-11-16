#!/usr/bin/env bash
# Compiles shaders/*.hlsl into assets/shaders/ for every SDL GPU backend:
#   <name>.dxil  Direct3D 12 (Windows)  - the Windows SDK's dxc
#   <name>.spv   Vulkan (Linux, etc.)   - the Vulkan SDK's dxc (-spirv)
#   <name>.msl   Metal (macOS)          - the Vulkan SDK's spirv-cross, from the SPIR-V
# The compiled files are committed, so building the client needs neither SDK; only someone who
# changes a shader does.
# Usage: bash tools/compile_shaders.sh   (from the repository root)
# Overrides: DXC=<Windows SDK dxc>, VULKAN_SDK=<Vulkan SDK folder>.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"

windows_dxc="${DXC:-}"
if [ -z "$windows_dxc" ]; then
  windows_dxc="$(ls -d "/c/Program Files (x86)/Windows Kits/10/bin/"*/x64/dxc.exe 2>/dev/null | sort -V | tail -1)"
fi
if [ -z "$windows_dxc" ] || [ ! -x "$windows_dxc" ]; then
  echo "Windows SDK dxc.exe not found; set DXC=/path/to/dxc" >&2
  exit 1
fi

vulkan_sdk="${VULKAN_SDK:-}"
if [ -z "$vulkan_sdk" ]; then
  echo "VULKAN_SDK is not set (install the LunarG Vulkan SDK)" >&2
  exit 1
fi
vulkan_sdk="$(cygpath -u "$vulkan_sdk" 2>/dev/null || echo "$vulkan_sdk")"
vulkan_bin="$vulkan_sdk/Bin"
[ -d "$vulkan_bin" ] || vulkan_bin="$vulkan_sdk/bin"
spirv_dxc="$vulkan_bin/dxc"
spirv_cross="$vulkan_bin/spirv-cross"
[ -x "$spirv_dxc" ] || spirv_dxc="$spirv_dxc.exe"
[ -x "$spirv_cross" ] || spirv_cross="$spirv_cross.exe"

out="$root/assets/shaders"
mkdir -p "$out"
for source in "$root"/shaders/*.hlsl; do
  name="$(basename "$source" .hlsl)"
  case "$name" in
    *.vert) profile=vs_6_0 ;;
    *.frag) profile=ps_6_0 ;;
    *) echo "skipping $name (needs .vert or .frag)"; continue ;;
  esac
  # The [[vk::...]] attributes only matter for SPIR-V.
  "$windows_dxc" -nologo -Wno-ignored-attributes -T "$profile" -E main -O3 "$source" -Fo "$out/$name.dxil"
  # -fvk-use-dx-layout: constant buffers keep the Direct3D packing the Rust uniform structs use.
  "$spirv_dxc" -nologo -spirv -fspv-target-env=vulkan1.0 -fvk-use-dx-layout -T "$profile" -E main -O3 \
    "$source" -Fo "$out/$name.spv"
  # spirv-cross renames the entry point to main0 (main is reserved in MSL).
  "$spirv_cross" "$out/$name.spv" --msl --msl-version 20100 --output "$out/$name.msl"
  echo "compiled $name (.dxil, .spv, .msl)"
done
