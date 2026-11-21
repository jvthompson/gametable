//! File helpers for the game definition editor. Port of `src/data/gamedef_ops.lua`
//! (`game_definition_file_ops.dart`).
//!
//! The editor works on a game whose image paths are the bare file names from `gamedef.json`
//! (never resolved), so saving writes them back unchanged.

use std::collections::HashSet;
use std::path::Path;

use gametable_core::json;
use gametable_core::models::{CARD_BACK_FILE_SUFFIX, CardDefinition, GameDefinition, TagGroup};
use gametable_core::tag_match::{encode_tag_entry, parse_tag_entry};
use gametable_core::util::trim_lua_whitespace;

use crate::data::file_system::{base_name, is_same_path, join_path, list_folder, strip_extension};
use crate::data::image_utils::rotate_landscape_images;

/// `O.imageFilterList`: the image types the editor imports.
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

/// `O.isImageFile`: the text from the last dot (even a leading one) is an image extension.
pub fn is_image_file(file_name: &str) -> bool {
    file_name.rfind('.').is_some_and(|dot| IMAGE_EXTENSIONS.contains(&file_name[dot + 1..].to_lowercase().as_str()))
}

/// `O.deriveIdFromFolderPath`: the folder's own name.
pub fn derive_id_from_folder_path(folder_path: &str) -> String {
    base_name(folder_path).to_string()
}

/// `O.uniqueCardId`: appends `_001`, `_002`, ... until the id is unused.
pub fn unique_card_id(base_id: &str, taken_ids: &HashSet<String>) -> String {
    if !taken_ids.contains(base_id) {
        return base_id.to_string();
    }
    (1..).map(|suffix| format!("{base_id}_{suffix:03}")).find(|candidate| !taken_ids.contains(candidate)).unwrap()
}

/// `O.findGameDefFile`: the folder's `gamedef.json` (name compared ignoring case).
pub fn find_game_definition_file(folder_path: &str) -> Option<String> {
    list_folder(Path::new(folder_path))?
        .into_iter()
        .find(|entry| !entry.is_folder && entry.name.to_lowercase() == "gamedef.json")
        .map(|entry| join_path(&[folder_path, &entry.name]))
}

/// `O.copyIntoFolder`: copies a file into a folder (nothing to do when it is already there),
/// optionally turning landscape art portrait. Returns the destination path.
pub fn copy_into_folder(
    source_path: &str,
    destination_folder: &str,
    file_name: &str,
    correct_card_orientation: bool,
) -> Result<String, String> {
    let destination = join_path(&[destination_folder, file_name]);
    if is_same_path(source_path, &destination) {
        return Ok(destination);
    }
    std::fs::create_dir_all(destination_folder).map_err(|error| error.to_string())?;
    std::fs::copy(source_path, &destination).map_err(|_| format!("could not copy {source_path}"))?;
    if correct_card_orientation {
        rotate_landscape_images(std::slice::from_ref(&destination));
    }
    Ok(destination)
}

/// `O.copyPickedImage`: copies a picked file into the game folder and returns its bare name.
pub fn copy_picked_file(
    source_path: &str,
    destination_folder: &str,
    correct_card_orientation: bool,
) -> Result<String, String> {
    let file_name = base_name(source_path).to_string();
    copy_into_folder(source_path, destination_folder, &file_name, correct_card_orientation)?;
    Ok(file_name)
}

/// `O.buildCardsFromImageFolder`: one card per image (sorted by byte order), skipping `*_BACK`
/// art, with ids made unique against `existing_card_ids`.
pub fn build_cards_from_image_folder(
    source_folder: &str,
    set_id: Option<&str>,
    existing_card_ids: &[String],
) -> Vec<CardDefinition> {
    let Some(entries) = list_folder(Path::new(source_folder)) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .into_iter()
        .filter(|entry| {
            !entry.is_folder
                && is_image_file(&entry.name)
                && !strip_extension(&entry.name).ends_with(CARD_BACK_FILE_SUFFIX)
        })
        .map(|entry| entry.name)
        .collect();
    names.sort();
    let mut taken: HashSet<String> = existing_card_ids.iter().cloned().collect();
    names
        .into_iter()
        .map(|name| {
            let title = strip_extension(&name).to_string();
            let id = unique_card_id(&title, &taken);
            taken.insert(id.clone());
            CardDefinition {
                image_path: Some(name),
                set_id: set_id.map(str::to_string),
                ..CardDefinition::new(id, title)
            }
        })
        .collect()
}

