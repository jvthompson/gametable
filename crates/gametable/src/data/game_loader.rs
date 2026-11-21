//! Loading game definitions from the game library. Port of `src/data/game_loader.lua`.
//!
//! GameTable addition: a game can also be a `.gamedef` package in the library root (a zip of the
//! game folder; see `gametable_core::game_archive`). A package's folder path is the package file
//! itself, and when a package and a loose folder hold the same game id, the package is used.

use std::collections::HashSet;
use std::path::Path;

use gametable_core::game_archive::{is_game_package_path, open_game_archive};
use gametable_core::json;
use gametable_core::models::{GameDefinition, build_standard_deck_cards};

use super::file_system::{join_path, list_folder};
use super::game_files;
use super::image_resolver::resolve_bare_image_path;

/// The bundled Standard 52-Card Deck (`assets/games/standard_52/standard_52.json`).
const STANDARD_DECK_JSON: &str = include_str!("../../../../assets/games/standard_52/standard_52.json");
pub const STANDARD_DECK_ID: &str = "standard_52";

/// A `gamedef.json` that could not be loaded.
#[derive(Debug, Clone, PartialEq)]
pub struct GameLoadError {
    pub path: String,
    pub error: String,
}

/// Appends the 52 generated cards (those not already present) when any zone asks for a standard
/// deck.
pub fn with_standard_deck_cards(mut game: GameDefinition) -> GameDefinition {
    if !game.zones.iter().any(|zone| zone.standard_deck) {
        return game;
    }
    let missing: Vec<_> =
        build_standard_deck_cards().into_iter().filter(|card| game.card_by_id(&card.id).is_none()).collect();
    game.cards_mut().extend(missing);
    game
}

/// Turns the bare image file names into full paths inside `folder_path` (card art inside its
/// set's folder; card backs are game-wide) and records the folder.
///
/// The original rebuilt the game from a field list that leaves out `currentTurnColor`, so a
/// loaded game never has its own current-turn colour (see the quirks log); kept as is.
fn resolve_game_image_paths(mut game: GameDefinition, folder_path: &str) -> GameDefinition {
    for card in game.cards_mut().iter_mut() {
        if let Some(bare) = &card.image_path {
            card.image_path = Some(resolve_bare_image_path(folder_path, bare, card.set_id.as_deref()));
        }
    }
    for back in &mut game.card_backs {
        if let Some(bare) = &back.image_path {
            back.image_path = Some(resolve_bare_image_path(folder_path, bare, None));
        }
    }
    game.folder_path = Some(folder_path.to_string());
    game.current_turn_color = None;
    game
}

/// `L.loadStandardDeck()`: the bundled deck (it has no folder).
pub fn load_standard_deck() -> GameDefinition {
    let raw = json::decode(STANDARD_DECK_JSON).expect("the bundled standard deck is valid JSON");
    let game = GameDefinition::from_json(&raw).expect("the bundled standard deck is a valid game");
    with_standard_deck_cards(game)
}

fn load_game_file(path: &str, folder_path: &str) -> Result<GameDefinition, String> {
    let bytes = game_files::read(path).map_err(|error| format!("{path}: {error}"))?;
    let raw = json::decode_bytes(&bytes).map_err(|error| error.to_string())?;
    let game = GameDefinition::from_json(&raw).map_err(|error| error.to_string())?;
    Ok(with_standard_deck_cards(resolve_game_image_paths(game, folder_path)))
}

/// `L.loadFromFolder`: every `gamedef.json` (name compared ignoring case) directly inside the
/// folder. Invalid files are skipped and reported in the second list.
pub fn load_from_folder(folder_path: &str) -> (Vec<GameDefinition>, Vec<GameLoadError>) {
    let (mut games, mut errors) = (Vec::new(), Vec::new());
    for entry in list_folder(Path::new(folder_path)).unwrap_or_default() {
        if entry.is_folder || entry.name.to_lowercase() != "gamedef.json" {
            continue;
        }
        let path = join_path(&[folder_path, &entry.name]);
        match load_game_file(&path, folder_path) {
            Ok(game) => games.push(game),
            Err(error) => errors.push(GameLoadError { path, error }),
        }
    }
    (games, errors)
}

/// A `.gamedef` package's game, with its image paths pointing inside the package.
pub fn load_from_package(package_path: &str) -> Result<GameDefinition, GameLoadError> {
    let load_error = |error: String| GameLoadError { path: package_path.to_string(), error };
    let archive = open_game_archive(Path::new(package_path)).map_err(load_error)?;
    let definition_path = archive
        .game_definition_path()
        .ok_or_else(|| load_error("No gamedef.json found inside the package.".to_string()))?;
    let path = join_path(&[package_path, definition_path]);
    load_game_file(&path, package_path).map_err(load_error)
}

/// `L.loadGamesFromDirectory`: the games in every direct subfolder of the library, and in every
/// `.gamedef` package there. Folders and files whose names start with a dot (such as `.gamedata`)
/// are skipped. A package replaces a folder game with the same id.
pub fn load_games_from_folder(root_path: &str) -> (Vec<GameDefinition>, Vec<GameLoadError>) {
    let (mut games, mut errors) = (Vec::new(), Vec::new());
    for entry in list_folder(Path::new(root_path)).unwrap_or_default() {
        if entry.name.starts_with('.') {
            continue;
        }
        let path = join_path(&[root_path, &entry.name]);
        if entry.is_folder {
            let (folder_games, folder_errors) = load_from_folder(&path);
            games.extend(folder_games);
            errors.extend(folder_errors);
        } else if is_game_package_path(&entry.name) {
            match load_from_package(&path) {
                Ok(game) => games.push(game),
                Err(error) => errors.push(error),
            }
        }
    }
    let packaged_ids: HashSet<String> = games
        .iter()
        .filter(|game| game.folder_path.as_deref().is_some_and(game_files::is_packaged_game_folder))
        .map(|game| game.id.clone())
        .collect();
    games.retain(|game| {
        game.folder_path.as_deref().is_some_and(game_files::is_packaged_game_folder) || !packaged_ids.contains(&game.id)
    });
    (games, errors)
}

