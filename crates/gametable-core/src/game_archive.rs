//! Reading a game packed into one `.gamedef` file (a GameTable addition). A `.gamedef` is a zip
//! with the game folder at its root (`lorcana/gamedef.json`, `lorcana/<set>/<image>.webp`, ...),
//! as written by the editor's "Save to Zip". The client and the server both play such a game in
//! place, reading its files out of the zip instead of from a folder.
//!
//! Paths into a package are written as if the package were the game folder:
//! `D:/library/lorcana.gamedef/1st/ariel.webp` is the entry `lorcana/1st/ariel.webp` inside
//! `D:/library/lorcana.gamedef`. [`split_package_path`] takes such a path apart, and
//! [`open_game_archive`] keeps each package's table of contents open so reads stay quick.

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use zip::ZipArchive;

/// The extension of a packed game file, without the dot.
pub const GAME_PACKAGE_EXTENSION: &str = "gamedef";

/// The game definition's file name, matched without regard to case as in a folder.
const GAME_DEFINITION_FILE_NAME: &str = "gamedef.json";

/// Whether `path` names a `.gamedef` file (by its extension; the file need not exist).
pub fn is_game_package_path(path: &str) -> bool {
    let file_name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    file_name.len() > GAME_PACKAGE_EXTENSION.len() + 1
        && file_name.to_ascii_lowercase().ends_with(&format!(".{GAME_PACKAGE_EXTENSION}"))
}

/// When a component of `path` is a `.gamedef` file on disk, returns the package's path and the
/// rest of `path` inside it (with `/` separators; empty for the package itself).
pub fn split_package_path(path: &str) -> Option<(String, String)> {
    let lowercase = path.to_ascii_lowercase();
    let marker = format!(".{GAME_PACKAGE_EXTENSION}");
    let mut search_from = 0;
    while let Some(found) = lowercase[search_from..].find(&marker) {
        let end = search_from + found + marker.len();
        let ends_component = matches!(path[end..].chars().next(), None | Some('/' | '\\'));
        if ends_component && Path::new(&path[..end]).is_file() {
            let relative = path[end..].trim_start_matches(['/', '\\']).replace('\\', "/");
            return Some((path[..end].to_string(), relative));
        }
        search_from = end;
    }
    None
}

/// The length and modified time a package had when it was opened, to notice it being replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    length: u64,
    modified: Option<SystemTime>,
}

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(FileStamp { length: metadata.len(), modified: metadata.modified().ok() })
}

/// One file inside a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagedFile {
    /// Its path inside the game folder, with `/` separators (`1st/ariel.webp`).
    pub relative_path: String,
    pub size: u64,
}

/// An opened `.gamedef`: the files it holds, and the zip to read them from.
pub struct GameArchive {
    package_path: PathBuf,
    /// The folder at the zip's root, which is the game's id for a valid package.
    root_folder: String,
    files: Vec<PackagedFile>,
    /// Relative path -> index into `files` and the zip entry index.
    index_by_path: HashMap<String, (usize, usize)>,
    /// The same, keyed by the lowercase path, for names whose case differs (as Windows allows).
    index_by_lowercase_path: HashMap<String, (usize, usize)>,
    /// Every folder inside the game folder (`1st`, `_documents`, ...); "" is the game folder.
    folders: BTreeSet<String>,
    stamp: Option<FileStamp>,
    zip: Mutex<ZipArchive<BufReader<File>>>,
}

/// A zip entry name split into parts, or `None` when it could escape the folder it is unpacked
/// into (absolute paths, drive letters, `..`).
fn safe_name_parts(name: &str) -> Option<Vec<&str>> {
    if name.contains('\0') || name.starts_with(['/', '\\']) || name.contains(':') {
        return None;
    }
    let parts: Vec<&str> = name.split(['/', '\\']).filter(|part| !part.is_empty() && *part != ".").collect();
    if parts.contains(&"..") {
        return None;
    }
    Some(parts)
}

/// `1st\ariel.webp`, `/1st/ariel.webp` and `./1st/ariel.webp` all become `1st/ariel.webp`.
fn normalize_relative_path(relative_path: &str) -> String {
    relative_path.split(['/', '\\']).filter(|part| !part.is_empty() && *part != ".").collect::<Vec<_>>().join("/")
}

