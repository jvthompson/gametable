//! The local deck library. Port of `src/data/deck_library.lua`.
//!
//! A deck's display name is its file name without `.json`. Player decks live in
//! `<deck library>/<game id>/`; built-in decks ship inside `<game>/decks/`, or `<game>/_decks/`
//! when that exists.

use std::path::Path;

use gametable_core::json::{self, JsonValue};
use gametable_core::models::{DeckConfig, GameDefinition, ModelError};

use super::file_system::join_path;
use super::game_files;

/// A readable deck for the game.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryDeck {
    pub display_name: String,
    /// `None` for decks that live on a server.
    pub file_path: Option<String>,
    pub deck: DeckConfig,
    pub is_built_in: bool,
}

impl LibraryDeck {
    /// `D.cardCount(entry)`.
    pub fn card_count(&self) -> i64 {
        self.deck.total_card_count()
    }
}

/// A deck for the game that can't be used: saved in the old format (local), or unreadable
/// (server).
#[derive(Debug, Clone, PartialEq)]
pub struct IncompatibleDeck {
    pub display_name: String,
    pub file_path: Option<String>,
    pub is_built_in: bool,
}

/// What a scan found, each list sorted by name.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeckScan {
    pub entries: Vec<LibraryDeck>,
    pub incompatible: Vec<IncompatibleDeck>,
}

/// Why [`read_deck_file`] failed (`"unreadable"`, `"invalid"`, `"legacy"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeckReadError {
    Unreadable,
    Invalid,
    Legacy,
}

fn has_json_extension(name: &str) -> bool {
    name.to_lowercase().ends_with(".json")
}

/// Reads one deck file as the scan does: `None` when it isn't a JSON object.
fn read_raw_object(path: &str) -> Option<JsonValue> {
    let bytes = game_files::read(path).unwrap_or_default();
    json::decode_bytes(&bytes).ok().filter(|raw| matches!(raw, JsonValue::Object(_) | JsonValue::Array(_)))
}

fn scan_folder(folder: &str, game: &GameDefinition, is_built_in: bool) -> DeckScan {
    let mut scan = DeckScan::default();
    for item in game_files::list_folder(folder).unwrap_or_default() {
        if item.is_folder || !has_json_extension(&item.name) {
            continue;
        }
        let path = join_path(&[folder, &item.name]);
        let display_name = item.name[..item.name.len() - 5].to_string();
        let Some(raw) = read_raw_object(&path) else { continue };
        match DeckConfig::from_json(&raw) {
            Ok(deck) => {
                if deck.game_id == game.id {
                    scan.entries.push(LibraryDeck { display_name, file_path: Some(path), deck, is_built_in });
                }
            }
            Err(ModelError::LegacyDeckFormat) => {
                if raw.get("gameId").and_then(JsonValue::as_str) == Some(game.id.as_str()) {
                    scan.incompatible.push(IncompatibleDeck { display_name, file_path: Some(path), is_built_in });
                }
            }
            Err(_) => {}
        }
    }
    scan.entries.sort_by(|first, second| first.display_name.cmp(&second.display_name));
    scan.incompatible.sort_by(|first, second| first.display_name.cmp(&second.display_name));
    scan
}

/// `<deck library>/<game id>`.
pub fn game_deck_folder(root_path: &str, game: &GameDefinition) -> String {
    join_path(&[root_path, &game.id])
}

/// The player's own decks for a game.
pub fn load_decks_for_game(root_path: &str, game: &GameDefinition) -> DeckScan {
    scan_folder(&game_deck_folder(root_path, game), game, false)
}

/// A game's built-in decks folder: `_decks` when present, else `decks`.
pub fn built_in_decks_folder(folder_path: &str) -> String {
    let preferred = join_path(&[folder_path, "_decks"]);
    if game_files::is_dir(&preferred) { preferred } else { join_path(&[folder_path, "decks"]) }
}

pub fn load_built_in_decks_for_game(game: &GameDefinition) -> DeckScan {
    match &game.folder_path {
        Some(folder_path) => scan_folder(&built_in_decks_folder(folder_path), game, true),
        None => DeckScan::default(),
    }
}

/// The player's own decks first, then the game's built-in ones.
pub fn load_all_decks_for_game(root_path: &str, game: &GameDefinition) -> DeckScan {
    let mut scan = load_decks_for_game(root_path, game);
    let built_in = load_built_in_decks_for_game(game);
    scan.entries.extend(built_in.entries);
    scan.incompatible.extend(built_in.incompatible);
    scan
}

