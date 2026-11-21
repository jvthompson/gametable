//! "Save to Zip" in the Game Definition Editor (a GameTable addition): packs a game's whole folder
//! into one `<folder name>.gamedef` file beside it in the game library. The file is an ordinary zip
//! with the game folder at its root (`lorcana/gamedef.json`, `lorcana/<set>/<image>`, ...), the
//! layout the server's admin game upload expects.
//!
//! Images are already compressed, so they are stored as they are; everything else is deflated.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

use crate::data::file_system::{base_name, join_path, lowercase_extension, parent_folder};

/// The extension of a packed game file.
pub const GAME_PACKAGE_EXTENSION: &str = "gamedef";

/// Extensions written without compression, since deflating them gains next to nothing.
const STORED_EXTENSIONS: [&str; 8] = [".png", ".jpg", ".jpeg", ".webp", ".gif", ".zip", ".mp3", ".ogg"];

/// Left behind by an interrupted image rewrite; never worth packing.
const TEMPORARY_FILE_SUFFIX: &str = ".gametable-tmp";

/// `<library>/lorcana` -> `<library>/lorcana.gamedef`.
pub fn game_package_path(game_folder_path: &str) -> String {
    let trimmed = game_folder_path.trim_end_matches(['/', '\\']);
    let folder_name = base_name(trimmed);
    join_path(&[&parent_folder(trimmed), &format!("{folder_name}.{GAME_PACKAGE_EXTENSION}")])
}

/// One file to pack: where it is, and its name inside the zip (always with `/` separators).
#[derive(Debug, Clone, PartialEq)]
pub struct PackageEntry {
    pub file_path: PathBuf,
    pub name_in_zip: String,
}

/// Every file under the game folder, sorted by name, each named `<folder name>/<relative path>`.
pub fn package_entries(game_folder_path: &str) -> Result<Vec<PackageEntry>, String> {
    let root = Path::new(game_folder_path.trim_end_matches(['/', '\\']));
    let folder_name = base_name(&root.to_string_lossy()).to_string();
    let mut entries = Vec::new();
    let mut folders_to_visit = vec![(root.to_path_buf(), folder_name)];
    while let Some((folder, name_in_zip)) = folders_to_visit.pop() {
        let items = std::fs::read_dir(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
        for item in items.filter_map(Result::ok) {
            let file_name = item.file_name().to_string_lossy().into_owned();
            let path = item.path();
            let child_name = format!("{name_in_zip}/{file_name}");
            if path.is_dir() {
                folders_to_visit.push((path, child_name));
            } else if !file_name.ends_with(TEMPORARY_FILE_SUFFIX) {
                entries.push(PackageEntry { file_path: path, name_in_zip: child_name });
            }
        }
    }
    entries.sort_by(|first, second| first.name_in_zip.cmp(&second.name_in_zip));
    Ok(entries)
}

/// What a finished package holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageSummary {
    pub file_count: usize,
    pub package_bytes: u64,
}

/// Why packing stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageError {
    Cancelled,
    Failed(String),
}

/// Writes `entries` to `package_path`, counting each finished file in `files_done`. The zip is
/// written to a temporary file first, so a failed or cancelled run never leaves a broken package
/// (or replaces a good one).
pub fn write_game_package(
    entries: &[PackageEntry],
    package_path: &str,
    files_done: &AtomicUsize,
    cancelled: &AtomicBool,
) -> Result<PackageSummary, PackageError> {
    let temporary_path = format!("{package_path}{TEMPORARY_FILE_SUFFIX}");
    let result = write_zip(entries, &temporary_path, files_done, cancelled).and_then(|()| {
        std::fs::rename(&temporary_path, package_path).map_err(|error| PackageError::Failed(error.to_string()))
    });
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(error);
    }
    let package_bytes = std::fs::metadata(package_path).map_or(0, |metadata| metadata.len());
    Ok(PackageSummary { file_count: entries.len(), package_bytes })
}

fn write_zip(
    entries: &[PackageEntry],
    zip_path: &str,
    files_done: &AtomicUsize,
    cancelled: &AtomicBool,
) -> Result<(), PackageError> {
    let failed = |error: &dyn std::fmt::Display| PackageError::Failed(error.to_string());
    let file = File::create(zip_path).map_err(|error| PackageError::Failed(format!("{zip_path}: {error}")))?;
    let mut writer = zip::ZipWriter::new(BufWriter::new(file));
    for entry in entries {
        if cancelled.load(Ordering::Relaxed) {
            return Err(PackageError::Cancelled);
        }
        let compression = if STORED_EXTENSIONS.contains(&lowercase_extension(&entry.name_in_zip).as_str()) {
            CompressionMethod::Stored
        } else {
            CompressionMethod::Deflated
        };
        let options = SimpleFileOptions::default().compression_method(compression).large_file(true);
        writer.start_file(entry.name_in_zip.as_str(), options).map_err(|error| failed(&error))?;
        let mut source = File::open(&entry.file_path)
            .map_err(|error| PackageError::Failed(format!("{}: {error}", entry.file_path.display())))?;
        std::io::copy(&mut source, &mut writer)
            .map_err(|error| PackageError::Failed(format!("{}: {error}", entry.file_path.display())))?;
        files_done.fetch_add(1, Ordering::Relaxed);
    }
    let mut buffered = writer.finish().map_err(|error| failed(&error))?;
    buffered.flush().map_err(|error| failed(&error))
}