impl std::fmt::Debug for GameArchive {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GameArchive")
            .field("package_path", &self.package_path)
            .field("root_folder", &self.root_folder)
            .field("files", &self.files.len())
            .finish()
    }
}

impl GameArchive {
    /// Reads the package's table of contents. Fails unless every entry sits safely under one
    /// folder at the zip's root.
    pub fn open(package_path: &Path) -> Result<GameArchive, String> {
        let shown_name = package_path
            .file_name()
            .map_or_else(|| package_path.display().to_string(), |name| name.to_string_lossy().into_owned());
        let stamp = file_stamp(package_path);
        let file = File::open(package_path).map_err(|error| format!("{shown_name}: {error}"))?;
        let mut zip = ZipArchive::new(BufReader::new(file))
            .map_err(|_| format!("{shown_name} isn't a readable .gamedef (zip) file."))?;
        let mut root_folder: Option<String> = None;
        let mut files = Vec::new();
        let mut index_by_path = HashMap::new();
        let mut index_by_lowercase_path = HashMap::new();
        let mut folders = BTreeSet::from([String::new()]);
        for entry_index in 0..zip.len() {
            let entry = zip.by_index_raw(entry_index).map_err(|error| format!("{shown_name}: {error}"))?;
            let name = entry.name().map_err(|error| format!("{shown_name}: {error}"))?.into_owned();
            let is_folder = entry.is_dir();
            let size = entry.size();
            drop(entry);
            let parts =
                safe_name_parts(&name).ok_or_else(|| format!("{shown_name} contains an unsafe path: {name}"))?;
            let Some((first, rest)) = parts.split_first() else { continue };
            match &root_folder {
                None => root_folder = Some(first.to_string()),
                Some(root) if root != first => {
                    return Err(format!("{shown_name} must contain exactly one folder at its root."));
                }
                Some(_) => {}
            }
            if rest.is_empty() {
                if is_folder {
                    continue;
                }
                return Err(format!("{shown_name} must contain exactly one folder at its root."));
            }
            let folder_depth = if is_folder { rest.len() } else { rest.len() - 1 };
            for depth in 1..=folder_depth {
                folders.insert(rest[..depth].join("/"));
            }
            if is_folder {
                continue;
            }
            let relative_path = rest.join("/");
            let position = (files.len(), entry_index);
            index_by_lowercase_path.entry(relative_path.to_lowercase()).or_insert(position);
            index_by_path.insert(relative_path.clone(), position);
            files.push(PackagedFile { relative_path, size });
        }
        let root_folder = root_folder.ok_or_else(|| format!("{shown_name} is empty."))?;
        Ok(GameArchive {
            package_path: package_path.to_path_buf(),
            root_folder,
            files,
            index_by_path,
            index_by_lowercase_path,
            folders,
            stamp,
            zip: Mutex::new(zip),
        })
    }

    pub fn package_path(&self) -> &Path {
        &self.package_path
    }

    /// The folder at the zip's root (the game's id in a valid package).
    pub fn root_folder(&self) -> &str {
        &self.root_folder
    }

    /// Every file in the game folder, in the zip's order.
    pub fn files(&self) -> &[PackagedFile] {
        &self.files
    }

    fn position_of(&self, relative_path: &str) -> Option<(usize, usize)> {
        let relative_path = normalize_relative_path(relative_path);
        self.index_by_path
            .get(&relative_path)
            .or_else(|| self.index_by_lowercase_path.get(&relative_path.to_lowercase()))
            .copied()
    }

    pub fn contains_file(&self, relative_path: &str) -> bool {
        self.position_of(relative_path).is_some()
    }

    pub fn contains_folder(&self, relative_path: &str) -> bool {
        let relative_path = normalize_relative_path(relative_path);
        self.folders.contains(&relative_path)
            || self.folders.iter().any(|folder| folder.eq_ignore_ascii_case(&relative_path))
    }

    /// The size of a file, or `None` when there is no such file.
    pub fn file_size(&self, relative_path: &str) -> Option<u64> {
        self.position_of(relative_path).map(|(file_index, _)| self.files[file_index].size)
    }

