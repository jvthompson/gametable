//! Keeps a local copy of a server's games in `<save folder>/servers/<host_port>/games/<game id>/`
//! (the game library's layout, so loading and image resolution work unchanged). Files are compared
//! by SHA-256 against the server's manifest; only new or changed ones are fetched, on a worker
//! thread that first reuses identical copies already on this computer and downloads the rest over
//! a second connection. Port of `src/net/asset_sync.lua` and `asset_worker.lua`.

use gametable_core::game_archive::{is_game_package_path, open_game_archive};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::JoinHandle;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use gametable_core::json::{self, JsonObject, JsonValue};
use gametable_core::models::GameDefinition;
use sha2::{Digest, Sha256};

use crate::data::file_system::{base_name, is_same_path, join_path, list_folder, parent_folder};
use crate::data::game_files;
use crate::data::game_loader::load_from_folder;
use crate::data::image_resolver::clear_existence_cache;
use crate::net::asset_channel::AssetChannel;
use crate::net::server_client::{ServerClient, response_message, response_ok};

const INDEX_FILE_NAME: &str = ".cardflux_manifest.json";

/// `safe(s)`: anything but letters, digits, `.`, `-` and `_` becomes `_`.
pub fn safe_name(text: &str) -> String {
    text.chars()
        .map(
            |character| {
                if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') { character } else { '_' }
            },
        )
        .collect()
}

/// `A.cacheRoot(sc)`.
pub fn cache_root(save_folder: &Path, server_key: &str) -> String {
    join_path(&[&save_folder.to_string_lossy(), "servers", &safe_name(server_key)])
}

/// `A.gameFolder(sc, gameId)`.
pub fn game_folder(save_folder: &Path, server_key: &str, game_id: &str) -> String {
    join_path(&[&cache_root(save_folder, server_key), "games", &safe_name(game_id)])
}

/// Every `servers/` folder that may hold another cached copy: this save folder's and those of
/// sibling identities.
fn server_roots(save_folder: &Path) -> Vec<String> {
    let save = save_folder.to_string_lossy().into_owned();
    let mut roots = vec![join_path(&[&save, "servers"])];
    let parent = parent_folder(&save);
    for entry in list_folder(Path::new(&parent)).unwrap_or_default() {
        let folder = join_path(&[&parent, &entry.name]);
        if entry.is_folder && !is_same_path(&folder, &save) {
            roots.push(join_path(&[&folder, "servers"]));
        }
    }
    roots
}

fn read_index(folder: &str) -> BTreeMap<String, String> {
    std::fs::read(join_path(&[folder, INDEX_FILE_NAME]))
        .ok()
        .and_then(|bytes| json::decode_bytes(&bytes).ok())
        .and_then(|value| value.as_object().cloned())
        .map(|object| {
            object.iter().filter_map(|(path, hash)| Some((path.clone(), hash.as_str()?.to_string()))).collect()
        })
        .unwrap_or_default()
}

fn write_index(folder: &str, index: &BTreeMap<String, String>) {
    let _ = std::fs::create_dir_all(folder);
    let mut object = JsonObject::new();
    for (path, hash) in index {
        object.insert(path.clone(), hash.as_str());
    }
    if let Ok(text) = json::encode_compact(&JsonValue::Object(object)) {
        let _ = std::fs::write(join_path(&[folder, INDEX_FILE_NAME]), text);
    }
}

