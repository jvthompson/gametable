//! Installing a `.gamedef` package into the game library (a GameTable addition): dropping one on
//! Home checks it, asks, then copies it to `<library>/<id>.gamedef`, where the library scan picks
//! it up and plays it in place.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use gametable_core::game_archive::{
    GAME_PACKAGE_EXTENSION, forget_game_archive, is_game_package_path, open_game_archive,
};
use gametable_core::json::{self, JsonValue};

use crate::data::file_system::{is_same_path, join_path};
use crate::net::admin_files::validate_game_zip;

/// What installing a dropped package would do.
#[derive(Debug, Clone, PartialEq)]
pub struct InstallPlan {
    pub source_path: String,
    pub game_id: String,
    pub game_name: String,
    /// `<library>/<id>.gamedef`.
    pub destination_path: String,
    /// The dropped file already is the installed package.
    pub already_installed: bool,
    /// An installed package with this id would be replaced.
    pub replaces_package: bool,
    /// A loose folder with this id exists; the package will be used instead of it.
    pub shadows_folder: bool,
}

/// Checks a dropped file and works out where it goes. The error is a sentence for the user.
pub fn plan_install(source_path: &str, library_path: &str) -> Result<InstallPlan, String> {
    if !is_game_package_path(source_path) {
        return Err("Only .gamedef game files can be installed.".to_string());
    }
    let game_id = validate_game_zip(source_path)?;
    let archive = open_game_archive(Path::new(source_path))?;
    let definition = archive.game_definition_bytes()?;
    let game_name = json::decode_bytes(&definition)
        .ok()
        .and_then(|raw| raw.get("name").and_then(JsonValue::as_str).map(str::to_string))
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| game_id.clone());
    let destination_path = join_path(&[library_path, &format!("{game_id}.{GAME_PACKAGE_EXTENSION}")]);
    let already_installed = is_same_path(source_path, &destination_path);
    Ok(InstallPlan {
        source_path: source_path.to_string(),
        replaces_package: !already_installed && Path::new(&destination_path).is_file(),
        shadows_folder: Path::new(&join_path(&[library_path, &game_id])).is_dir(),
        game_id,
        game_name,
        destination_path,
        already_installed,
    })
}

impl InstallPlan {
    /// The confirmation's text.
    pub fn confirmation_message(&self) -> String {
        let mut message = format!("Install \"{}\" into your game library?", self.game_name);
        if self.replaces_package {
            message.push_str(&format!(" This replaces the installed {}.{GAME_PACKAGE_EXTENSION}.", self.game_id));
        }
        if self.shadows_folder {
            message.push_str(&format!(
                " Your library also has a \"{}\" folder; the installed package will be played instead of it.",
                self.game_id
            ));
        }
        message
    }
}

/// Copies `source` to `destination` through a temporary file, counting bytes in `copied`.
fn copy_package(source: &str, destination: &str, copied: &AtomicU64) -> Result<(), String> {
    if let Some(parent) = Path::new(destination).parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary_path = format!("{destination}.gametable-tmp");
    let result = (|| -> std::io::Result<()> {
        let mut reader = std::fs::File::open(source)?;
        let mut writer = std::io::BufWriter::new(std::fs::File::create(&temporary_path)?);
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            writer.write_all(&buffer[..count])?;
            copied.fetch_add(count as u64, Ordering::Relaxed);
        }
        writer.flush()?;
        drop(writer);
        // The open copy of an old package holds its file; let go of it before replacing it.
        forget_game_archive(Path::new(destination));
        std::fs::rename(&temporary_path, destination)
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(error.to_string());
    }
    forget_game_archive(Path::new(destination));
    Ok(())
}

/// The copy on a background thread, for Home's progress dialog.
pub struct GameInstallJob {
    pub plan: InstallPlan,
    pub total_bytes: u64,
    copied_bytes: Arc<AtomicU64>,
    finished: Receiver<Result<(), String>>,
}

