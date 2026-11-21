//! The server's game library. Port of the Go server's `internal/games/games.go`.
//!
//! Scans `<data>/games/<folder>/gamedef.json` (the same layout as the client's game library),
//! keeps the parts of each game the server needs (cards for packs and collections, sets, zones,
//! starter decks), and builds the file manifests clients use to download games. The manifest hash
//! must match the Go server's exactly, because clients cache games by it.
//!
//! GameTable addition: a game can also be a `.gamedef` package (a zip of the game folder) in the
//! games folder. Its files are served straight out of the package, and its manifest is built
//! exactly as for the same game unpacked, so clients see the same hash either way. A package
//! replaces a folder with the same game id.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use gametable_core::game_archive::{GameArchive, is_game_package_path, open_game_archive};

use sha2::{Digest, Sha256};

use gametable_core::json::{self, JsonValue};

use crate::store::sha256_hex;

/// The built-in Standard 52-Card Deck, served when no library folder has the same id.
pub const STANDARD_52_GAMEDEF: &[u8] = include_bytes!("../../../assets/games/standard_52/standard_52.json");

/// File types included in a game's download manifest.
const ASSET_EXTENSIONS: [&str; 9] = ["png", "jpg", "jpeg", "webp", "gif", "bmp", "json", "md", "pdf"];

/// Folders the client writes into its own copy; never part of the manifest.
const CLIENT_ONLY_FOLDERS: [&str; 2] = ["savegames", "gamelogs"];

/// The parts of a card definition the server uses.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerCard {
    pub id: String,
    pub title: String,
    /// Empty for cards outside any set.
    pub set_id: String,
    pub types: Vec<String>,
    pub unownable: bool,
    /// Always in every collection, unlimited copies.
    pub is_free_card: bool,
    /// The card's JSON as written in the game definition.
    pub raw: JsonValue,
}

