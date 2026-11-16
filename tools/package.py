#!/usr/bin/env python3
"""Packages CardTable for this platform, and optionally starts a release. Port of the original's
tools/build.ps1, for Windows, macOS and Linux.

  python tools/package.py              build dist/cardtable-<platform>.zip for this platform
  python tools/package.py --skip-build package the existing release binary
  python tools/package.py --universal  (macOS) a universal arm64 + x86_64 binary
  python tools/package.py --publish    bump BUILD in version.rs, commit, tag v<name>+<build> and
                                       push; GitHub Actions then builds all three packages and
                                       publishes the release (.github/workflows/ci.yml)

Package layouts (what the in-app updater expects, see crates/gametable/src/platform/updater.rs):
  Windows  cardtable-windows.zip       CardTable.exe, assets/, the DLLs, LICENSE at the zip's root
                                       (as the LOVE build did, so its updater can move to this one)
  Linux    cardtable-linux-x86_64.zip  cardtable, assets/, libpdfium.so, cardtable.desktop, LICENSE
  macOS    cardtable-macos.zip         CardTable.app (ad-hoc signed; libraries in Contents/Frameworks)
The app's version comes from crates/gametable/src/version.rs; Windows gets its icon and version
resource from crates/gametable/build.rs.
"""

import argparse
import os
import re
import shutil
import stat
import subprocess
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VERSION_FILE = ROOT / "crates" / "gametable" / "src" / "version.rs"
DIST = ROOT / "dist"
# Cargo's build folder (CARGO_TARGET_DIR when set, e.g. a Linux build from a Windows checkout).
TARGET = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
# Assets the app reads at runtime (shaders and the standard deck are compiled into the binary).
ASSET_EXCLUDES = {"games", "shaders", "icon.ico"}
BUNDLE_IDENTIFIER = "io.github.jvthompson.cardtable"


def step(message):
    print(f"==> {message}", flush=True)


def platform_name():
    if sys.platform.startswith("win"):
        return "windows"
    if sys.platform == "darwin":
        return "macos"
    return "linux"


def read_version():
    text = VERSION_FILE.read_text(encoding="utf-8")
    name = re.search(r'VERSION_NAME: &str = "([^"]+)"', text).group(1)
    build = int(re.search(r"BUILD: u32 = (\d+)", text).group(1))
    return name, build, text


def run(command, **options):
    print("   $ " + " ".join(str(part) for part in command), flush=True)
    subprocess.run([str(part) for part in command], check=True, cwd=options.pop("cwd", ROOT), **options)


def cargo_environment():
    environment = dict(os.environ)
    # .cargo/config.toml points CMAKE at Visual Studio's copy on the development machine; anywhere
    # cmake is on PATH (CI, macOS, Linux) use that instead.
    if "CMAKE" not in environment and shutil.which("cmake"):
        environment["CMAKE"] = "cmake"
    return environment


def build_binary(platform, universal):
    if platform == "macos" and universal:
        binaries = []
        for target in ("aarch64-apple-darwin", "x86_64-apple-darwin"):
            run(["rustup", "target", "add", target])
            run(["cargo", "build", "--release", "-p", "gametable", "--target", target], env=cargo_environment())
            binaries.append(TARGET / target / "release" / "gametable")
        universal_binary = TARGET / "universal-gametable"
        run(["lipo", "-create", "-output", universal_binary, *binaries])
        return universal_binary
    run(["cargo", "build", "--release", "-p", "gametable"], env=cargo_environment())
    return TARGET / "release" / ("gametable.exe" if platform == "windows" else "gametable")


def copy_assets(destination):
    for item in (ROOT / "assets").iterdir():
        if item.name in ASSET_EXCLUDES:
            continue
        if item.is_dir():
            shutil.copytree(item, destination / item.name)
        else:
            shutil.copy2(item, destination / item.name)


def copy_runtime_libraries(platform, destination):
    """Everything in runtime/<platform>/ (pdfium, libwebp, the Discord SDK and their licenses)."""
    folder = ROOT / "runtime" / platform
    if not folder.is_dir():
        return
    destination.mkdir(parents=True, exist_ok=True)
    for item in folder.iterdir():
        if item.is_file():
            shutil.copy2(item, destination / item.name)


def make_executable(path):
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def zip_folder_contents(folder, zip_path):
    """Zips what's inside `folder` (forward-slash names, Unix permissions kept)."""
    if zip_path.exists():
        zip_path.unlink()
    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for path in sorted(folder.rglob("*")):
            relative = path.relative_to(folder).as_posix()
            if path.is_dir():
                continue
            info = zipfile.ZipInfo.from_file(path, relative)
            info.compress_type = zipfile.ZIP_DEFLATED
            with open(path, "rb") as source:
                archive.writestr(info, source.read())


def package_windows(binary, name, build):
    folder = DIST / "CardTable"
    folder.mkdir(parents=True)
    shutil.copy2(binary, folder / "CardTable.exe")
    (folder / "assets").mkdir()
    copy_assets(folder / "assets")
    copy_runtime_libraries("windows", folder)
    shutil.copy2(ROOT / "LICENSE", folder / "LICENSE")
    zip_path = DIST / "cardtable-windows.zip"
    zip_folder_contents(folder, zip_path)
    return zip_path


