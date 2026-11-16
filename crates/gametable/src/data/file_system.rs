//! File and folder helpers with the original's semantics. Port of `src/data/fs.lua`.
//!
//! The original needed Win32 FFI for Unicode and long paths; Rust's `std::fs` already handles
//! both (it adds the `\\?\` prefix itself), so this module keeps only the behaviour the rest of
//! the app relies on: case-insensitive sorted listings, Dart-style extension helpers, recursive
//! copy/remove and relative file walks.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// One item in a folder listing.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderEntry {
    pub name: String,
    pub is_folder: bool,
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub modified_at: f64,
}

/// A folder's entries sorted by name ignoring case, or `None` when it can't be read.
pub fn list_folder(folder: &Path) -> Option<Vec<FolderEntry>> {
    let items = std::fs::read_dir(folder).ok()?;
    let mut entries: Vec<FolderEntry> = items
        .filter_map(Result::ok)
        .map(|item| {
            let metadata = item.metadata().ok();
            FolderEntry {
                name: item.file_name().to_string_lossy().into_owned(),
                is_folder: metadata.as_ref().is_some_and(|metadata| metadata.is_dir()),
                size: metadata.as_ref().map_or(0, |metadata| metadata.len()),
                modified_at: metadata
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map_or(0.0, |elapsed| elapsed.as_secs_f64()),
            }
        })
        .collect();
    entries.sort_by_key(|entry| entry.name.to_lowercase());
    Some(entries)
}

/// The extension including the dot, lowercase; `""` when there is none (a leading dot alone,
/// as in `.hidden`, is not an extension).
pub fn lowercase_extension(path: &str) -> String {
    let name = base_name(path);
    match name.rfind('.') {
        Some(dot) if dot > 0 => name[dot..].to_lowercase(),
        _ => String::new(),
    }
}

/// Strips from the last dot, like Dart's `stripExtension` (a leading dot is kept).
pub fn strip_extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(dot) if dot > 0 => &name[..dot],
        _ => name,
    }
}

/// The last component of a path written with either slash (trailing slashes ignored).
pub fn base_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    match trimmed.rfind(['/', '\\']) {
        Some(separator) => &trimmed[separator + 1..],
        None if trimmed.is_empty() => path,
        None => trimmed,
    }
}

/// Removes a file, or a folder and everything in it.
pub fn remove_tree(path: &Path) -> std::io::Result<()> {
    if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) }
}

/// Copies a file, or a folder recursively, overwriting existing files.
pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in list_folder(from).unwrap_or_default() {
            copy_tree(&from.join(&entry.name), &to.join(&entry.name))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// A file found by [`list_files_recursive`].
#[derive(Debug, Clone, PartialEq)]
pub struct WalkedFile {
    pub path: PathBuf,
    /// Relative to the walked folder, `/`-separated on every OS.
    pub relative_path: String,
}

/// Every file under a folder (not the folders themselves), in sorted listing order.
pub fn list_files_recursive(folder: &Path) -> Vec<WalkedFile> {
    fn walk(folder: &Path, relative: &str, files: &mut Vec<WalkedFile>) {
        for entry in list_folder(folder).unwrap_or_default() {
            let path = folder.join(&entry.name);
            let relative_path =
                if relative.is_empty() { entry.name.clone() } else { format!("{relative}/{}", entry.name) };
            if entry.is_folder {
                walk(&path, &relative_path, files);
            } else {
                files.push(WalkedFile { path, relative_path });
            }
        }
    }
    let mut files = Vec::new();
    walk(folder, "", &mut files);
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_helpers_match_the_original() {
        assert_eq!(lowercase_extension("C:\\a\\Card.PNG"), ".png");
        assert_eq!(lowercase_extension("deck.backup.json"), ".json");
        assert_eq!(lowercase_extension(".hidden"), "");
        assert_eq!(lowercase_extension("noext"), "");
        assert_eq!(strip_extension("deck.backup.json"), "deck.backup");
        assert_eq!(strip_extension(".hidden"), ".hidden");
        assert_eq!(base_name("a/b\\c.txt"), "c.txt");
        assert_eq!(base_name("a/b/"), "b");
        assert_eq!(base_name("plain"), "plain");
    }

    #[test]
    fn listing_walking_copying_and_removing() {
        let root = std::env::temp_dir().join(format!("gametable-fs-{}", gametable_core::uuid::new_v4_uuid()));
        std::fs::create_dir_all(root.join("src").join("Sub")).unwrap();
        std::fs::write(root.join("src").join("b.txt"), "b").unwrap();
        std::fs::write(root.join("src").join("A.txt"), "a").unwrap();
        std::fs::write(root.join("src").join("Sub").join("c.txt"), "c").unwrap();

        let names: Vec<String> = list_folder(&root.join("src")).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["A.txt", "b.txt", "Sub"]);
        assert!(list_folder(&root.join("missing")).is_none());

        let walked: Vec<String> =
            list_files_recursive(&root.join("src")).into_iter().map(|f| f.relative_path).collect();
        assert_eq!(walked, ["A.txt", "b.txt", "Sub/c.txt"]);

        copy_tree(&root.join("src"), &root.join("copy")).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("copy").join("Sub").join("c.txt")).unwrap(), "c");
        remove_tree(&root.join("copy")).unwrap();
        assert!(!root.join("copy").exists());
        remove_tree(&root).unwrap();
    }
}
