//! Well-known folders. Port of `src/data/paths.lua`.
//!
//! The save folder is SDL's per-user preference folder for the app identity (`%APPDATA%\gametable`
//! on Windows). The files in it keep the original's formats; on first launch the original
//! CardTable app's folder (or the older CardFlux one) is copied in (`legacy_migration`).

use std::path::{Path, PathBuf};

use super::settings::{Settings, keys};

/// The app's identity: its save folder's name.
pub const DEFAULT_IDENTITY: &str = "gametable";
/// Identities from before the app was renamed, newest first. The first one that has a save folder
/// is copied over once: `cardtable` (the LÖVE CardTable app), then `cardflux` (its earlier name).
pub const LEGACY_IDENTITIES: [&str; 2] = ["cardtable", "cardflux"];

/// The app's own folder and its per-user save folder.
#[derive(Debug, Clone, PartialEq)]
pub struct AppPaths {
    /// The folder holding the executable; default libraries live next to it.
    pub app_folder: PathBuf,
    /// The per-user save folder (created on demand).
    pub save_folder: PathBuf,
}

impl AppPaths {
    /// The real folders for an identity (`--identity <name>` picks another save folder).
    pub fn for_identity(identity: &str) -> Result<AppPaths, String> {
        let save_folder = sdl3::filesystem::get_pref_path("", identity).map_err(|error| error.to_string())?;
        let app_folder = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(AppPaths { app_folder, save_folder: strip_trailing_separator(save_folder) })
    }

    /// The save folder, created if missing.
    pub fn save_folder(&self) -> &Path {
        let _ = std::fs::create_dir_all(&self.save_folder);
        &self.save_folder
    }

    /// Where the libraries default to: next to the executable on Windows (the original's portable
    /// folder install). On macOS (inside a signed, read-only `.app`) and Linux (a system or
    /// AppImage install) the app's folder isn't writable, so the save folder is used instead.
    fn default_library_parent(&self) -> PathBuf {
        if cfg!(windows) { self.app_folder.clone() } else { self.save_folder.clone() }
    }

    pub fn default_games_folder(&self) -> PathBuf {
        self.default_library_parent().join("game_library")
    }

    pub fn default_decks_folder(&self) -> PathBuf {
        self.default_library_parent().join("deck_library")
    }

    /// The games library: the saved folder when it is absolute, else the default. Development
    /// hook: `GAMETABLE_GAMES_FOLDER=<folder>` uses that folder instead (nothing is saved).
    pub fn games_folder(&self, settings: &Settings) -> PathBuf {
        if let Some(folder) = std::env::var_os("GAMETABLE_GAMES_FOLDER").filter(|folder| !folder.is_empty()) {
            return PathBuf::from(folder);
        }
        saved_folder(settings, keys::GAMES_DIRECTORY_PATH).unwrap_or_else(|| self.default_games_folder())
    }

    pub fn decks_folder(&self, settings: &Settings) -> PathBuf {
        saved_folder(settings, keys::DECKS_DIRECTORY_PATH).unwrap_or_else(|| self.default_decks_folder())
    }

    pub fn avatar_folder(&self) -> PathBuf {
        self.save_folder().join("avatar")
    }
}

/// Saves a library folder. Only absolute paths are accepted.
pub fn set_games_folder(settings: &mut Settings, folder: &str) -> Result<(), String> {
    set_saved_folder(settings, keys::GAMES_DIRECTORY_PATH, folder)
}

pub fn set_decks_folder(settings: &mut Settings, folder: &str) -> Result<(), String> {
    set_saved_folder(settings, keys::DECKS_DIRECTORY_PATH, folder)
}

fn set_saved_folder(settings: &mut Settings, key: &str, folder: &str) -> Result<(), String> {
    if !is_absolute(folder) {
        return Err(format!("library path must be absolute: {folder}"));
    }
    settings.set(key, folder);
    Ok(())
}

/// A saved library path is only used when it is absolute; anything else (e.g. a bare folder name
/// from an old file-dialog bug) falls back to the default.
fn saved_folder(settings: &Settings, key: &str) -> Option<PathBuf> {
    settings.text(key).filter(|path| is_absolute(path)).map(PathBuf::from)
}

/// Absolute the way the original decides it: `C:\`/`C:/` or a `\\`-style UNC root on Windows,
/// a leading `/` elsewhere.
pub fn is_absolute(path: &str) -> bool {
    if cfg!(windows) {
        let bytes = path.as_bytes();
        let drive_root =
            bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'/' | b'\\');
        let unc_root = bytes.len() >= 2 && matches!(bytes[0], b'/' | b'\\') && matches!(bytes[1], b'/' | b'\\');
        drive_root || unc_root
    } else {
        path.starts_with('/')
    }
}

fn strip_trailing_separator(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    let trimmed = text.trim_end_matches(['/', '\\']);
    if trimmed.len() == text.len() || trimmed.is_empty() { path } else { PathBuf::from(trimmed) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_paths() -> (AppPaths, PathBuf) {
        let root = std::env::temp_dir().join(format!("gametable-paths-{}", gametable_core::uuid::new_v4_uuid()));
        let paths = AppPaths { app_folder: root.join("app"), save_folder: root.join("saves").join(DEFAULT_IDENTITY) };
        (paths, root)
    }

    #[test]
    #[cfg(windows)]
    fn absolute_paths_like_the_original() {
        assert!(is_absolute("D:\\CardfluxLibrary"));
        assert!(is_absolute("d:/x"));
        assert!(is_absolute("\\\\server\\share"));
        assert!(!is_absolute("game_library"));
        assert!(!is_absolute("C:relative"));
        assert!(!is_absolute(""));
    }

    #[test]
    fn library_folders_fall_back_to_defaults() {
        let (paths, root) = test_paths();
        let default_parent = if cfg!(windows) { root.join("app") } else { paths.save_folder.clone() };
        let mut settings = Settings::in_memory();
        assert_eq!(paths.games_folder(&settings), default_parent.join("game_library"));
        settings.set(keys::GAMES_DIRECTORY_PATH, "relative_name");
        assert_eq!(paths.games_folder(&settings), default_parent.join("game_library"));
        let absolute = if cfg!(windows) { "D:\\Games" } else { "/games" };
        set_games_folder(&mut settings, absolute).unwrap();
        assert_eq!(paths.games_folder(&settings), PathBuf::from(absolute));
        assert!(set_decks_folder(&mut settings, "nope").is_err());
        assert_eq!(paths.decks_folder(&settings), default_parent.join("deck_library"));
    }

    #[test]
    fn sdl_pref_path_is_the_original_save_folder() {
        let paths = AppPaths::for_identity("gametable_paths_test").unwrap();
        let expected_parent = if cfg!(windows) { std::env::var_os("APPDATA").map(PathBuf::from) } else { None };
        assert!(paths.save_folder.ends_with("gametable_paths_test"), "{:?}", paths.save_folder);
        if let Some(parent) = expected_parent {
            assert_eq!(paths.save_folder.parent(), Some(parent.as_path()));
        }
        let _ = std::fs::remove_dir(&paths.save_folder);
    }
}