DESKTOP_ENTRY = """[Desktop Entry]
Type=Application
Name=CardTable
Comment=A tabletop sandbox for trading card games
Exec=cardtable
Icon=cardtable
Categories=Game;CardGame;
Terminal=false
"""


def package_linux(binary, name, build):
    folder = DIST / "cardtable"
    folder.mkdir(parents=True)
    shutil.copy2(binary, folder / "cardtable")
    make_executable(folder / "cardtable")
    (folder / "assets").mkdir()
    copy_assets(folder / "assets")
    copy_runtime_libraries("linux", folder)
    shutil.copy2(ROOT / "assets" / "icon.png", folder / "cardtable.png")
    (folder / "cardtable.desktop").write_text(DESKTOP_ENTRY, encoding="utf-8")
    shutil.copy2(ROOT / "LICENSE", folder / "LICENSE")
    zip_path = DIST / "cardtable-linux-x86_64.zip"
    zip_folder_contents(folder, zip_path)
    return zip_path


INFO_PLIST = """<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>CardTable</string>
  <key>CFBundleDisplayName</key><string>CardTable</string>
  <key>CFBundleIdentifier</key><string>{identifier}</string>
  <key>CFBundleExecutable</key><string>CardTable</string>
  <key>CFBundleIconFile</key><string>CardTable</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>{name}</string>
  <key>CFBundleVersion</key><string>{build}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.card-games</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
"""


def make_icns(png, destination):
    """CardTable.icns from icon.png with the system's sips and iconutil."""
    iconset = DIST / "CardTable.iconset"
    iconset.mkdir()
    for size in (16, 32, 128, 256, 512):
        for scale in (1, 2):
            pixels = size * scale
            suffix = "" if scale == 1 else "@2x"
            run(["sips", "-z", pixels, pixels, png, "--out", iconset / f"icon_{size}x{size}{suffix}.png"],
                stdout=subprocess.DEVNULL)
    run(["iconutil", "-c", "icns", iconset, "-o", destination])
    shutil.rmtree(iconset)


def package_macos(binary, name, build):
    bundle = DIST / "CardTable.app"
    contents = bundle / "Contents"
    (contents / "MacOS").mkdir(parents=True)
    (contents / "Resources" / "assets").mkdir(parents=True)
    shutil.copy2(binary, contents / "MacOS" / "CardTable")
    make_executable(contents / "MacOS" / "CardTable")
    copy_assets(contents / "Resources" / "assets")
    copy_runtime_libraries("macos", contents / "Frameworks")
    shutil.copy2(ROOT / "LICENSE", contents / "Resources" / "LICENSE")
    (contents / "Info.plist").write_text(INFO_PLIST.format(identifier=BUNDLE_IDENTIFIER, name=name, build=build),
                                         encoding="utf-8")
    make_icns(ROOT / "assets" / "icon.png", contents / "Resources" / "CardTable.icns")
    # Ad-hoc signature: Apple silicon refuses unsigned code. Not notarized, so the first launch
    # of a downloaded copy needs right-click > Open.
    run(["codesign", "--force", "--deep", "--sign", "-", bundle])
    zip_path = DIST / "cardtable-macos.zip"
    # The zip holds CardTable.app itself; ditto keeps the bundle's attributes intact.
    run(["ditto", "-c", "-k", "--keepParent", bundle, zip_path])
    return zip_path


def publish():
    status = subprocess.run(["git", "status", "--porcelain"], cwd=ROOT, capture_output=True, text=True, check=True)
    if status.stdout.strip():
        sys.exit("Working tree has uncommitted changes -- commit or stash them before publishing:\n" + status.stdout)
    name, build, text = read_version()
    build += 1
    VERSION_FILE.write_text(re.sub(r"(BUILD: u32 = )\d+", rf"\g<1>{build}", text, count=1), encoding="utf-8")
    tag = f"v{name}+{build}"
    step(f"Releasing {tag}")
    run(["git", "add", VERSION_FILE])
    run(["git", "commit", "-m", f"Release {tag}"])
    run(["git", "tag", tag])
    run(["git", "push"])
    run(["git", "push", "origin", tag])
    step(f"Pushed {tag}; GitHub Actions builds the packages and publishes the release.")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--skip-build", action="store_true", help="package the existing release binary")
    parser.add_argument("--universal", action="store_true", help="macOS: build for Apple silicon and Intel")
    parser.add_argument("--publish", action="store_true", help="bump the build, tag it and push (CI releases it)")
    parser.add_argument("--dist", type=Path, help="output folder (default: dist/ in the repository)")
    arguments = parser.parse_args()
    if arguments.publish:
        publish()
        return
    global DIST
    if arguments.dist:
        DIST = arguments.dist.resolve()
    platform = platform_name()
    name, build, _ = read_version()
    step(f"Packaging CardTable {name}+{build} for {platform}")
    if arguments.skip_build:
        if platform == "macos" and arguments.universal:
            binary = TARGET / "universal-gametable"
        else:
            binary = TARGET / "release" / ("gametable.exe" if platform == "windows" else "gametable")
    else:
        binary = build_binary(platform, arguments.universal)
    if DIST.exists():
        shutil.rmtree(DIST)
    DIST.mkdir()
    packager = {"windows": package_windows, "linux": package_linux, "macos": package_macos}[platform]
    zip_path = packager(binary, name, build)
    step(f"Built {zip_path} ({zip_path.stat().st_size / 1024 / 1024:.1f} MB)")


if __name__ == "__main__":
    main()
