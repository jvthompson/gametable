//! The one-time carry-over of the old app's save folder (a GameTable addition around the original
//! port of `paths.lua`'s copy). On the first launch after installing GameTable, the save folder of
//! the old CardTable app (or the older CardFlux one) is copied into GameTable's own. That folder can
//! hold gigabytes of games downloaded from servers, so the copy runs on a background thread while
//! the app shows a "Migrating Old Data..." progress dialog over Home, instead of holding up the
//! window.
//!
//! The copy goes into a staging folder beside the save folder (`gametable-migrating`) and only
//! moves into the save folder once every file is there. The app is usable during the copy and may
//! write its own `settings.json`, so a migration is still due while the staging folder exists: if
//! the app was closed part way, the next launch starts the copy again.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};

use super::file_system::list_folder;
use super::paths::AppPaths;
use super::settings::{SETTINGS_FILE_NAME, Settings, keys};

/// The staging folder's name is the save folder's with this added.
const STAGING_SUFFIX: &str = "-migrating";

/// What to copy where.
#[derive(Debug, Clone, PartialEq)]
pub struct LegacyMigrationPlan {
    /// The old app's save folder.
    pub old_folder: PathBuf,
    /// GameTable's save folder.
    pub new_folder: PathBuf,
    /// Where the copy is built before it moves into `new_folder`.
    pub staging_folder: PathBuf,
}

/// The migration due on this launch, if any: the first old identity that has a save folder, when
/// GameTable's own save folder has no settings yet or an earlier migration didn't finish.
pub fn plan_legacy_migration(paths: &AppPaths, old_identities: &[&str]) -> Option<LegacyMigrationPlan> {
    let new_folder = paths.save_folder.clone();
    let parent = new_folder.parent()?;
    let folder_name = new_folder.file_name()?.to_string_lossy().into_owned();
    let staging_folder = parent.join(format!("{folder_name}{STAGING_SUFFIX}"));
    let unfinished = staging_folder.exists();
    if new_folder.join(SETTINGS_FILE_NAME).exists() && !unfinished {
        return None;
    }
    let old_folder = old_identities.iter().map(|identity| parent.join(identity)).find(|folder| folder.is_dir())?;
    Some(LegacyMigrationPlan { old_folder, new_folder, staging_folder })
}

/// How far the copy has got, shared with the progress dialog.
#[derive(Debug, Default)]
pub struct MigrationProgress {
    pub bytes_copied: AtomicU64,
    /// 0 until the old folder has been measured.
    pub bytes_total: AtomicU64,
}

impl MigrationProgress {
    /// 0..1, or `None` while the old folder is still being measured.
    pub fn fraction(&self) -> Option<f32> {
        let total = self.bytes_total.load(Ordering::Relaxed);
        if total == 0 {
            return None;
        }
        Some((self.bytes_copied.load(Ordering::Relaxed) as f64 / total as f64).min(1.0) as f32)
    }
}

/// The folders and files under `folder` (relative paths), folders first, then their contents.
fn walk(folder: &Path, relative: &Path, folders: &mut Vec<PathBuf>, files: &mut Vec<(PathBuf, u64)>) {
    for entry in list_folder(folder).unwrap_or_default() {
        let path = folder.join(&entry.name);
        let relative_path = relative.join(&entry.name);
        if path.is_dir() {
            folders.push(relative_path.clone());
            walk(&path, &relative_path, folders, files);
        } else {
            let size = std::fs::metadata(&path).map_or(0, |metadata| metadata.len());
            files.push((relative_path, size));
        }
    }
}