    /// A file's bytes.
    pub fn read(&self, relative_path: &str) -> std::io::Result<Vec<u8>> {
        let not_found = || {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} has no {relative_path}", self.package_path.display()),
            )
        };
        let (file_index, entry_index) = self.position_of(relative_path).ok_or_else(not_found)?;
        let mut bytes = Vec::with_capacity(self.files[file_index].size.min(256 * 1024 * 1024) as usize);
        let mut zip = self.zip.lock().map_err(|_| std::io::Error::other("package reader was poisoned"))?;
        let mut entry = zip.by_index(entry_index).map_err(std::io::Error::other)?;
        entry.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    /// The files and folders directly inside a folder of the game, each `(name, is_folder)`,
    /// sorted by name. `None` when there is no such folder.
    pub fn list(&self, relative_folder: &str) -> Option<Vec<(String, bool)>> {
        let relative_folder = normalize_relative_path(relative_folder);
        let actual_folder = if self.folders.contains(&relative_folder) {
            relative_folder
        } else {
            self.folders.iter().find(|folder| folder.eq_ignore_ascii_case(&relative_folder))?.clone()
        };
        let prefix = if actual_folder.is_empty() { String::new() } else { format!("{actual_folder}/") };
        let direct_child = |path: &str| -> Option<String> {
            let rest = path.strip_prefix(&prefix)?;
            (!rest.is_empty() && !rest.contains('/')).then(|| rest.to_string())
        };
        let mut children: Vec<(String, bool)> = self
            .folders
            .iter()
            .filter_map(|folder| direct_child(folder).map(|name| (name, true)))
            .chain(self.files.iter().filter_map(|file| direct_child(&file.relative_path).map(|name| (name, false))))
            .collect();
        children.sort();
        Some(children)
    }

    /// The path of `gamedef.json` inside the game folder, whatever its case.
    pub fn game_definition_path(&self) -> Option<&str> {
        self.files
            .iter()
            .map(|file| file.relative_path.as_str())
            .find(|path| path.eq_ignore_ascii_case(GAME_DEFINITION_FILE_NAME))
    }

    /// The bytes of `gamedef.json`.
    pub fn game_definition_bytes(&self) -> Result<Vec<u8>, String> {
        let path = self
            .game_definition_path()
            .ok_or_else(|| format!("No gamedef.json found inside {}.", self.package_path.display()))?;
        self.read(path).map_err(|error| error.to_string())
    }

    /// False once the file on disk has been replaced or removed since it was opened.
    fn is_current(&self) -> bool {
        self.stamp.is_some() && file_stamp(&self.package_path) == self.stamp
    }
}

fn open_archives() -> &'static Mutex<HashMap<String, Arc<GameArchive>>> {
    static OPEN_ARCHIVES: OnceLock<Mutex<HashMap<String, Arc<GameArchive>>>> = OnceLock::new();
    OPEN_ARCHIVES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// One key per file however its path is spelled (separators, and case on Windows).
fn archive_key(package_path: &Path) -> String {
    let key = package_path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) { key.to_lowercase() } else { key }
}

/// The package at `package_path`, opened once and shared; reopened when the file changes.
pub fn open_game_archive(package_path: &Path) -> Result<Arc<GameArchive>, String> {
    let key = archive_key(package_path);
    if let Some(archive) = open_archives().lock().ok().and_then(|archives| archives.get(&key).cloned())
        && archive.is_current()
    {
        return Ok(archive);
    }
    let archive = Arc::new(GameArchive::open(package_path)?);
    if let Ok(mut archives) = open_archives().lock() {
        archives.insert(key, archive.clone());
    }
    Ok(archive)
}

