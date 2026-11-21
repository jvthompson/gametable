//! Reading a game's files whether the game is a loose folder or a `.gamedef` package (a GameTable
//! addition). A packaged game's folder path is the package file itself, so its files have paths
//! such as `D:/library/lorcana.gamedef/1st/ariel.webp`; these functions read those out of the
//! package (see [`gametable_core::game_archive`]) and every other path from the disk as before.
//!
//! Packages can't be written to, so what a game saves at runtime (save games, logs) goes to
//! [`writable_game_folder`] instead.

use std::path::Path;

use gametable_core::game_archive::{is_game_package_path, open_game_archive, split_package_path};

use crate::data::file_system::{
    FolderEntry, join_path, list_folder as list_disk_folder, parent_folder, strip_extension,
};

/// The folder in the game library that holds what packaged games write.
pub const PACKAGED_GAME_DATA_FOLDER: &str = ".gamedata";

/// A file's bytes, from inside a package or from the disk.
pub fn read(path: &str) -> std::io::Result<Vec<u8>> {
    match split_package_path(path) {
        Some((package_path, relative_path)) => {
            let archive = open_game_archive(Path::new(&package_path)).map_err(std::io::Error::other)?;
            archive.read(&relative_path)
        }
        None => std::fs::read(path),
    }
}

/// Whether `path` is a file (inside a package, or on the disk).
pub fn is_file(path: &str) -> bool {
    match split_package_path(path) {
        Some((_, relative_path)) if relative_path.is_empty() => false,
        Some((package_path, relative_path)) => {
            open_game_archive(Path::new(&package_path)).is_ok_and(|archive| archive.contains_file(&relative_path))
        }
        None => Path::new(path).is_file(),
    }
}

/// Whether `path` is a folder; a package counts as its own game folder.
pub fn is_dir(path: &str) -> bool {
    match split_package_path(path) {
        Some((package_path, relative_path)) => {
            open_game_archive(Path::new(&package_path)).is_ok_and(|archive| archive.contains_folder(&relative_path))
        }
        None => Path::new(path).is_dir(),
    }
}

/// A folder's entries sorted by name ignoring case, or `None` when there is no such folder.
/// Entries inside a package have no modified time.
pub fn list_folder(path: &str) -> Option<Vec<FolderEntry>> {
    let Some((package_path, relative_folder)) = split_package_path(path) else {
        return list_disk_folder(Path::new(path));
    };
    let archive = open_game_archive(Path::new(&package_path)).ok()?;
    let mut entries: Vec<FolderEntry> = archive
        .list(&relative_folder)?
        .into_iter()
        .map(|(name, is_folder)| {
            let size =
                if is_folder { 0 } else { archive.file_size(&join_relative(&relative_folder, &name)).unwrap_or(0) };
            FolderEntry { name, is_folder, size, modified_at: 0.0 }
        })
        .collect();
    entries.sort_by_key(|entry| entry.name.to_lowercase());
    Some(entries)
}

fn join_relative(folder: &str, name: &str) -> String {
    if folder.is_empty() { name.to_string() } else { format!("{folder}/{name}") }
}

/// Whether a game's folder path is a `.gamedef` package rather than a loose folder.
pub fn is_packaged_game_folder(game_folder_path: &str) -> bool {
    is_game_package_path(game_folder_path)
}

/// Where a game writes its save games and logs: the game folder itself, or for a package
/// `<library>/.gamedata/<package name>`, which survives the package being reinstalled.
pub fn writable_game_folder(game_folder_path: &str) -> String {
    if !is_packaged_game_folder(game_folder_path) {
        return game_folder_path.to_string();
    }
    let trimmed = game_folder_path.trim_end_matches(['/', '\\']);
    let package_name = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
    join_path(&[&parent_folder(trimmed), PACKAGED_GAME_DATA_FOLDER, strip_extension(package_name)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_package(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, bytes) in entries {
            writer.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn package_paths_read_like_a_folder() {
        let library = std::env::temp_dir().join(format!("gametable_game_files_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&library);
        std::fs::create_dir_all(library.join("loose")).unwrap();
        std::fs::write(library.join("loose").join("card.png"), b"loose card").unwrap();
        let package = library.join("mygame.gamedef");
        write_package(&package, &[("mygame/gamedef.json", b"{}"), ("mygame/set1/card.webp", b"packed card")]);
        let package_path = package.to_string_lossy().into_owned();
        let loose_path = library.join("loose").to_string_lossy().into_owned();

        assert_eq!(read(&join_path(&[&package_path, "set1", "card.webp"])).unwrap(), b"packed card");
        assert_eq!(read(&join_path(&[&loose_path, "card.png"])).unwrap(), b"loose card");
        assert!(is_file(&join_path(&[&package_path, "set1", "card.webp"])));
        assert!(!is_file(&join_path(&[&package_path, "set1", "other.webp"])));
        assert!(!is_file(&package_path), "the package is the game folder, not a file inside it");
        assert!(is_dir(&package_path));
        assert!(is_dir(&join_path(&[&package_path, "set1"])));
        assert!(!is_dir(&join_path(&[&package_path, "_decks"])));
        let names: Vec<String> = list_folder(&package_path)
            .unwrap()
            .into_iter()
            .map(|entry| format!("{}{}", entry.name, if entry.is_folder { "/" } else { "" }))
            .collect();
        assert_eq!(names, ["gamedef.json", "set1/"]);
        assert!(list_folder(&join_path(&[&package_path, "missing"])).is_none());
        std::fs::remove_dir_all(&library).unwrap();
    }

    #[test]
    fn packaged_games_write_beside_the_package() {
        assert_eq!(writable_game_folder("D:/library/lorcana"), "D:/library/lorcana");
        assert_eq!(
            writable_game_folder("D:/library/lorcana.gamedef"),
            join_path(&["D:/library", ".gamedata", "lorcana"])
        );
    }
}