/// A manifest-relative path (`/`-separated) as a local path inside `folder`.
fn local_path(folder: &str, relative: &str) -> String {
    join_path(&[folder, relative])
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// One file of a manifest.
#[derive(Debug, Clone, PartialEq)]
struct ManifestFile {
    path: String,
    sha256: String,
    size: Option<u64>,
}

/// What the worker needs.
struct DownloadJob {
    host: String,
    port: u16,
    token: Option<String>,
    game_id: String,
    folder: String,
    files: Vec<ManifestFile>,
    games_folder: String,
    cache_game_folder: String,
    server_roots: Vec<String>,
}

enum WorkerMessage {
    File { path: String, sha256: String, bytes: u64, reused: bool },
    Done,
    Error(String),
}

/// Local game folders whose `gamedef.json` has this game's id, and `.gamedef` packages of it
/// (read through `game_files`, so a package works as a folder here).
fn local_game_folders(job: &DownloadJob) -> Vec<String> {
    let mut folders = Vec::new();
    for entry in list_folder(Path::new(&job.games_folder)).unwrap_or_default() {
        if !entry.is_folder {
            let package = join_path(&[&job.games_folder, &entry.name]);
            if is_game_package_path(&entry.name)
                && open_game_archive(Path::new(&package))
                    .ok()
                    .and_then(|archive| archive.game_definition_bytes().ok())
                    .and_then(|bytes| json::decode_bytes(&bytes).ok())
                    .and_then(|value| value.get("id").and_then(JsonValue::as_str).map(str::to_string))
                    .as_deref()
                    == Some(job.game_id.as_str())
            {
                folders.push(package);
            }
            continue;
        }
        let folder = join_path(&[&job.games_folder, &entry.name]);
        let definition = list_folder(Path::new(&folder))
            .unwrap_or_default()
            .into_iter()
            .find(|file| !file.is_folder && file.name.to_lowercase() == "gamedef.json");
        if let Some(definition) = definition {
            let id = std::fs::read(join_path(&[&folder, &definition.name]))
                .ok()
                .and_then(|bytes| json::decode_bytes(&bytes).ok())
                .and_then(|value| value.get("id").and_then(JsonValue::as_str).map(str::to_string));
            if id.as_deref() == Some(job.game_id.as_str()) {
                folders.push(folder);
            }
        }
    }
    folders
}

/// Every folder that may hold a copy, the cache folder itself first.
fn source_folders(job: &DownloadJob) -> Vec<String> {
    let mut folders = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |folder: String| {
        if seen.insert(folder.to_lowercase()) && game_files::is_dir(&folder) {
            folders.push(folder);
        }
    };
    add(job.folder.clone());
    for folder in local_game_folders(job) {
        add(folder);
    }
    for root in &job.server_roots {
        for entry in list_folder(Path::new(root)).unwrap_or_default() {
            if entry.is_folder {
                add(join_path(&[root, &entry.name, "games", &job.cache_game_folder]));
            }
        }
    }
    folders
}

/// Copies a verified local copy into place; true when it found one.
fn reuse_local(
    file: &ManifestFile,
    destination: &str,
    sources: &[String],
    sizes: &mut HashMap<String, HashMap<String, u64>>,
    job_folder: &str,
) -> Result<bool, String> {
    for folder in sources {
        let source = local_path(folder, &file.path);
        let directory = parent_folder(&source);
        let listing = sizes.entry(directory.clone()).or_insert_with(|| {
            game_files::list_folder(&directory)
                .unwrap_or_default()
                .into_iter()
                .filter(|entry| !entry.is_folder)
                .map(|entry| (entry.name.to_lowercase(), entry.size))
                .collect()
        });
        let Some(&size) = listing.get(&base_name(&source).to_lowercase()) else { continue };
        if file.size.is_some_and(|wanted| wanted != size) && size != 0 {
            continue;
        }
        let Ok(data) = game_files::read(&source) else { continue };
        if sha256_hex(&data) != file.sha256 {
            continue;
        }
        if !is_same_path(folder, job_folder) {
            let _ = std::fs::create_dir_all(parent_folder(destination));
            std::fs::write(destination, &data).map_err(|error| format!("could not write {destination}: {error}"))?;
        }
        return Ok(true);
    }
    Ok(false)
}

fn download(channel: &mut AssetChannel, game_id: &str, path: &str) -> Result<Vec<u8>, String> {
    channel.send("assetGet", JsonObject::new().with("gameId", game_id).with("path", path))?;
    let mut data = Vec::new();
    loop {
        let (message_type, payload) = channel.receive()?;
        if message_type == "assetError" {
            let error = payload.get("error").map(|error| error.as_str().unwrap_or("").to_string()).unwrap_or_default();
            return Err(format!("{path}: {error}"));
        }
        if message_type == "assetData" && payload.get("path").and_then(JsonValue::as_str) == Some(path) {
            let chunk = payload.get("data").and_then(JsonValue::as_str).unwrap_or("");
            data.extend(BASE64.decode(chunk).map_err(|error| error.to_string())?);
            if payload.get("done").is_some_and(JsonValue::is_exactly_true) {
                return Ok(data);
            }
        }
    }
}

fn run_worker(job: DownloadJob, out: mpsc::Sender<WorkerMessage>) {
    let result = (|| -> Result<(), String> {
        let sources = source_folders(&job);
        let mut sizes = HashMap::new();
        let mut channel: Option<AssetChannel> = None;
        for file in &job.files {
            let destination = local_path(&job.folder, &file.path);
            if reuse_local(file, &destination, &sources, &mut sizes, &job.folder)? {
                let _ = out.send(WorkerMessage::File {
                    path: file.path.clone(),
                    sha256: file.sha256.clone(),
                    bytes: 0,
                    reused: true,
                });
                continue;
            }
            if channel.is_none() {
                channel = Some(AssetChannel::open(&job.host, job.port, job.token.as_deref(), "for downloads")?);
            }
            let data = download(channel.as_mut().expect("opened above"), &job.game_id, &file.path)?;
            let _ = std::fs::create_dir_all(parent_folder(&destination));
            std::fs::write(&destination, &data).map_err(|error| format!("could not write {destination}: {error}"))?;
            let _ = out.send(WorkerMessage::File {
                path: file.path.clone(),
                sha256: file.sha256.clone(),
                bytes: data.len() as u64,
                reused: false,
            });
        }
        Ok(())
    })();
    let _ = out.send(match result {
        Ok(()) => WorkerMessage::Done,
        Err(message) => WorkerMessage::Error(message),
    });
}

/// How a download is going.
#[derive(Debug, Clone, PartialEq)]
pub enum DownloadProgress {
    /// Asking the server for the manifest.
    Checking,
    /// `reused` counts files found already on this computer.
    Downloading {
        done: usize,
        total: usize,
        bytes: u64,
        reused: usize,
    },
    Ready(Box<GameDefinition>),
    Failed(String),
}

enum DownloadState {
    Manifest {
        rid: u64,
    },
    Files {
        manifest_hash: String,
        index: BTreeMap<String, String>,
        done: usize,
        total: usize,
        bytes: u64,
        reused: usize,
        messages: Receiver<WorkerMessage>,
        thread: Option<JoinHandle<()>>,
    },
    Finished(DownloadProgress),
}

/// `A.ensureGame(sc, gameId, onDone, onProgress)`: makes sure a server game is cached and
/// current, then loads it (memoized per manifest hash on the client).
pub struct GameDownload {
    pub game_id: String,
    folder: String,
    save_folder: std::path::PathBuf,
    games_folder: String,
    token: Option<String>,
    state: DownloadState,
}

impl GameDownload {
    pub fn start(
        server: &mut ServerClient,
        save_folder: &Path,
        games_folder: &str,
        saved_token: Option<String>,
        game_id: &str,
    ) -> GameDownload {
        let folder = game_folder(save_folder, &server.key, game_id);
        let memo_current = server
            .loaded_games
            .get(game_id)
            .zip(server.game_by_id(game_id))
            .filter(|((hash, _), info)| *hash == info.manifest_hash);
        let state = match memo_current {
            Some(((_, game), _)) => DownloadState::Finished(DownloadProgress::Ready(Box::new(game.clone()))),
            None => DownloadState::Manifest {
                rid: server.request("gameManifest", JsonObject::new().with("gameId", game_id)),
            },
        };
        GameDownload {
            game_id: game_id.to_string(),
            folder,
            save_folder: save_folder.to_path_buf(),
            games_folder: games_folder.to_string(),
            token: server.token.clone().or(saved_token),
            state,
        }
    }

    fn load(&self, server: &mut ServerClient, manifest_hash: &str) -> DownloadProgress {
        clear_existence_cache();
        match load_from_folder(&self.folder).0.into_iter().next() {
            Some(game) => {
                server.loaded_games.insert(self.game_id.clone(), (manifest_hash.to_string(), game.clone()));
                DownloadProgress::Ready(Box::new(game))
            }
            None => DownloadProgress::Failed("The downloaded game could not be loaded.".to_string()),
        }
    }

    /// Advances the download; call each frame until it is Ready or Failed.
    pub fn poll(&mut self, server: &mut ServerClient) -> DownloadProgress {
        let next = match &mut self.state {
            DownloadState::Finished(progress) => return progress.clone(),
            DownloadState::Manifest { rid } => {
                let Some(response) = server.take_response(*rid) else { return DownloadProgress::Checking };
                if !response_ok(&response) {
                    DownloadState::Finished(DownloadProgress::Failed(response_message(
                        &response,
                        "The server doesn't have that game.",
                    )))
                } else {
                    self.start_files(server, &response)
                }
            }
            DownloadState::Files { manifest_hash, index, done, total, bytes, reused, messages, thread } => {
                let mut finished: Option<Option<String>> = None;
                loop {
                    match messages.try_recv() {
                        Ok(WorkerMessage::File { path, sha256, bytes: size, reused: was_reused }) => {
                            index.insert(path, sha256);
                            *done += 1;
                            *bytes += size;
                            if was_reused {
                                *reused += 1;
                            }
                            if *done % 25 == 0 {
                                write_index(&self.folder, index);
                            }
                        }
                        Ok(WorkerMessage::Done) => {
                            finished = Some(None);
                            break;
                        }
                        Ok(WorkerMessage::Error(message)) => {
                            finished = Some(Some(message));
                            break;
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            finished = Some(Some("The download stopped unexpectedly.".to_string()));
                            break;
                        }
                    }
                }
                let Some(error) = finished else {
                    return DownloadProgress::Downloading {
                        done: *done,
                        total: *total,
                        bytes: *bytes,
                        reused: *reused,
                    };
                };
                write_index(&self.folder, index);
                if let Some(thread) = thread.take() {
                    let _ = thread.join();
                }
                let manifest_hash = manifest_hash.clone();
                DownloadState::Finished(match error {
                    Some(message) => DownloadProgress::Failed(message),
                    None => self.load(server, &manifest_hash),
                })
            }
        };
        self.state = next;
        self.poll(server)
    }

    /// Compares the manifest with the local index: drops files the server no longer has and
    /// starts the worker for new or changed ones (or loads straight away).
    fn start_files(&mut self, server: &mut ServerClient, response: &JsonValue) -> DownloadState {
        let manifest_hash = response.get("manifestHash").and_then(JsonValue::as_str).unwrap_or("").to_string();
        let files: Vec<ManifestFile> = response
            .get("files")
            .and_then(JsonValue::as_array)
            .map(|files| {
                files
                    .iter()
                    .map(|file| ManifestFile {
                        path: file.get("path").and_then(JsonValue::as_str).unwrap_or("").to_string(),
                        sha256: file.get("sha256").and_then(JsonValue::as_str).unwrap_or("").to_string(),
                        size: file.get("size").and_then(JsonValue::as_f64).map(|size| size as u64),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut index = read_index(&self.folder);
        let wanted: HashSet<&str> = files.iter().map(|file| file.path.as_str()).collect();
        let todo: Vec<ManifestFile> = files
            .iter()
            .filter(|file| {
                index.get(&file.path) != Some(&file.sha256)
                    || !Path::new(&local_path(&self.folder, &file.path)).is_file()
            })
            .cloned()
            .collect();
        // Files the server dropped.
        let dropped: Vec<String> = index.keys().filter(|path| !wanted.contains(path.as_str())).cloned().collect();
        for path in dropped {
            let _ = std::fs::remove_file(local_path(&self.folder, &path));
            index.remove(&path);
        }
        if todo.is_empty() {
            write_index(&self.folder, &index);
            return DownloadState::Finished(self.load(server, &manifest_hash));
        }
        let total = todo.len();
        let job = DownloadJob {
            host: server.host.clone(),
            port: server.port,
            token: self.token.clone(),
            game_id: self.game_id.clone(),
            folder: self.folder.clone(),
            files: todo,
            games_folder: self.games_folder.clone(),
            cache_game_folder: safe_name(&self.game_id),
            server_roots: server_roots(&self.save_folder),
        };
        let (sender, messages) = mpsc::channel();
        let thread =
            std::thread::Builder::new().name("asset-download".to_string()).spawn(move || run_worker(job, sender)).ok();
        DownloadState::Files { manifest_hash, index, done: 0, total, bytes: 0, reused: 0, messages, thread }
    }
}

/// `A.cachedGame(sc, gameId)`: a game already in memory (no network).
pub fn cached_game(server: &ServerClient, game_id: &str) -> Option<GameDefinition> {
    server.loaded_games.get(game_id).map(|(_, game)| game.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_names_and_reuse() {
        assert_eq!(safe_name("127.0.0.1:51234"), "127.0.0.1_51234");
        assert_eq!(safe_name("my game/é"), "my_game__");
        let root = std::env::temp_dir().join(format!("gametable_asset_sync_{}", std::process::id()));
        let source = root.join("source");
        std::fs::create_dir_all(source.join("sub")).unwrap();
        std::fs::write(source.join("sub").join("a.png"), b"hello").unwrap();
        let file = ManifestFile { path: "sub/a.png".into(), sha256: sha256_hex(b"hello"), size: Some(5) };
        let job_folder = root.join("cache").to_string_lossy().into_owned();
        let destination = local_path(&job_folder, &file.path);
        let sources = vec![source.to_string_lossy().into_owned()];
        let mut sizes = HashMap::new();
        assert!(reuse_local(&file, &destination, &sources, &mut sizes, &job_folder).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), b"hello");
        let wrong = ManifestFile { sha256: sha256_hex(b"other"), ..file };
        assert!(!reuse_local(&wrong, &destination, &sources, &mut HashMap::new(), &job_folder).unwrap());
        let mut index = BTreeMap::new();
        index.insert("sub/a.png".to_string(), "abc".to_string());
        write_index(&job_folder, &index);
        assert_eq!(read_index(&job_folder), index);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
