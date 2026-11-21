//! Unpacking an installed `.gamedef` package into a loose game folder so the Game Definition
//! Editor can edit it (a GameTable addition). The package's files go to `<library>/<id>/`; the
//! package itself is kept, and is still what the game plays from until "Save to Zip" rewrites it.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use gametable_core::game_archive::open_game_archive;

use crate::data::file_system::{join_path, remove_tree};

/// `<library>/<root folder of the package>`, where extracting a package puts its files.
pub fn extraction_folder(package_path: &str) -> Result<String, String> {
    let archive = open_game_archive(Path::new(package_path))?;
    let library =
        Path::new(package_path).parent().map(|parent| parent.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(join_path(&[&library, archive.root_folder()]))
}

/// Writes every file of the package under `destination_folder`, replacing whatever was there. The
/// files are written to a temporary folder first, so a failure leaves the old folder alone.
pub fn extract_game_package(
    package_path: &str,
    destination_folder: &str,
    files_done: &AtomicUsize,
) -> Result<(), String> {
    let archive = open_game_archive(Path::new(package_path))?;
    let temporary_folder = format!("{destination_folder}.gametable-extracting");
    let _ = remove_tree(Path::new(&temporary_folder));
    let written = (|| -> Result<(), String> {
        for file in archive.files() {
            let path = Path::new(&temporary_folder).join(&file.relative_path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
            }
            let bytes = archive.read(&file.relative_path).map_err(|error| error.to_string())?;
            std::fs::write(&path, bytes).map_err(|error| format!("{}: {error}", path.display()))?;
            files_done.fetch_add(1, Ordering::Relaxed);
        }
        if Path::new(destination_folder).exists() {
            remove_tree(Path::new(destination_folder)).map_err(|error| format!("{destination_folder}: {error}"))?;
        }
        std::fs::rename(&temporary_folder, destination_folder).map_err(|error| format!("{destination_folder}: {error}"))
    })();
    if written.is_err() {
        let _ = remove_tree(Path::new(&temporary_folder));
    }
    written
}

/// Extraction on a background thread, for the editor's progress dialog.
pub struct GameExtractJob {
    pub destination_folder: String,
    pub total_files: usize,
    files_done: Arc<AtomicUsize>,
    finished: Receiver<Result<(), String>>,
}

impl GameExtractJob {
    pub fn start(package_path: &str) -> Result<GameExtractJob, String> {
        let destination_folder = extraction_folder(package_path)?;
        let total_files = open_game_archive(Path::new(package_path))?.files().len();
        let files_done = Arc::new(AtomicUsize::new(0));
        let (sender, finished) = mpsc::channel();
        let (package, destination, done) = (package_path.to_string(), destination_folder.clone(), files_done.clone());
        std::thread::Builder::new()
            .name("extract-game".to_string())
            .spawn(move || {
                let _ = sender.send(extract_game_package(&package, &destination, &done));
            })
            .map_err(|error| error.to_string())?;
        Ok(GameExtractJob { destination_folder, total_files, files_done, finished })
    }

    pub fn progress(&self) -> f32 {
        if self.total_files == 0 {
            0.0
        } else {
            self.files_done.load(Ordering::Relaxed) as f32 / self.total_files as f32
        }
    }

    pub fn poll(&self) -> Option<Result<(), String>> {
        match self.finished.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("Extracting stopped unexpectedly.".to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn packages_extract_into_a_folder_named_after_their_root() {
        let library = std::env::temp_dir().join(format!("gametable_extract_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&library);
        std::fs::create_dir_all(library.join("mygame")).unwrap();
        std::fs::write(library.join("mygame").join("stale.txt"), b"old").unwrap();
        let package = library.join("mygame.gamedef");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&package).unwrap());
        for (name, text) in [("mygame/gamedef.json", "{}"), ("mygame/set1/card.webp", "card")] {
            writer.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
        let package_text = package.to_string_lossy().into_owned();

        let destination = extraction_folder(&package_text).unwrap();
        assert_eq!(destination, join_path(&[&library.to_string_lossy(), "mygame"]));
        let done = AtomicUsize::new(0);
        extract_game_package(&package_text, &destination, &done).unwrap();
        assert_eq!(done.load(Ordering::Relaxed), 2);
        assert_eq!(std::fs::read(library.join("mygame").join("set1").join("card.webp")).unwrap(), b"card");
        assert!(!library.join("mygame").join("stale.txt").exists(), "the old folder is replaced");
        assert!(package.exists(), "the package is kept");
        gametable_core::game_archive::forget_game_archive(&package);
        std::fs::remove_dir_all(&library).unwrap();
    }
}