/// `L.loadLibrary`: the library's games with the Standard 52-Card Deck always last.
pub fn load_library(root_path: &str) -> (Vec<GameDefinition>, Vec<GameLoadError>) {
    let (mut games, errors) = load_games_from_folder(root_path);
    games.push(load_standard_deck());
    (games, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_library() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("gametable-loader-{}", gametable_core::uuid::new_v4_uuid()))
    }

    #[test]
    fn standard_deck_has_its_generated_cards() {
        let game = load_standard_deck();
        assert_eq!(game.id, STANDARD_DECK_ID);
        // Its own 52 cards are in the file; no zone asks for the generated ones.
        assert_eq!(game.cards().len(), 52);
        assert!(game.card_by_id("hearts_A").is_some());
        assert_eq!(game.folder_path, None);
    }

    /// Loads a real library read-only: `GAMETABLE_LIBRARY=<folder> cargo test -- --ignored real_library`.
    #[test]
    #[ignore]
    fn real_library_loads() {
        let Ok(root) = std::env::var("GAMETABLE_LIBRARY") else { return };
        let (games, errors) = load_library(&root);
        for game in &games {
            let decks = super::super::deck_library::load_built_in_decks_for_game(game);
            println!(
                "{} ({}): {} cards, {} built-in decks",
                game.name,
                game.id,
                game.cards().len(),
                decks.entries.len()
            );
        }
        for error in &errors {
            println!("ERROR {}: {}", error.path, error.error);
        }
        assert!(games.len() > 1);
    }

    #[test]
    fn loads_valid_games_and_reports_broken_ones() {
        let root = temporary_library();
        let good = root.join("good");
        let broken = root.join("broken");
        std::fs::create_dir_all(good.join("set1")).unwrap();
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(
            good.join("GameDef.JSON"),
            r##"{"id":"g","name":"Good","currentTurnColor":"#FF0000","cardBacks":[{"id":"b","name":"B","imagePath":"back.png"}],
                "sets":[{"id":"set1","name":"Set 1","cards":[{"id":"c1","cardTitle":"One","imagePath":"one.png"}]}]}"##,
        )
        .unwrap();
        std::fs::write(broken.join("gamedef.json"), r#"{"id":"x"}"#).unwrap();
        std::fs::write(root.join("gamedef.json"), r#"{"id":"top","name":"Ignored","cards":[]}"#).unwrap();

        let root_text = root.to_string_lossy().into_owned();
        let (games, errors) = load_library(&root_text);
        assert_eq!(games.iter().map(|game| game.id.as_str()).collect::<Vec<_>>(), ["g", STANDARD_DECK_ID]);
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].path.ends_with("gamedef.json") && errors[0].error.contains("missing required field"),
            "{errors:?}"
        );

        let game = &games[0];
        let good_text = good.to_string_lossy().into_owned();
        assert_eq!(game.folder_path.as_deref(), Some(good_text.as_str()));
        assert_eq!(game.cards()[0].image_path, Some(join_path(&[&good_text, "set1", "one.png"])));
        assert_eq!(game.card_backs[0].image_path, Some(join_path(&[&good_text, "back.png"])));
        assert_eq!(game.current_turn_color, None);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn write_package(path: &Path, entries: &[(&str, &str)]) {
        use std::io::Write;
        let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, text) in entries {
            writer.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn packages_load_in_place_and_replace_folders_with_the_same_id() {
        let root = temporary_library();
        std::fs::create_dir_all(root.join("g")).unwrap();
        std::fs::create_dir_all(root.join(".gamedata").join("g")).unwrap();
        let definition = |name: &str| {
            format!(
                r#"{{"id":"g","name":"{name}","sets":[{{"id":"set1","name":"Set 1","cards":[{{"id":"c1","cardTitle":"One","imagePath":"one.png"}}]}}]}}"#
            )
        };
        std::fs::write(root.join("g").join("gamedef.json"), definition("Folder")).unwrap();
        let package = root.join("g.gamedef");
        write_package(&package, &[("g/gamedef.json", &definition("Packaged")), ("g/set1/one.png", "png")]);
        write_package(&root.join("broken.gamedef"), &[("broken/readme.txt", "no definition")]);

        let root_text = root.to_string_lossy().into_owned();
        let (games, errors) = load_games_from_folder(&root_text);
        assert_eq!(games.iter().map(|game| game.name.as_str()).collect::<Vec<_>>(), ["Packaged"]);
        assert_eq!(errors.len(), 1, "{errors:?}");
        let package_text = package.to_string_lossy().into_owned();
        assert_eq!(games[0].folder_path.as_deref(), Some(package_text.as_str()));
        let image_path = games[0].cards()[0].image_path.clone().unwrap();
        assert_eq!(image_path, join_path(&[&package_text, "set1", "one.png"]));
        assert_eq!(game_files::read(&image_path).unwrap(), b"png");
        gametable_core::game_archive::forget_game_archive(&package);
        std::fs::remove_dir_all(root).unwrap();
    }
}