/// Packing on a background thread, for the editor's progress dialog.
pub struct GamePackageJob {
    pub package_path: String,
    pub total_files: usize,
    files_done: Arc<AtomicUsize>,
    cancelled: Arc<AtomicBool>,
    finished: std::sync::mpsc::Receiver<Result<PackageSummary, PackageError>>,
}

impl GamePackageJob {
    /// Lists the folder's files and starts packing them into [`game_package_path`].
    pub fn start(game_folder_path: &str) -> Result<GamePackageJob, String> {
        let entries = package_entries(game_folder_path)?;
        let package_path = game_package_path(game_folder_path);
        let files_done = Arc::new(AtomicUsize::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (sender, finished) = std::sync::mpsc::channel();
        let job = GamePackageJob {
            package_path: package_path.clone(),
            total_files: entries.len(),
            files_done: files_done.clone(),
            cancelled: cancelled.clone(),
            finished,
        };
        std::thread::Builder::new()
            .name("save-to-zip".to_string())
            .spawn(move || {
                let _ = sender.send(write_game_package(&entries, &package_path, &files_done, &cancelled));
            })
            .map_err(|error| error.to_string())?;
        Ok(job)
    }

    pub fn files_done(&self) -> usize {
        self.files_done.load(Ordering::Relaxed)
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelling(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// The outcome once the thread is done.
    pub fn poll(&self) -> Option<Result<PackageSummary, PackageError>> {
        match self.finished.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some(Err(PackageError::Failed("Save to Zip stopped unexpectedly.".to_string())))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn package_path_sits_beside_the_game_folder() {
        assert_eq!(game_package_path("D:/library/lorcana"), join_path(&["D:/library", "lorcana.gamedef"]));
        assert_eq!(game_package_path("D:\\library\\lorcana\\"), join_path(&["D:\\library", "lorcana.gamedef"]));
    }

    #[test]
    fn package_holds_the_whole_folder_under_its_name() {
        let library = std::env::temp_dir().join(format!("gametable_package_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&library);
        let game = library.join("mygame");
        std::fs::create_dir_all(game.join("set1").join("nested")).unwrap();
        std::fs::write(game.join("gamedef.json"), br#"{"id":"mygame"}"#).unwrap();
        std::fs::write(game.join("set1").join("card.webp"), b"image bytes").unwrap();
        std::fs::write(game.join("set1").join("nested").join("notes.txt"), b"notes").unwrap();
        std::fs::write(game.join("set1").join("card.png.gametable-tmp"), b"half written").unwrap();
        let game_path = game.to_string_lossy().into_owned();

        let entries = package_entries(&game_path).unwrap();
        let names: Vec<&str> = entries.iter().map(|entry| entry.name_in_zip.as_str()).collect();
        assert_eq!(names, ["mygame/gamedef.json", "mygame/set1/card.webp", "mygame/set1/nested/notes.txt"]);

        let package_path = game_package_path(&game_path);
        let files_done = AtomicUsize::new(0);
        let summary = write_game_package(&entries, &package_path, &files_done, &AtomicBool::new(false)).unwrap();
        assert_eq!(summary.file_count, 3);
        assert_eq!(files_done.load(Ordering::Relaxed), 3);
        assert_eq!(crate::net::admin_files::validate_game_zip(&package_path), Ok("mygame".to_string()));
        let mut archive = zip::ZipArchive::new(File::open(&package_path).unwrap()).unwrap();
        let mut image = String::new();
        archive.by_name("mygame/set1/card.webp").unwrap().read_to_string(&mut image).unwrap();
        assert_eq!(image, "image bytes");
        assert!(!Path::new(&format!("{package_path}{TEMPORARY_FILE_SUFFIX}")).exists());

        // A cancelled run leaves the earlier package alone.
        let cancelled = write_game_package(&entries, &package_path, &AtomicUsize::new(0), &AtomicBool::new(true));
        assert_eq!(cancelled, Err(PackageError::Cancelled));
        assert_eq!(crate::net::admin_files::validate_game_zip(&package_path), Ok("mygame".to_string()));
        std::fs::remove_dir_all(&library).unwrap();
    }
}