/// Drops the shared copy of a package (before it is replaced or deleted, so its file isn't held
/// open, and so the next read sees the new file).
pub fn forget_game_archive(package_path: &Path) {
    if let Ok(mut archives) = open_archives().lock() {
        archives.remove(&archive_key(package_path));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn test_folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("gametable_core_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    pub(crate) fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, bytes) in entries {
            if name.ends_with('/') {
                writer.add_directory(name.trim_end_matches('/'), SimpleFileOptions::default()).unwrap();
            } else {
                writer.start_file(*name, SimpleFileOptions::default()).unwrap();
                writer.write_all(bytes).unwrap();
            }
        }
        writer.finish().unwrap();
    }

    #[test]
    fn package_paths_are_recognised_and_split() {
        let folder = test_folder("split");
        let package = folder.join("mygame.gamedef");
        write_zip(&package, &[("mygame/gamedef.json", b"{}")]);
        let package_text = package.to_string_lossy().into_owned();
        assert!(is_game_package_path(&package_text));
        assert!(!is_game_package_path("D:/library/mygame"));
        assert_eq!(
            split_package_path(&format!("{package_text}/set1\\card.webp")),
            Some((package_text.clone(), "set1/card.webp".to_string()))
        );
        assert_eq!(split_package_path(&package_text), Some((package_text.clone(), String::new())));
        // A folder merely named like a package isn't one.
        let folder_text = folder.to_string_lossy().into_owned();
        assert_eq!(split_package_path(&format!("{folder_text}/other.gamedefs/card.png")), None);
        assert_eq!(split_package_path(&format!("{folder_text}/missing.gamedef/card.png")), None);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn files_and_folders_are_read_from_inside_the_root_folder() {
        let folder = test_folder("read");
        let package = folder.join("mygame.gamedef");
        write_zip(
            &package,
            &[
                ("mygame/", b""),
                ("mygame/GameDef.json", br#"{"id":"mygame"}"#),
                ("mygame/set1/Card.webp", b"card"),
                ("mygame/_documents/rules.md", b"# Rules"),
                ("mygame/_tokens/empty/", b""),
            ],
        );
        let archive = GameArchive::open(&package).unwrap();
        assert_eq!(archive.root_folder(), "mygame");
        assert_eq!(archive.game_definition_bytes().unwrap(), br#"{"id":"mygame"}"#);
        assert_eq!(archive.read("set1/Card.webp").unwrap(), b"card");
        assert_eq!(archive.read("SET1\\card.WEBP").unwrap(), b"card", "case and separators don't matter");
        assert!(archive.read("set1/missing.webp").is_err());
        assert!(archive.contains_file("_documents/rules.md"));
        assert!(archive.contains_folder("_tokens/empty"));
        assert!(archive.contains_folder(""));
        assert!(!archive.contains_folder("set2"));
        assert_eq!(archive.file_size("set1/Card.webp"), Some(4));
        assert_eq!(
            archive.list("").unwrap(),
            vec![
                ("GameDef.json".to_string(), false),
                ("_documents".to_string(), true),
                ("_tokens".to_string(), true),
                ("set1".to_string(), true),
            ]
        );
        assert_eq!(archive.list("_tokens/empty").unwrap(), vec![]);
        assert_eq!(archive.list("nowhere"), None);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn badly_shaped_packages_are_refused() {
        let folder = test_folder("refuse");
        let two_roots = folder.join("two.gamedef");
        write_zip(&two_roots, &[("a/gamedef.json", b"{}"), ("b/card.png", b"x")]);
        assert!(GameArchive::open(&two_roots).unwrap_err().contains("exactly one folder"));
        let loose_file = folder.join("loose.gamedef");
        write_zip(&loose_file, &[("gamedef.json", b"{}")]);
        assert!(GameArchive::open(&loose_file).unwrap_err().contains("exactly one folder"));
        let escaping = folder.join("escape.gamedef");
        write_zip(&escaping, &[("a/../../evil.png", b"x")]);
        assert!(GameArchive::open(&escaping).unwrap_err().contains("unsafe"));
        let not_zip = folder.join("text.gamedef");
        std::fs::write(&not_zip, b"hello").unwrap();
        assert!(GameArchive::open(&not_zip).is_err());
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn a_replaced_package_is_reopened() {
        let folder = test_folder("reopen");
        let package = folder.join("mygame.gamedef");
        write_zip(&package, &[("mygame/gamedef.json", b"first")]);
        assert_eq!(open_game_archive(&package).unwrap().game_definition_bytes().unwrap(), b"first");
        write_zip(&package, &[("mygame/gamedef.json", b"second version")]);
        assert_eq!(open_game_archive(&package).unwrap().game_definition_bytes().unwrap(), b"second version");
        forget_game_archive(&package);
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