impl ServerCard {
    /// Whether the card can come from packs and starter decks.
    pub fn is_collectible(&self) -> bool {
        !self.unownable && !self.is_free_card
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerSet {
    pub id: String,
    pub name: String,
    /// The set's `packSettings` JSON, if any.
    pub pack_settings: Option<JsonValue>,
}

/// A built-in deck marked `isStarter`; collection-mode servers sell it for tokens.
#[derive(Debug, Clone, PartialEq)]
pub struct StarterDeck {
    pub name: String,
    /// The deck file as stored on disk.
    pub deck_bytes: Vec<u8>,
    /// Card id -> copies, summed over every subdeck.
    pub card_quantities: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerZone {
    pub id: String,
    pub shared: bool,
    pub standard_deck: bool,
    pub deck_name: String,
}

/// One downloadable file of a game.
#[derive(Debug, Clone, PartialEq)]
pub struct ManifestFile {
    /// Slash-separated, relative to the game folder.
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// One library entry.
#[derive(Debug, Clone)]
pub struct ServerGame {
    pub id: String,
    pub name: String,
    /// `None` for a game compiled into the server, or one installed as a `.gamedef` package.
    pub folder: Option<PathBuf>,
    /// The `.gamedef` package the game's files are read from, when it is one.
    pub package: Option<Arc<GameArchive>>,
    /// `gamedef.json` exactly as stored.
    pub definition_bytes: Vec<u8>,
    pub cards: HashMap<String, ServerCard>,
    pub sets: Vec<ServerSet>,
    pub zones: Vec<ServerZone>,
    /// Sorted by name.
    pub starter_decks: Vec<StarterDeck>,
    /// Sorted by path.
    pub files: Vec<ManifestFile>,
    pub manifest_hash: String,
}

impl ServerGame {
    /// The cards in a set, sorted by id.
    pub fn set_cards(&self, set_id: &str) -> Vec<&ServerCard> {
        let mut cards: Vec<&ServerCard> = self.cards.values().filter(|card| card.set_id == set_id).collect();
        cards.sort_by(|a, b| a.id.cmp(&b.id));
        cards
    }

    pub fn set(&self, set_id: &str) -> Option<&ServerSet> {
        self.sets.iter().find(|set| set.id == set_id)
    }

    pub fn starter_deck(&self, name: &str) -> Option<&StarterDeck> {
        self.starter_decks.iter().find(|deck| deck.name == name)
    }

    pub fn manifest_file(&self, path: &str) -> Option<&ManifestFile> {
        self.files.iter().find(|file| file.path == path)
    }

    /// Reads a file listed in the manifest.
    pub fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
        if self.manifest_file(path).is_none() {
            return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        }
        if let Some(package) = &self.package {
            return package.read(&self.package_entry_path(package, path));
        }
        match &self.folder {
            None => Ok(self.definition_bytes.clone()), // embedded games only have gamedef.json
            Some(folder) => std::fs::read(folder.join(path)),
        }
    }

    /// Where a manifest path is inside the package: the definition is listed as `gamedef.json`
    /// whatever its case in the package.
    fn package_entry_path(&self, package: &GameArchive, manifest_path: &str) -> String {
        match package.game_definition_path() {
            Some(definition_path) if manifest_path == "gamedef.json" => definition_path.to_string(),
            _ => manifest_path.to_string(),
        }
    }

    /// Whether the game has files of its own (a folder or a package), so an admin can remove it.
    pub fn is_removable(&self) -> bool {
        self.folder.is_some() || self.package.is_some()
    }

    /// Opens a file listed in the manifest for streaming.
    pub fn open_file(&self, path: &str) -> std::io::Result<Box<dyn std::io::Read + Send>> {
        if self.manifest_file(path).is_none() {
            return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        }
        if let Some(package) = &self.package {
            return Ok(Box::new(std::io::Cursor::new(package.read(&self.package_entry_path(package, path))?)));
        }
        match &self.folder {
            None => Ok(Box::new(std::io::Cursor::new(self.definition_bytes.clone()))),
            Some(folder) => Ok(Box::new(std::fs::File::open(folder.join(path))?)),
        }
    }

    /// A built-in deck by display name (for shared zones' `deckName`).
    pub fn built_in_deck(&self, name: &str) -> Option<Vec<u8>> {
        if name.contains(['/', '\\']) {
            return None;
        }
        if let Some(package) = &self.package {
            return package.read(&format!("{}/{name}.json", packaged_decks_folder(package))).ok();
        }
        let folder = self.folder.as_ref()?;
        std::fs::read(built_in_decks_folder(folder).join(format!("{name}.json"))).ok()
    }
}

/// `<folder>/_decks` when it exists, otherwise `<folder>/decks`.
pub fn built_in_decks_folder(folder: &Path) -> PathBuf {
    let preferred = folder.join("_decks");
    if preferred.is_dir() { preferred } else { folder.join("decks") }
}

/// The same choice inside a package: `_decks` when it exists, otherwise `decks`.
fn packaged_decks_folder(package: &GameArchive) -> &'static str {
    if package.contains_folder("_decks") { "_decks" } else { "decks" }
}

/// Whether a file inside a game folder belongs in its manifest: no client-only or dot folder on
/// the way to it, and an asset extension (the same rules as the folder walk).
fn is_manifest_path(relative_path: &str) -> bool {
    let parts: Vec<&str> = relative_path.split('/').collect();
    let (file_name, folders) = parts.split_last().expect("split always yields one part");
    if folders
        .iter()
        .any(|folder| CLIENT_ONLY_FOLDERS.contains(&folder.to_lowercase().as_str()) || folder.starts_with('.'))
    {
        return false;
    }
    Path::new(file_name)
        .extension()
        .is_some_and(|extension| ASSET_EXTENSIONS.contains(&extension.to_string_lossy().to_lowercase().as_str()))
}

fn text_field(value: &JsonValue, key: &str) -> Option<String> {
    value.get(key).and_then(JsonValue::as_str).map(str::to_string)
}

fn flag_field(value: &JsonValue, key: &str) -> bool {
    value.get(key).is_some_and(JsonValue::is_exactly_true)
}

/// A card the way Go's `json.Unmarshal` into `Card` sees it; `None` when it would fail or has no id.
fn parse_server_card(raw: &JsonValue, set_id: &str) -> Option<ServerCard> {
    let wrong_type = |key: &str, expected: fn(&JsonValue) -> bool| raw.get(key).is_some_and(|value| !expected(value));
    if !matches!(raw, JsonValue::Object(_))
        || wrong_type("id", |v| v.as_str().is_some())
        || wrong_type("cardTitle", |v| v.as_str().is_some())
        || wrong_type("unownable", |v| v.as_bool().is_some())
        || wrong_type("isFreeCard", |v| v.as_bool().is_some())
        || wrong_type("types", |v| v.as_array().is_some_and(|items| items.iter().all(|item| item.as_str().is_some())))
    {
        return None;
    }
    let id = text_field(raw, "id").filter(|id| !id.is_empty())?;
    let types = raw
        .get("types")
        .and_then(JsonValue::as_array)
        .map(|items| items.iter().filter_map(JsonValue::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    Some(ServerCard {
        id,
        title: text_field(raw, "cardTitle").unwrap_or_default(),
        set_id: set_id.to_string(),
        types,
        unownable: flag_field(raw, "unownable"),
        is_free_card: flag_field(raw, "isFreeCard"),
        raw: raw.clone(),
    })
}

/// Parses the server's view of a game definition.
pub fn parse_game_definition(definition_bytes: &[u8]) -> Result<ServerGame, String> {
    let value = json::decode_bytes(definition_bytes).map_err(|error| error.to_string())?;
    if !matches!(value, JsonValue::Object(_)) {
        return Err("game definition is not an object".to_string());
    }
    let id = text_field(&value, "id").filter(|id| !id.is_empty()).ok_or("missing id")?;
    let mut game = ServerGame {
        id,
        name: text_field(&value, "name").unwrap_or_default(),
        folder: None,
        package: None,
        definition_bytes: definition_bytes.to_vec(),
        cards: HashMap::new(),
        sets: Vec::new(),
        zones: Vec::new(),
        starter_decks: Vec::new(),
        files: Vec::new(),
        manifest_hash: String::new(),
    };
    let add_card = |raw: &JsonValue, set_id: &str, game: &mut ServerGame| {
        if let Some(card) = parse_server_card(raw, set_id) {
            game.cards.entry(card.id.clone()).or_insert(card); // the first card with an id wins
        }
    };
    for raw_set in value.get("sets").and_then(JsonValue::as_array).map(Vec::as_slice).unwrap_or(&[]) {
        let set_id = text_field(raw_set, "id").unwrap_or_default();
        game.sets.push(ServerSet {
            id: set_id.clone(),
            name: text_field(raw_set, "name").unwrap_or_default(),
            pack_settings: raw_set.get("packSettings").cloned(),
        });
        for raw_card in raw_set.get("cards").and_then(JsonValue::as_array).map(Vec::as_slice).unwrap_or(&[]) {
            add_card(raw_card, &set_id, &mut game);
        }
    }
    // Unlike the client, the server also reads top-level cards when sets exist.
    for raw_card in value.get("cards").and_then(JsonValue::as_array).map(Vec::as_slice).unwrap_or(&[]) {
        add_card(raw_card, "", &mut game);
    }
    for raw_zone in value.get("zones").and_then(JsonValue::as_array).map(Vec::as_slice).unwrap_or(&[]) {
        game.zones.push(ServerZone {
            id: text_field(raw_zone, "id").unwrap_or_default(),
            shared: flag_field(raw_zone, "shared"),
            standard_deck: flag_field(raw_zone, "standardDeck"),
            deck_name: text_field(raw_zone, "deckName").unwrap_or_default(),
        });
    }
    Ok(game)
}

/// The first 16 hex characters of SHA-256 over `"<path>:<sha256>\n"` for each file in order.
pub fn manifest_hash(files: &[ManifestFile]) -> String {
    let mut hasher = Sha256::new();
    for file in files {
        hasher.update(format!("{}:{}\n", file.path, file.sha256).as_bytes());
    }
    let digest: String = hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
    digest[..16].to_string()
}

/// Starter decks in `folder` for `game_id`; unreadable or invalid files are skipped.
fn scan_starter_decks(folder: &Path, game_id: &str) -> Vec<StarterDeck> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let deck_files = entries.filter_map(Result::ok).filter_map(|entry| {
        let path = entry.path();
        let is_json = path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
        if path.is_dir() || !is_json {
            return None;
        }
        let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        Some((name, std::fs::read(&path).ok()?))
    });
    starter_decks_from_files(deck_files, game_id)
}

/// Starter decks in a package's `_decks` (or `decks`) folder.
fn scan_packaged_starter_decks(package: &GameArchive, game_id: &str) -> Vec<StarterDeck> {
    let folder = packaged_decks_folder(package);
    let deck_files = package.list(folder).unwrap_or_default().into_iter().filter_map(|(file_name, is_folder)| {
        let is_json = Path::new(&file_name).extension().is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
        if is_folder || !is_json {
            return None;
        }
        let name = Path::new(&file_name).file_stem().unwrap_or_default().to_string_lossy().to_string();
        Some((name, package.read(&format!("{folder}/{file_name}")).ok()?))
    });
    starter_decks_from_files(deck_files, game_id)
}

/// The starter decks among `(name, file bytes)` deck files.
fn starter_decks_from_files(deck_files: impl Iterator<Item = (String, Vec<u8>)>, game_id: &str) -> Vec<StarterDeck> {
    let mut decks = Vec::new();
    for (name, bytes) in deck_files {
        let Ok(deck) = json::decode_bytes(&bytes) else { continue };
        let is_starter = flag_field(&deck, "isStarter") || flag_field(&deck, "IsStarter");
        let Some(subdecks) = deck.get("subdecks").and_then(JsonValue::as_array) else { continue };
        if !is_starter || text_field(&deck, "gameId").as_deref() != Some(game_id) {
            continue;
        }
        let mut card_quantities: BTreeMap<String, i64> = BTreeMap::new();
        let mut is_valid = true;
        for entry in subdecks
            .iter()
            .flat_map(|subdeck| subdeck.get("entries").and_then(JsonValue::as_array).into_iter().flatten())
        {
            let quantity = entry.get("quantity").and_then(JsonValue::as_f64).unwrap_or(0.0);
            if quantity.fract() != 0.0 {
                is_valid = false; // Go's int64 unmarshal rejects the whole file
                break;
            }
            if let Some(definition_id) = text_field(entry, "definitionId").filter(|id| !id.is_empty())
                && quantity > 0.0
            {
                *card_quantities.entry(definition_id).or_default() += quantity as i64;
            }
        }
        if !is_valid {
            continue;
        }
        decks.push(StarterDeck { name, deck_bytes: bytes, card_quantities });
    }
    decks.sort_by(|a, b| a.name.cmp(&b.name));
    decks
}

/// Cache key for file hashes: unchanged size and modification time reuse the hash.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FileHashKey {
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
}

/// The server's game library.
pub struct GameLibrary {
    games_folder: PathBuf,
    games_by_id: HashMap<String, ServerGame>,
    /// Game ids in display order.
    display_order: Vec<String>,
    scan_errors: Vec<String>,
    file_hash_cache: HashMap<FileHashKey, String>,
    /// Game definitions compiled into the server, by id.
    built_in_definitions: Vec<(String, &'static [u8])>,
}

impl GameLibrary {
    pub fn new(games_folder: impl Into<PathBuf>, built_in_definitions: Vec<(String, &'static [u8])>) -> Self {
        Self {
            games_folder: games_folder.into(),
            games_by_id: HashMap::new(),
            display_order: Vec::new(),
            scan_errors: Vec::new(),
            file_hash_cache: HashMap::new(),
            built_in_definitions,
        }
    }

    pub fn games_folder(&self) -> &Path {
        &self.games_folder
    }

    /// Rereads the library folder. Built-in games are added when no folder has their id.
    pub fn scan(&mut self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.games_folder)?;
        let mut folders: Vec<PathBuf> =
            std::fs::read_dir(&self.games_folder)?.filter_map(Result::ok).map(|entry| entry.path()).collect();
        let mut packages: Vec<PathBuf> = folders
            .iter()
            .filter(|path| path.is_file() && is_game_package_path(&path.to_string_lossy()))
            .cloned()
            .collect();
        packages.sort();
        folders.retain(|path| path.is_dir());
        folders.sort();

        let mut games_by_id: HashMap<String, ServerGame> = HashMap::new();
        let mut display_order = Vec::new();
        let mut scan_errors = Vec::new();
        for folder in folders {
            let folder_name = folder.file_name().unwrap_or_default().to_string_lossy().to_string();
            match self.scan_folder(&folder) {
                Err(error) => scan_errors.push(format!("{folder_name}: {error}")),
                Ok(None) => {}
                Ok(Some(game)) => {
                    if games_by_id.contains_key(&game.id) {
                        scan_errors.push(format!("{folder_name}: duplicate game id \"{}\"", game.id));
                        continue;
                    }
                    display_order.push(game.id.clone());
                    games_by_id.insert(game.id.clone(), game);
                }
            }
        }
        for package_path in packages {
            let package_name = package_path.file_name().unwrap_or_default().to_string_lossy().to_string();
            match self.scan_package(&package_path) {
                Err(error) => scan_errors.push(format!("{package_name}: {error}")),
                Ok(game) => {
                    match games_by_id.get(&game.id) {
                        Some(existing) if existing.package.is_some() => {
                            scan_errors.push(format!("{package_name}: duplicate game id \"{}\"", game.id));
                            continue;
                        }
                        // A package replaces a folder with the same id.
                        Some(_) => {}
                        None => display_order.push(game.id.clone()),
                    }
                    games_by_id.insert(game.id.clone(), game);
                }
            }
        }
        display_order.sort_by_key(|id| games_by_id[id].name.to_lowercase());

        for (id, definition) in &self.built_in_definitions {
            if games_by_id.contains_key(id) {
                continue;
            }
            match parse_game_definition(definition) {
                Err(error) => scan_errors.push(format!("built-in {id}: {error}")),
                Ok(mut game) => {
                    game.files = vec![ManifestFile {
                        path: "gamedef.json".to_string(),
                        sha256: sha256_hex(definition),
                        size: definition.len() as u64,
                    }];
                    game.manifest_hash = manifest_hash(&game.files);
                    display_order.push(game.id.clone());
                    games_by_id.insert(game.id.clone(), game);
                }
            }
        }
        self.games_by_id = games_by_id;
        self.display_order = display_order;
        self.scan_errors = scan_errors;
        Ok(())
    }

    fn scan_folder(&mut self, folder: &Path) -> Result<Option<ServerGame>, String> {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(folder)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        entries.sort();
        // The last matching name wins, as in the Go loop.
        let Some(definition_path) = entries
            .iter()
            .filter(|path| path.is_file())
            .filter(|path| {
                path.file_name().is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("gamedef.json"))
            })
            .last()
            .cloned()
        else {
            return Ok(None);
        };
        let definition_file_name = definition_path.file_name().unwrap().to_string_lossy().to_string();
        let definition_bytes = std::fs::read(&definition_path).map_err(|error| error.to_string())?;
        let mut game = parse_game_definition(&definition_bytes)?;
        game.folder = Some(folder.to_path_buf());
        game.starter_decks = scan_starter_decks(&built_in_decks_folder(folder), &game.id);

        let mut files = Vec::new();
        self.collect_manifest_files(folder, folder, &definition_file_name, &mut files)?;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        game.manifest_hash = manifest_hash(&files);
        game.files = files;
        Ok(Some(game))
    }

    /// A `.gamedef` package's game, with the manifest its files would have unpacked.
    fn scan_package(&mut self, package_path: &Path) -> Result<ServerGame, String> {
        let package = open_game_archive(package_path)?;
        let definition_path =
            package.game_definition_path().ok_or_else(|| "no gamedef.json in the package".to_string())?.to_string();
        let definition_bytes = package.read(&definition_path).map_err(|error| error.to_string())?;
        let mut game = parse_game_definition(&definition_bytes)?;
        game.starter_decks = scan_packaged_starter_decks(&package, &game.id);
        let modified = std::fs::metadata(package_path).and_then(|metadata| metadata.modified()).ok();
        let mut files = Vec::new();
        for file in package.files() {
            if !is_manifest_path(&file.relative_path) {
                continue;
            }
            let key = FileHashKey { path: package_path.join(&file.relative_path), size: file.size, modified };
            let sha256 = match self.file_hash_cache.get(&key) {
                Some(hash) => hash.clone(),
                None => {
                    let bytes = package.read(&file.relative_path).map_err(|error| error.to_string())?;
                    let hash = sha256_hex(&bytes);
                    self.file_hash_cache.insert(key, hash.clone());
                    hash
                }
            };
            let path = if file.relative_path == definition_path {
                "gamedef.json".to_string()
            } else {
                file.relative_path.clone()
            };
            files.push(ManifestFile { path, sha256, size: file.size });
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        game.manifest_hash = manifest_hash(&files);
        game.files = files;
        game.package = Some(package);
        Ok(game)
    }

    /// Walks a game folder (skipping client-only and dot folders) collecting manifest files.
    fn collect_manifest_files(
        &mut self,
        game_folder: &Path,
        current_folder: &Path,
        definition_file_name: &str,
        files: &mut Vec<ManifestFile>,
    ) -> Result<(), String> {
        let mut entries: Vec<std::fs::DirEntry> =
            std::fs::read_dir(current_folder).map_err(|error| error.to_string())?.filter_map(Result::ok).collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let file_type = entry.file_type().map_err(|error| error.to_string())?;
            if file_type.is_dir() {
                let is_skipped = CLIENT_ONLY_FOLDERS.contains(&name.to_lowercase().as_str()) || name.starts_with('.');
                if !is_skipped {
                    self.collect_manifest_files(game_folder, &path, definition_file_name, files)?;
                }
                continue;
            }
            let extension = path.extension().map(|extension| extension.to_string_lossy().to_lowercase());
            if !extension.is_some_and(|extension| ASSET_EXTENSIONS.contains(&extension.as_str())) {
                continue;
            }
            let relative_parts: Vec<String> = path
                .strip_prefix(game_folder)
                .expect("inside the game folder")
                .components()
                .map(|part| part.as_os_str().to_string_lossy().to_string())
                .collect();
            let mut relative_path = relative_parts.join("/");
            if relative_path == definition_file_name {
                relative_path = "gamedef.json".to_string(); // always sent under its canonical name
            }
            let metadata = entry.metadata().map_err(|error| error.to_string())?;
            let sha256 = self.file_hash(&path, &metadata)?;
            files.push(ManifestFile { path: relative_path, sha256, size: metadata.len() });
        }
        Ok(())
    }

    fn file_hash(&mut self, path: &Path, metadata: &std::fs::Metadata) -> Result<String, String> {
        let key = FileHashKey { path: path.to_path_buf(), size: metadata.len(), modified: metadata.modified().ok() };
        if let Some(hash) = self.file_hash_cache.get(&key) {
            return Ok(hash.clone());
        }
        let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
        let hash = sha256_hex(&bytes);
        self.file_hash_cache.insert(key, hash.clone());
        Ok(hash)
    }

    pub fn game(&self, id: &str) -> Option<&ServerGame> {
        self.games_by_id.get(id)
    }

    /// Games in display order (by lowercase name, built-ins last).
    pub fn games(&self) -> impl Iterator<Item = &ServerGame> {
        self.display_order.iter().map(|id| &self.games_by_id[id])
    }

    pub fn scan_errors(&self) -> &[String] {
        &self.scan_errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_folder(label: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("gametable_games_test_{label}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn manifest_hash_matches_the_go_format() {
        let files = vec![
            ManifestFile { path: "a.png".into(), sha256: "11".into(), size: 1 },
            ManifestFile { path: "gamedef.json".into(), sha256: "22".into(), size: 2 },
        ];
        let expected = &sha256_hex(b"a.png:11\ngamedef.json:22\n")[..16];
        assert_eq!(manifest_hash(&files), expected);
    }

    #[test]
    fn scans_folders_manifests_starter_decks_and_built_ins() {
        let root = temporary_folder("scan");
        write(
            &root.join("zeta/GameDef.json"),
            r#"{"id":"zeta","name":"Zeta","sets":[{"id":"s","name":"S","cards":[{"id":"c1","cardTitle":"One","types":["Rarity|Rare"]},{"id":"c2","unownable":true},{"id":"bad","unownable":"yes"}]}],"cards":[{"id":"loose"},{"id":"c1","cardTitle":"Duplicate"}],"zones":[{"id":"z","shared":true,"deckName":"Starter"}]}"#,
        );
        write(&root.join("zeta/s/c1.PNG"), "image");
        write(&root.join("zeta/notes.txt"), "not an asset");
        write(&root.join("zeta/savegames/save.json"), "{}");
        write(&root.join("zeta/.hidden/x.png"), "hidden");
        write(
            &root.join("zeta/_decks/Starter.json"),
            r#"{"gameId":"zeta","isStarter":true,"subdecks":[{"name":"main_deck","entries":[{"definitionId":"c1","quantity":2}]},{"name":"side","entries":[{"definitionId":"c1","quantity":1},{"definitionId":"c2","quantity":0}]}]}"#,
        );
        write(&root.join("zeta/_decks/NotStarter.json"), r#"{"gameId":"zeta","subdecks":[]}"#);
        write(&root.join("alpha/gamedef.json"), r#"{"id":"alpha","name":"alpha game","cards":[]}"#);
        write(&root.join("dupe/gamedef.json"), r#"{"id":"alpha","name":"Dupe","cards":[]}"#);
        write(&root.join("broken/gamedef.json"), r#"{"name":"No id"}"#);
        write(&root.join("empty/readme.md"), "no gamedef");

        let mut library = GameLibrary::new(&root, vec![("standard_52".into(), STANDARD_52_GAMEDEF)]);
        library.scan().unwrap();
        let ids: Vec<&str> = library.games().map(|game| game.id.as_str()).collect();
        assert_eq!(ids, ["alpha", "zeta", "standard_52"]);
        assert_eq!(library.scan_errors().len(), 2, "{:?}", library.scan_errors());

        let zeta = library.game("zeta").unwrap();
        let paths: Vec<&str> = zeta.files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, ["_decks/NotStarter.json", "_decks/Starter.json", "gamedef.json", "s/c1.PNG"]);
        assert_eq!(zeta.manifest_hash, manifest_hash(&zeta.files));
        assert_eq!(zeta.cards.len(), 3); // c1 (from the set), c2, loose; "bad" is skipped
        assert_eq!(zeta.cards["c1"].title, "One");
        assert!(!zeta.cards["c2"].is_collectible());
        assert_eq!(zeta.set_cards("s").iter().map(|card| card.id.as_str()).collect::<Vec<_>>(), ["c1", "c2"]);
        assert_eq!(zeta.starter_decks.len(), 1);
        assert_eq!(zeta.starter_decks[0].card_quantities, BTreeMap::from([("c1".to_string(), 3)]));
        assert!(zeta.built_in_deck("Starter").is_some());
        assert!(zeta.built_in_deck("../alpha/gamedef").is_none());
        assert_eq!(zeta.read_file("s/c1.PNG").unwrap(), b"image");
        assert!(zeta.read_file("notes.txt").is_err());

        let standard = library.game("standard_52").unwrap();
        assert_eq!(standard.files.len(), 1);
        assert_eq!(standard.read_file("gamedef.json").unwrap(), STANDARD_52_GAMEDEF);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Zips `folder` the way the editor's Save to Zip does: the folder at the zip's root.
    fn pack_folder(folder: &Path, package_path: &Path) {
        use std::io::Write;
        fn add(writer: &mut zip::ZipWriter<std::fs::File>, root: &Path, current: &Path, prefix: &str) {
            let mut entries: Vec<PathBuf> =
                std::fs::read_dir(current).unwrap().map(|entry| entry.unwrap().path()).collect();
            entries.sort();
            for path in entries {
                let relative = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                if path.is_dir() {
                    add(writer, root, &path, prefix);
                } else {
                    writer
                        .start_file(format!("{prefix}/{relative}"), zip::write::SimpleFileOptions::default())
                        .unwrap();
                    writer.write_all(&std::fs::read(&path).unwrap()).unwrap();
                }
            }
        }
        let mut writer = zip::ZipWriter::new(std::fs::File::create(package_path).unwrap());
        let prefix = folder.file_name().unwrap().to_string_lossy().to_string();
        add(&mut writer, folder, folder, &prefix);
        writer.finish().unwrap();
    }

    #[test]
    fn packages_serve_the_same_manifest_as_their_folder_and_replace_it() {
        let unpacked = temporary_folder("unpacked");
        write(
            &unpacked.join("zeta/GameDef.json"),
            r#"{"id":"zeta","name":"Zeta","sets":[{"id":"s","name":"S","cards":[{"id":"c1","cardTitle":"One"}]}],"zones":[{"id":"z","shared":true,"deckName":"Starter"}]}"#,
        );
        write(&unpacked.join("zeta/s/c1.PNG"), "image");
        write(&unpacked.join("zeta/notes.txt"), "not an asset");
        write(&unpacked.join("zeta/savegames/save.json"), "{}");
        write(&unpacked.join("zeta/.hidden/x.png"), "hidden");
        write(
            &unpacked.join("zeta/_decks/Starter.json"),
            r#"{"gameId":"zeta","isStarter":true,"subdecks":[{"name":"main_deck","entries":[{"definitionId":"c1","quantity":2}]}]}"#,
        );
        let mut folder_library = GameLibrary::new(&unpacked, Vec::new());
        folder_library.scan().unwrap();
        let from_folder = folder_library.game("zeta").unwrap();

        let packed = temporary_folder("packed");
        // A stale folder with the same id is replaced by the package.
        write(&packed.join("zeta/gamedef.json"), r#"{"id":"zeta","name":"Old Zeta","cards":[]}"#);
        pack_folder(&unpacked.join("zeta"), &packed.join("zeta.gamedef"));
        let mut package_library = GameLibrary::new(&packed, Vec::new());
        package_library.scan().unwrap();
        assert!(package_library.scan_errors().is_empty(), "{:?}", package_library.scan_errors());
        let ids: Vec<&str> = package_library.games().map(|game| game.id.as_str()).collect();
        assert_eq!(ids, ["zeta"]);
        let from_package = package_library.game("zeta").unwrap();

        assert_eq!(from_package.name, "Zeta");
        assert!(from_package.package.is_some() && from_package.folder.is_none() && from_package.is_removable());
        assert_eq!(from_package.files, from_folder.files);
        assert_eq!(from_package.manifest_hash, from_folder.manifest_hash);
        assert_eq!(from_package.starter_decks, from_folder.starter_decks);
        assert_eq!(from_package.read_file("gamedef.json").unwrap(), from_folder.read_file("gamedef.json").unwrap());
        assert_eq!(from_package.read_file("s/c1.PNG").unwrap(), b"image");
        let mut streamed = Vec::new();
        std::io::Read::read_to_end(&mut from_package.open_file("s/c1.PNG").unwrap(), &mut streamed).unwrap();
        assert_eq!(streamed, b"image");
        assert!(from_package.read_file("notes.txt").is_err());
        assert_eq!(from_package.built_in_deck("Starter"), from_folder.built_in_deck("Starter"));

        drop(package_library);
        gametable_core::game_archive::forget_game_archive(&packed.join("zeta.gamedef"));
        let _ = std::fs::remove_dir_all(&unpacked);
        let _ = std::fs::remove_dir_all(&packed);
    }

    /// Reads (never writes) the real library to check every game parses. Run with
    /// `cargo test -p gametable-server -- --ignored real_library`.
    #[test]
    #[ignore]
    fn real_library_scans_without_errors() {
        let mut library = GameLibrary::new("D:/CardfluxLibrary/game_library", vec![]);
        library.scan().unwrap();
        for game in library.games() {
            println!(
                "{} ({}): {} cards, {} files, manifest {}",
                game.name,
                game.id,
                game.cards.len(),
                game.files.len(),
                game.manifest_hash
            );
        }
        assert!(library.scan_errors().is_empty(), "{:?}", library.scan_errors());
    }
}