/// `O.writeGameDefinition`: `gamedef.json` with a 2-space indent.
pub fn write_game_definition(folder_path: &str, game: &GameDefinition) -> Result<(), String> {
    std::fs::create_dir_all(folder_path).map_err(|error| error.to_string())?;
    let text = json::encode_indented(&game.to_json()).map_err(|error| error.to_string())?;
    std::fs::write(join_path(&[folder_path, "gamedef.json"]), text).map_err(|error| error.to_string())
}

/// `O.validateGameId`: an id must work as a folder name.
pub fn validate_game_id(id: &str) -> Option<&'static str> {
    if id.is_empty() {
        return Some("Enter a game ID.");
    }
    if id == "." || id == ".." {
        return Some("Invalid ID.");
    }
    if id.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        return Some("ID cannot contain: / \\ : * ? \" < > |");
    }
    None
}

/// `O.loadGameDefFile`: loads a `gamedef.json` with its image paths left bare.
pub fn load_game_definition_file(json_path: &str, folder_path: &str) -> Result<GameDefinition, String> {
    let bytes = std::fs::read(json_path).map_err(|error| format!("{json_path}: {error}"))?;
    let raw = json::decode_bytes(&bytes).map_err(|error| error.to_string())?;
    let mut game = GameDefinition::from_json(&raw).map_err(|error| error.to_string())?;
    game.folder_path = Some(folder_path.to_string());
    Ok(game)
}

/// A `.gamedef` package's game for the editor's list: image paths left bare, and the folder path
/// is the package (so the entry screen can offer to extract it).
pub fn load_packaged_game_definition(package_path: &str) -> Result<GameDefinition, String> {
    let archive = gametable_core::game_archive::open_game_archive(Path::new(package_path))?;
    let bytes = archive.game_definition_bytes()?;
    let raw = json::decode_bytes(&bytes).map_err(|error| error.to_string())?;
    let mut game = GameDefinition::from_json(&raw).map_err(|error| error.to_string())?;
    game.folder_path = Some(package_path.to_string());
    Ok(game)
}

/// `O.scanGameLibrary`: every direct subfolder holding a loadable `gamedef.json`, by name ignoring
/// case. Unloadable files are skipped. GameTable addition: `.gamedef` packages are listed too,
/// unless a folder with the same game id is already there to edit.
pub fn scan_game_library(root: &str) -> Result<Vec<GameDefinition>, String> {
    let entries = match list_folder(Path::new(root)) {
        Some(entries) => entries,
        None if !Path::new(root).exists() => Vec::new(),
        None => return Err(format!("{root}: cannot be read")),
    };
    let mut games: Vec<GameDefinition> = entries
        .iter()
        .filter(|entry| entry.is_folder && !entry.name.starts_with('.'))
        .filter_map(|entry| {
            let folder = join_path(&[root, &entry.name]);
            let path = find_game_definition_file(&folder)?;
            load_game_definition_file(&path, &folder).ok()
        })
        .collect();
    let packaged_games: Vec<GameDefinition> = entries
        .iter()
        .filter(|entry| !entry.is_folder && gametable_core::game_archive::is_game_package_path(&entry.name))
        .filter_map(|entry| load_packaged_game_definition(&join_path(&[root, &entry.name])).ok())
        .collect();
    for packaged_game in packaged_games {
        if !games.iter().any(|game| game.id == packaged_game.id) {
            games.push(packaged_game);
        }
    }
    games.sort_by_key(|game| game.name.to_lowercase());
    Ok(games)
}

/// `O.createGameDefinition`: writes a stub `gamedef.json` under `root/id`.
pub fn create_game_definition(root: &str, id: &str) -> Result<GameDefinition, String> {
    let folder_path = join_path(&[root, id]);
    let mut game = GameDefinition::new(id, id);
    game.folder_path = Some(folder_path.clone());
    write_game_definition(&folder_path, &game)?;
    Ok(game)
}

/// Cards sharing a key: `key` and the indexes into the card list.
#[derive(Debug, Clone, PartialEq)]
pub struct CardGroup {
    pub key: String,
    pub indexes: Vec<usize>,
}