/// `D.readDeckFile(path)`: a single deck file (e.g. "Browse for file...").
pub fn read_deck_file(path: &str) -> Result<DeckConfig, DeckReadError> {
    let bytes = std::fs::read(path).map_err(|_| DeckReadError::Unreadable)?;
    let raw = json::decode_bytes(&bytes).map_err(|_| DeckReadError::Invalid)?;
    if !matches!(raw, JsonValue::Object(_) | JsonValue::Array(_)) {
        return Err(DeckReadError::Invalid);
    }
    DeckConfig::from_json(&raw).map_err(|error| match error {
        ModelError::LegacyDeckFormat => DeckReadError::Legacy,
        _ => DeckReadError::Invalid,
    })
}

/// `D.validateName(name, emptyMessage)`: the error for an unusable deck or save name, if any.
/// `name` is already trimmed.
pub fn validate_name(name: &str, empty_message: Option<&str>) -> Option<String> {
    if name.is_empty() {
        return Some(empty_message.unwrap_or("Enter a deck name.").to_string());
    }
    if name == "." || name == ".." {
        return Some("Invalid name.".to_string());
    }
    if name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        return Some("Name cannot contain: / \\ : * ? \" < > |".to_string());
    }
    None
}

/// `<deck library>/<game id>/<name>.json`.
pub fn deck_path(root_path: &str, game_id: &str, name: &str) -> String {
    join_path(&[root_path, game_id, &format!("{name}.json")])
}

pub fn deck_exists(root_path: &str, game_id: &str, name: &str) -> bool {
    Path::new(&deck_path(root_path, game_id, name)).is_file()
}

/// Writes `<deck library>/<game id>/<name>.json` as compact JSON; returns the path.
pub fn write_deck(root_path: &str, game_id: &str, name: &str, deck: &DeckConfig) -> Result<String, String> {
    std::fs::create_dir_all(join_path(&[root_path, game_id])).map_err(|error| error.to_string())?;
    let path = deck_path(root_path, game_id, name);
    let text = json::encode_compact(&deck.to_json()).map_err(|error| error.to_string())?;
    std::fs::write(&path, text).map_err(|error| format!("{path}: {error}"))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gametable_core::models::{DeckEntry, SubDeck};

    #[test]
    fn names() {
        assert_eq!(validate_name("", None).as_deref(), Some("Enter a deck name."));
        assert_eq!(validate_name("", Some("Enter a save name.")).as_deref(), Some("Enter a save name."));
        assert_eq!(validate_name("..", None).as_deref(), Some("Invalid name."));
        assert!(validate_name("a:b", None).is_some());
        assert_eq!(validate_name("My Deck", None), None);
    }

    #[test]
    fn scans_writes_and_reads_decks() {
        let root = std::env::temp_dir().join(format!("gametable-decks-{}", gametable_core::uuid::new_v4_uuid()));
        let library = root.join("library");
        let game_folder = root.join("game");
        let mut game = GameDefinition::new("g", "Game");
        game.folder_path = Some(game_folder.to_string_lossy().into_owned());
        let library_text = library.to_string_lossy().into_owned();

        let deck = DeckConfig::new("g", vec![SubDeck::new("main_deck", vec![DeckEntry::new("c1", 3)])]);
        let written = write_deck(&library_text, "g", "Zed", &deck).unwrap();
        write_deck(&library_text, "g", "Alpha", &deck).unwrap();
        write_deck(&library_text, "g", "Other game", &DeckConfig::new("h", vec![])).unwrap();
        std::fs::write(library.join("g").join("Old.json"), r#"{"gameId":"g","cardIds":["c1"]}"#).unwrap();
        std::fs::write(library.join("g").join("junk.json"), "not json").unwrap();
        assert!(deck_exists(&library_text, "g", "Zed"));

        std::fs::create_dir_all(game_folder.join("decks")).unwrap();
        std::fs::create_dir_all(game_folder.join("_decks")).unwrap();
        std::fs::write(game_folder.join("decks").join("Ignored.json"), json::encode_compact(&deck.to_json()).unwrap())
            .unwrap();
        std::fs::write(game_folder.join("_decks").join("Starter.json"), json::encode_compact(&deck.to_json()).unwrap())
            .unwrap();

        let scan = load_all_decks_for_game(&library_text, &game);
        let names: Vec<_> = scan.entries.iter().map(|entry| (entry.display_name.as_str(), entry.is_built_in)).collect();
        assert_eq!(names, [("Alpha", false), ("Zed", false), ("Starter", true)]);
        assert_eq!(scan.entries[0].card_count(), 3);
        assert_eq!(scan.incompatible.iter().map(|deck| deck.display_name.as_str()).collect::<Vec<_>>(), ["Old"]);
        assert_eq!(read_deck_file(&deck_path(&library_text, "g", "Old")), Err(DeckReadError::Legacy));
        assert_eq!(read_deck_file(&written), Ok(deck));
        assert_eq!(read_deck_file(&deck_path(&library_text, "g", "junk")), Err(DeckReadError::Invalid));
        assert_eq!(read_deck_file("Z:\\missing\\nope.json"), Err(DeckReadError::Unreadable));
        std::fs::remove_dir_all(root).unwrap();
    }
}