impl GameInstallJob {
    pub fn start(plan: InstallPlan) -> GameInstallJob {
        let total_bytes = std::fs::metadata(&plan.source_path).map_or(0, |metadata| metadata.len());
        let copied_bytes = Arc::new(AtomicU64::new(0));
        let (sender, finished) = mpsc::channel();
        let (source, destination, copied) =
            (plan.source_path.clone(), plan.destination_path.clone(), copied_bytes.clone());
        let spawned = std::thread::Builder::new().name("install-game".to_string()).spawn(move || {
            let _ = sender.send(copy_package(&source, &destination, &copied));
        });
        let (sender, failed_start) = mpsc::channel();
        let finished = match spawned {
            Ok(_) => finished,
            Err(error) => {
                let _ = sender.send(Err(error.to_string()));
                failed_start
            }
        };
        GameInstallJob { plan, total_bytes, copied_bytes, finished }
    }

    /// How far the copy has got, 0 to 1.
    pub fn progress(&self) -> f32 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        (self.copied_bytes.load(Ordering::Relaxed) as f64 / self.total_bytes as f64).min(1.0) as f32
    }

    /// The outcome once the copy is done.
    pub fn poll(&self) -> Option<Result<(), String>> {
        match self.finished.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("The install stopped unexpectedly.".to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_package(path: &Path, entries: &[(&str, &str)]) {
        let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, text) in entries {
            writer.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }

    fn wait_for(job: &GameInstallJob) -> Result<(), String> {
        loop {
            if let Some(result) = job.poll() {
                return result;
            }
            std::thread::yield_now();
        }
    }

    #[test]
    fn dropped_packages_install_and_replace() {
        let folder = std::env::temp_dir().join(format!("gametable_install_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let downloads = folder.join("downloads");
        let library = folder.join("library");
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::create_dir_all(library.join("mygame")).unwrap();
        let dropped = downloads.join("my game (1).gamedef");
        write_package(&dropped, &[("mygame/gamedef.json", r#"{"id":"mygame","name":"My Game"}"#)]);
        let library_text = library.to_string_lossy().into_owned();

        let plan = plan_install(&dropped.to_string_lossy(), &library_text).unwrap();
        assert_eq!((plan.game_id.as_str(), plan.game_name.as_str()), ("mygame", "My Game"));
        assert_eq!(plan.destination_path, join_path(&[&library_text, "mygame.gamedef"]));
        assert!(!plan.replaces_package && plan.shadows_folder && !plan.already_installed);
        assert!(plan.confirmation_message().contains("played instead"));
        wait_for(&GameInstallJob::start(plan.clone())).unwrap();
        assert_eq!(std::fs::read(&plan.destination_path).unwrap(), std::fs::read(&dropped).unwrap());

        // Installed copies are recognised, and a second drop replaces the first.
        let again = plan_install(&dropped.to_string_lossy(), &library_text).unwrap();
        assert!(again.replaces_package);
        assert!(plan_install(&plan.destination_path, &library_text).unwrap().already_installed);
        write_package(&dropped, &[("mygame/gamedef.json", r#"{"id":"mygame","name":"My Game v2"}"#)]);
        open_game_archive(Path::new(&plan.destination_path)).unwrap();
        wait_for(&GameInstallJob::start(again)).unwrap();
        let installed = open_game_archive(Path::new(&plan.destination_path)).unwrap();
        assert!(String::from_utf8(installed.game_definition_bytes().unwrap()).unwrap().contains("v2"));
        drop(installed);
        forget_game_archive(Path::new(&plan.destination_path));

        let not_a_package = downloads.join("notes.zip");
        std::fs::write(&not_a_package, b"x").unwrap();
        assert_eq!(
            plan_install(&not_a_package.to_string_lossy(), &library_text),
            Err("Only .gamedef game files can be installed.".to_string())
        );
        let broken = downloads.join("broken.gamedef");
        write_package(&broken, &[("a/gamedef.json", "{}"), ("b/x.png", "x")]);
        assert!(plan_install(&broken.to_string_lossy(), &library_text).is_err());
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