/// Groups card indexes by `key_of` (`None` skips the card), keeping groups of two or more in
/// first-seen order.
fn duplicate_groups(cards: &[CardDefinition], key_of: impl Fn(&CardDefinition) -> Option<String>) -> Vec<CardGroup> {
    let mut groups: Vec<CardGroup> = Vec::new();
    for (index, card) in cards.iter().enumerate() {
        let Some(key) = key_of(card) else { continue };
        match groups.iter_mut().find(|group| group.key == key) {
            Some(group) => group.indexes.push(index),
            None => groups.push(CardGroup { key, indexes: vec![index] }),
        }
    }
    groups.retain(|group| group.indexes.len() > 1);
    groups
}

/// `O.duplicateIdGroups`: ids shared by more than one card.
pub fn duplicate_id_groups(cards: &[CardDefinition]) -> Vec<CardGroup> {
    duplicate_groups(cards, |card| Some(card.id.clone()))
}

/// `O.duplicateImageGroups`: cards sharing both their set and their exact image (cards without
/// an image never count).
pub fn duplicate_image_groups(cards: &[CardDefinition]) -> Vec<CardGroup> {
    duplicate_groups(cards, |card| {
        let image = card.image_path.as_ref()?;
        Some(format!("{}\0{image}", card.set_id.as_deref().unwrap_or("")))
    })
}

/// `O.unionTypes`: the group's tags, first-seen order.
pub fn union_types(cards: &[CardDefinition], indexes: &[usize]) -> Vec<String> {
    let mut types: Vec<String> = Vec::new();
    for &index in indexes {
        for entry in &cards[index].types {
            if !types.contains(entry) {
                types.push(entry.clone());
            }
        }
    }
    types
}

/// `O.mergeDuplicateImageCards`: each same-set, same-image group becomes its first card with the
/// group's tags combined; the others are dropped.
pub fn merge_duplicate_image_cards(cards: &[CardDefinition]) -> Vec<CardDefinition> {
    let mut merged: Vec<Option<CardDefinition>> = cards.iter().cloned().map(Some).collect();
    for group in duplicate_image_groups(cards) {
        let types = union_types(cards, &group.indexes);
        if let Some(first) = merged[group.indexes[0]].as_mut() {
            first.types = types;
        }
        for &index in &group.indexes[1..] {
            merged[index] = None;
        }
    }
    merged.into_iter().flatten().collect()
}

/// `O.cardsOutsideSets`: with sets declared, `gamedef.json` keeps cards only inside their set, so
/// a card with no set (or an unknown one) would be lost on save.
pub fn cards_outside_sets(game: &GameDefinition) -> Vec<&CardDefinition> {
    if game.sets.is_empty() {
        return Vec::new();
    }
    game.cards()
        .iter()
        .filter(|card| card.set_id.as_ref().is_none_or(|set_id| !game.sets.iter().any(|set| &set.id == set_id)))
        .collect()
}

/// `O.rarityTags`: the tags of the group named "Rarity" (ignoring case and surrounding space).
pub fn rarity_tags(tag_groups: &[TagGroup]) -> Vec<String> {
    tag_groups
        .iter()
        .find(|group| trim_lua_whitespace(&group.name).to_lowercase() == "rarity")
        .map(|group| group.tags.clone())
        .unwrap_or_default()
}