/// Copies the old save folder into the staging folder (counting bytes in `progress`), moves it into
/// GameTable's save folder, and points the avatar setting at its new place. Runs on the migration
/// thread; also called directly by tests.
pub fn run_legacy_migration(plan: &LegacyMigrationPlan, progress: &MigrationProgress) -> Result<(), String> {
    let describe = |path: &Path, error: std::io::Error| format!("{}: {error}", path.display());
    if plan.staging_folder.exists() {
        std::fs::remove_dir_all(&plan.staging_folder).map_err(|error| describe(&plan.staging_folder, error))?;
    }
    let (mut folders, mut files) = (Vec::new(), Vec::new());
    walk(&plan.old_folder, Path::new(""), &mut folders, &mut files);
    progress.bytes_total.store(files.iter().map(|(_, size)| size).sum::<u64>().max(1), Ordering::Relaxed);

    std::fs::create_dir_all(&plan.staging_folder).map_err(|error| describe(&plan.staging_folder, error))?;
    for folder in &folders {
        let target = plan.staging_folder.join(folder);
        std::fs::create_dir_all(&target).map_err(|error| describe(&target, error))?;
    }
    for (file, size) in &files {
        let (source, target) = (plan.old_folder.join(file), plan.staging_folder.join(file));
        std::fs::copy(&source, &target).map_err(|error| describe(&source, error))?;
        progress.bytes_copied.fetch_add(*size, Ordering::Relaxed);
    }

    // Into place: each top-level entry replaces whatever GameTable wrote meanwhile.
    std::fs::create_dir_all(&plan.new_folder).map_err(|error| describe(&plan.new_folder, error))?;
    for entry in list_folder(&plan.staging_folder).unwrap_or_default() {
        let (staged, target) = (plan.staging_folder.join(&entry.name), plan.new_folder.join(&entry.name));
        if target.is_dir() {
            std::fs::remove_dir_all(&target).map_err(|error| describe(&target, error))?;
        } else if target.exists() {
            std::fs::remove_file(&target).map_err(|error| describe(&target, error))?;
        }
        std::fs::rename(&staged, &target).map_err(|error| describe(&staged, error))?;
    }
    std::fs::remove_dir_all(&plan.staging_folder).map_err(|error| describe(&plan.staging_folder, error))?;

    // The avatar was saved as a full path into the old folder.
    let mut settings = Settings::load_from_folder(&plan.new_folder);
    let old_prefix = plan.old_folder.to_string_lossy().to_string();
    if let Some(avatar) = settings.text(keys::PLAYER_AVATAR_PATH)
        && let Some(rest) = avatar.strip_prefix(&old_prefix)
    {
        let moved = format!("{}{rest}", plan.new_folder.to_string_lossy());
        settings.set(keys::PLAYER_AVATAR_PATH, moved);
    }
    Ok(())
}

/// A migration running on its own thread.
pub struct LegacyMigrationJob {
    pub progress: Arc<MigrationProgress>,
    finished: Receiver<Result<(), String>>,
}

impl LegacyMigrationJob {
    pub fn start(plan: LegacyMigrationPlan) -> LegacyMigrationJob {
        let progress = Arc::new(MigrationProgress::default());
        let (sender, finished) = mpsc::channel();
        let thread_progress = progress.clone();
        let spawned = std::thread::Builder::new().name("legacy-migration".to_string()).spawn(move || {
            let _ = sender.send(run_legacy_migration(&plan, &thread_progress));
        });
        if let Err(error) = spawned {
            let (sender, receiver) = mpsc::channel();
            let _ = sender.send(Err(format!("could not start the copy: {error}")));
            return LegacyMigrationJob { progress, finished: receiver };
        }
        LegacyMigrationJob { progress, finished }
    }

    /// The result once the copy has finished.
    pub fn poll(&self) -> Option<Result<(), String>> {
        self.finished.try_recv().ok()
    }
}

