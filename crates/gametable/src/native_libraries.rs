//! Where the optional native libraries (pdfium, libwebp/libsharpyuv and, later, the Discord SDK)
//! are found on each platform. The app works without any of them; each feature reports itself
//! unavailable instead.
//!
//! Searched, in order:
//! - the executable's folder (Windows and Linux release layout);
//! - inside a macOS `.app` bundle, `Contents/Frameworks` and `Contents/Resources`;
//! - in development builds, the workspace's `runtime/<platform>/` folder;
//! - finally the system's own library search (`libwebp.so.7` from a Linux package, for example).

use std::path::{Path, PathBuf};

/// The `runtime/` subfolder holding this platform's prebuilt libraries.
pub fn runtime_platform_folder() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// The folders searched for bundled libraries, best first.
pub fn search_folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    if let Some(executable_folder) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        // A macOS bundle: <App>.app/Contents/MacOS/<executable>.
        if cfg!(target_os = "macos")
            && executable_folder.ends_with("Contents/MacOS")
            && let Some(contents) = executable_folder.parent()
        {
            folders.push(contents.join("Frameworks"));
            folders.push(contents.join("Resources"));
        }
        folders.insert(0, executable_folder);
    }
    if cfg!(debug_assertions) {
        folders.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime").join(runtime_platform_folder()));
    }
    folders
}

/// Whether `file_name` is a build of the library `stem` for any platform, ignoring case:
/// `pdfium.dll`, `libpdfium.so`, `libwebp-7.dll`, `libwebp.so.7`, `libwebp.7.dylib`. Version
/// numbers may follow the stem (after `-` or `.`) or, for Linux, the `.so`.
pub fn is_library_named(file_name: &str, stem: &str) -> bool {
    let lowercase = file_name.to_lowercase();
    let stem = stem.to_lowercase();
    let without_prefix = lowercase.strip_prefix("lib").filter(|_| !stem.starts_with("lib")).unwrap_or(&lowercase);
    let Some(mut rest) = without_prefix.strip_prefix(stem.as_str()) else { return false };
    // An optional version before the extension: -7, .7, -0.2.
    if let Some(after_separator) = rest.strip_prefix('-').or_else(|| rest.strip_prefix('.')) {
        let version_end = after_separator
            .find(|character: char| !(character.is_ascii_digit() || character == '.'))
            .unwrap_or(after_separator.len());
        let version = after_separator[..version_end].trim_end_matches('.');
        if !version.is_empty() && version.starts_with(|character: char| character.is_ascii_digit()) {
            rest = &after_separator[version.len()..];
        }
    }
    match rest {
        ".dll" | ".dylib" | ".so" => true,
        // libwebp.so.7, libwebp.so.7.1.8
        _ => rest.strip_prefix(".so.").is_some_and(|version| {
            !version.is_empty() && version.chars().all(|character| character.is_ascii_digit() || character == '.')
        }),
    }
}

/// Every file in the search folders that is a build of `stem`, then the platform's plain library
/// file name for the system search (`pdfium.dll`, `libpdfium.so`, `libpdfium.dylib`).
pub fn candidates(stem: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for folder in search_folders() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        let mut names: Vec<(String, PathBuf)> = entries
            .filter_map(Result::ok)
            .map(|entry| (entry.file_name().to_string_lossy().into_owned(), entry.path()))
            .filter(|(name, _)| is_library_named(name, stem))
            .collect();
        names.sort();
        found.extend(names.into_iter().map(|(_, path)| path));
    }
    found.push(PathBuf::from(libloading::library_filename(stem)));
    found
}

/// How to name the library in "needs X next to the app" messages on this platform.
pub fn display_file_name(stem: &str) -> String {
    libloading::library_filename(stem).to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_names_for_every_platform() {
        for name in ["pdfium.dll", "PDFium.DLL", "libpdfium.so", "libpdfium.dylib"] {
            assert!(is_library_named(name, "pdfium"), "{name}");
        }
        for name in ["libwebp-7.dll", "libwebp.so", "libwebp.so.7", "libwebp.so.7.1.8", "libwebp.7.dylib", "webp.dll"] {
            assert!(is_library_named(name, "webp"), "{name}");
        }
        assert!(is_library_named("libsharpyuv-0.dll", "sharpyuv"));
        assert!(is_library_named("libsharpyuv.so.0", "sharpyuv"));
        for name in ["libwebpdemux-2.dll", "libwebp.a", "libwebp.so.x", "pdfium.lib", "libwebp-7.dll.bak"] {
            assert!(!is_library_named(name, "webp") && !is_library_named(name, "pdfium"), "{name}");
        }
        assert!(!is_library_named("libsharpyuv-0.dll", "webp"));
    }
}
