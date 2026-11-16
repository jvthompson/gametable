//! Windows: gives the executable its icon (`assets/icon.ico`) and a version resource from
//! `src/version.rs`, as the original's `tools/build.ps1` patched into the fused `love.exe`. The
//! resource script is compiled with the Windows SDK's `rc.exe` and linked into the binaries. Other
//! platforms get their icon and version from the package (macOS `Info.plist`, Linux `.desktop`).

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=src/version.rs");
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    println!("cargo:rerun-if-env-changed=RC");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" || target_env != "msvc" {
        return;
    }
    let Some(resource_compiler) = find_resource_compiler() else {
        println!("cargo:warning=rc.exe not found (Windows SDK); building without an icon or version resource");
        return;
    };
    let manifest_folder = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let version_source = std::fs::read_to_string(manifest_folder.join("src/version.rs")).unwrap();
    let version_name = string_constant(&version_source, "VERSION_NAME").unwrap_or_else(|| "0.0.0".to_string());
    let build = number_constant(&version_source, "BUILD").unwrap_or(0);
    let parts: Vec<u32> =
        version_name.split('.').map(|part| part.parse().unwrap_or(0)).chain([0, 0, 0]).take(3).collect();
    let icon = manifest_folder.join("../../assets/icon.ico").canonicalize().unwrap();
    let tag = format!("{version_name}+{build}");
    let numeric = format!("{},{},{},{build}", parts[0], parts[1], parts[2]);
    let script = format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "CardTable"
      VALUE "FileVersion", "{tag}"
      VALUE "InternalName", "CardTable.exe"
      VALUE "OriginalFilename", "CardTable.exe"
      VALUE "ProductName", "CardTable"
      VALUE "ProductVersion", "{tag}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = icon.to_string_lossy().trim_start_matches(r"\\?\").replace('\\', "\\\\"),
    );
    let out_folder = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let script_path = out_folder.join("cardtable.rc");
    let resource_path = out_folder.join("cardtable.res");
    std::fs::write(&script_path, script).unwrap();
    let status = std::process::Command::new(&resource_compiler)
        .arg("/nologo")
        .arg("/fo")
        .arg(&resource_path)
        .arg(&script_path)
        .status();
    match status {
        Ok(status) if status.success() => {
            // Only the executables (not the library or tests) get the resource.
            println!("cargo:rustc-link-arg-bins={}", resource_path.display());
        }
        other => println!("cargo:warning=rc.exe failed ({other:?}); building without an icon or version resource"),
    }
}

/// `RC`, else the newest x64 `rc.exe` in the Windows 10/11 SDK.
fn find_resource_compiler() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("RC").map(PathBuf::from).filter(|path| path.is_file()) {
        return Some(path);
    }
    let kits = std::env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"))
        .join(r"Windows Kits\10\bin");
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(&kits)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join(r"x64\rc.exe"))
        .filter(|path| path.is_file())
        .collect();
    candidates.sort_by_key(|path| version_of(path));
    candidates.pop()
}

/// `...\bin\10.0.26100.0\x64\rc.exe` -> [10, 0, 26100, 0].
fn version_of(path: &Path) -> Vec<u32> {
    path.parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().split('.').map(|part| part.parse().unwrap_or(0)).collect())
        .unwrap_or_default()
}

/// `pub const NAME: &str = "value";`
fn string_constant(source: &str, name: &str) -> Option<String> {
    let line = source.lines().find(|line| line.contains(&format!("const {name}:")))?;
    let start = line.find('"')? + 1;
    let end = start + line[start..].find('"')?;
    Some(line[start..end].to_string())
}

/// `pub const NAME: u32 = 38;`
fn number_constant(source: &str, name: &str) -> Option<u32> {
    let line = source.lines().find(|line| line.contains(&format!("const {name}:")))?;
    line.split('=').nth(1)?.trim().trim_end_matches(';').trim().parse().ok()
}