/// "1.2 GB" / "340 MB" for the progress dialog.
pub fn size_text(bytes: u64) -> String {
    const MEGABYTE: f64 = 1024.0 * 1024.0;
    let megabytes = bytes as f64 / MEGABYTE;
    if megabytes >= 1024.0 { format!("{:.1} GB", megabytes / 1024.0) } else { format!("{megabytes:.0} MB") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::paths::{DEFAULT_IDENTITY, LEGACY_IDENTITIES};

    fn test_paths() -> (AppPaths, PathBuf) {
        let root = std::env::temp_dir().join(format!("gametable-migration-{}", gametable_core::uuid::new_v4_uuid()));
        let paths = AppPaths { app_folder: root.join("app"), save_folder: root.join("saves").join(DEFAULT_IDENTITY) };
        (paths, root)
    }

    #[test]
    fn the_old_folder_is_copied_once_with_progress() {
        let (paths, root) = test_paths();
        let legacy = root.join("saves").join("cardflux");
        std::fs::create_dir_all(legacy.join("avatar")).unwrap();
        std::fs::create_dir_all(legacy.join("servers").join("empty")).unwrap();
        std::fs::write(legacy.join("settings.json"), "{\"a\": 1}").unwrap();
        std::fs::write(legacy.join("avatar").join("avatar.png"), "png").unwrap();

        let plan = plan_legacy_migration(&paths, &LEGACY_IDENTITIES).expect("a migration is due");
        assert_eq!(plan.old_folder, legacy);
        let progress = MigrationProgress::default();
        assert_eq!(progress.fraction(), None);
        run_legacy_migration(&plan, &progress).unwrap();
        assert_eq!(progress.fraction(), Some(1.0));
        assert_eq!(Settings::load_from_folder(&paths.save_folder).number("a"), Some(1.0));
        assert!(paths.save_folder.join("avatar").join("avatar.png").exists());
        assert!(paths.save_folder.join("servers").join("empty").is_dir(), "empty folders come too");
        assert!(!plan.staging_folder.exists(), "the staging folder is cleaned up");
        assert!(legacy.join("settings.json").exists(), "the old folder is left alone");
        // Once the new folder has settings, nothing is due again.
        assert_eq!(plan_legacy_migration(&paths, &LEGACY_IDENTITIES), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unfinished_copy_is_redone_and_replaces_what_the_app_wrote_meanwhile() {
        let (paths, root) = test_paths();
        let legacy = root.join("saves").join("cardtable");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("settings.json"), "{\"from\": \"old\"}").unwrap();
        // The app wrote its own settings during a copy that was cut short.
        std::fs::create_dir_all(&paths.save_folder).unwrap();
        std::fs::write(paths.save_folder.join("settings.json"), "{\"from\": \"new\"}").unwrap();
        let staging_folder = root.join("saves").join(format!("{DEFAULT_IDENTITY}{STAGING_SUFFIX}"));
        std::fs::create_dir_all(staging_folder.join("half")).unwrap();
        let plan = plan_legacy_migration(&paths, &LEGACY_IDENTITIES).expect("the unfinished copy is still due");
        assert_eq!(plan.staging_folder, staging_folder);

        run_legacy_migration(&plan, &MigrationProgress::default()).unwrap();
        assert_eq!(Settings::load_from_folder(&paths.save_folder).text("from"), Some("old"));
        assert!(!paths.save_folder.join("half").exists(), "the half-done copy was started over");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_newest_legacy_identity_wins_and_the_avatar_path_moves() {
        let (paths, root) = test_paths();
        let cardtable = root.join("saves").join("cardtable");
        let cardflux = root.join("saves").join("cardflux");
        std::fs::create_dir_all(cardtable.join("avatar")).unwrap();
        std::fs::create_dir_all(&cardflux).unwrap();
        std::fs::write(cardtable.join("avatar").join("avatar.png"), "png").unwrap();
        let old_avatar = cardtable.join("avatar").join("avatar.png");
        let mut old_settings = Settings::load_from_folder(&cardtable);
        old_settings.set(keys::PLAYER_AVATAR_PATH, old_avatar.to_string_lossy().as_ref());
        old_settings.set("from", "cardtable");
        std::fs::write(cardflux.join("settings.json"), "{\"from\": \"cardflux\"}").unwrap();

        let plan = plan_legacy_migration(&paths, &LEGACY_IDENTITIES).unwrap();
        run_legacy_migration(&plan, &MigrationProgress::default()).unwrap();
        let settings = Settings::load_from_folder(&paths.save_folder);
        assert_eq!(settings.text("from"), Some("cardtable"));
        let new_avatar = paths.save_folder.join("avatar").join("avatar.png");
        assert_eq!(settings.text(keys::PLAYER_AVATAR_PATH), Some(new_avatar.to_string_lossy().as_ref()));
        assert!(new_avatar.exists());
        // The old app's own settings are untouched.
        assert_eq!(
            Settings::load_from_folder(&cardtable).text(keys::PLAYER_AVATAR_PATH),
            Some(old_avatar.to_string_lossy().as_ref())
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nothing_is_due_without_an_old_folder() {
        let (paths, root) = test_paths();
        assert_eq!(plan_legacy_migration(&paths, &LEGACY_IDENTITIES), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sizes_read_in_megabytes_or_gigabytes() {
        assert_eq!(size_text(340 * 1024 * 1024), "340 MB");
        assert_eq!(size_text(3 * 1024 * 1024 * 1024 + 300 * 1024 * 1024), "3.3 GB");
    }
}