/// `O.renameTagGroupInCards`: rewrites `"old|Tag"` entries to `"new|Tag"` so renaming a group
/// keeps its cards tagged. Other groups' and legacy bare entries are untouched.
pub fn rename_tag_group_in_cards(cards: &mut [CardDefinition], old_name: &str, new_name: &str) {
    for card in cards {
        for entry in &mut card.types {
            let (group, tag) = parse_tag_entry(entry);
            if group == Some(old_name) {
                *entry = encode_tag_entry(new_name, tag);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gametable_core::models::GameSet;

    fn card(id: &str, set: Option<&str>, image: Option<&str>, types: &[&str]) -> CardDefinition {
        CardDefinition {
            set_id: set.map(str::to_string),
            image_path: image.map(str::to_string),
            types: types.iter().map(|tag| tag.to_string()).collect(),
            ..CardDefinition::new(id, id.to_uppercase())
        }
    }

    #[test]
    fn names_and_ids() {
        assert!(is_image_file("a.PNG") && is_image_file("b.webp") && is_image_file(".jpg"));
        assert!(!is_image_file("a.json") && !is_image_file("noext"));
        assert_eq!(derive_id_from_folder_path("C:\\games\\Core Set\\"), "Core Set");
        let taken: HashSet<String> = ["x", "x_001"].map(String::from).into();
        assert_eq!(unique_card_id("y", &taken), "y");
        assert_eq!(unique_card_id("x", &taken), "x_002");
        assert_eq!(validate_game_id(""), Some("Enter a game ID."));
        assert_eq!(validate_game_id(".."), Some("Invalid ID."));
        assert_eq!(validate_game_id("a:b"), Some("ID cannot contain: / \\ : * ? \" < > |"));
        assert_eq!(validate_game_id("my game"), None);
    }

    #[test]
    fn duplicates_and_merging() {
        let cards = vec![
            card("a", Some("s"), Some("1.png"), &["T|x"]),
            card("a", Some("s"), Some("2.png"), &[]),
            card("b", Some("s"), Some("1.png"), &["T|y", "T|x"]),
            card("c", Some("t"), Some("1.png"), &[]),
            card("d", None, None, &[]),
            card("e", None, None, &[]),
        ];
        assert_eq!(duplicate_id_groups(&cards), vec![CardGroup { key: "a".into(), indexes: vec![0, 1] }]);
        let image_groups = duplicate_image_groups(&cards);
        assert_eq!(image_groups.len(), 1);
        assert_eq!(image_groups[0].indexes, vec![0, 2]);
        assert_eq!(union_types(&cards, &[0, 2]), ["T|x", "T|y"]);
        let merged = merge_duplicate_image_cards(&cards);
        assert_eq!(merged.iter().map(|card| card.id.as_str()).collect::<Vec<_>>(), ["a", "a", "c", "d", "e"]);
        assert_eq!(merged[0].types, ["T|x", "T|y"]);
    }

    #[test]
    fn sets_rarity_and_tag_renames() {
        let mut game = GameDefinition::new("g", "G").with_cards(vec![
            card("a", Some("s"), None, &[]),
            card("b", None, None, &[]),
            card("c", Some("gone"), None, &[]),
        ]);
        // No sets declared: cards are written at the top level, so nothing is lost.
        assert!(cards_outside_sets(&game).is_empty());
        game.sets.push(GameSet { id: "s".into(), name: "S".into(), pack_settings: Vec::new() });
        let lost: Vec<&str> = cards_outside_sets(&game).iter().map(|card| card.id.as_str()).collect();
        assert_eq!(lost, ["b", "c"]);
        let groups = vec![
            TagGroup { id: "1".into(), name: "Type".into(), tags: vec!["X".into()] },
            TagGroup { id: "2".into(), name: " rarity ".into(), tags: vec!["Common".into(), "Rare".into()] },
        ];
        assert_eq!(rarity_tags(&groups), ["Common", "Rare"]);
        assert!(rarity_tags(&groups[..1]).is_empty());
        let mut cards = vec![card("a", None, None, &["Old|X", "Other|X", "X"])];
        rename_tag_group_in_cards(&mut cards, "Old", "New");
        assert_eq!(cards[0].types, ["New|X", "Other|X", "X"]);
    }

    #[test]
    fn files_round_trip() {
        let root = std::env::temp_dir().join(format!("gametable_gamedef_ops_{}", std::process::id()));
        let root_text = root.to_string_lossy().into_owned();
        let art = root.join("art");
        std::fs::create_dir_all(&art).unwrap();
        for name in ["b.png", "a.png", "a_BACK.png", "notes.txt"] {
            std::fs::write(art.join(name), b"x").unwrap();
        }
        let built = build_cards_from_image_folder(art.to_str().unwrap(), Some("core"), &["a".to_string()]);
        let summary: Vec<(String, String, Option<String>)> =
            built.iter().map(|card| (card.id.clone(), card.card_title.clone(), card.image_path.clone())).collect();
        assert_eq!(
            summary,
            vec![("a_001".into(), "a".into(), Some("a.png".into())), ("b".into(), "b".into(), Some("b.png".into())),]
        );
        assert!(built.iter().all(|card| card.set_id.as_deref() == Some("core")));

        let game = create_game_definition(&root_text, "newgame").unwrap();
        assert_eq!(game.folder_path.as_deref(), Some(join_path(&[&root_text, "newgame"]).as_str()));
        let games = scan_game_library(&root_text).unwrap();
        assert_eq!(games.iter().map(|game| game.id.as_str()).collect::<Vec<_>>(), ["newgame"]);
        let written = std::fs::read_to_string(root.join("newgame").join("gamedef.json")).unwrap();
        assert_eq!(written, "{\n  \"id\": \"newgame\",\n  \"name\": \"newgame\",\n  \"cards\": []\n}");
        assert_eq!(scan_game_library(&join_path(&[&root_text, "missing"])).unwrap(), Vec::new());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
