//! Where a card's, card back's or game's images are on disk. Port of `src/data/image_resolver.lua`.
//!
//! Existence checks run while drawing, so they are remembered; call
//! [`clear_existence_cache`] after the files change (editor saves, Refresh).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use gametable_core::models::{
    CARD_BACK_FILE_SUFFIX, CardBackDefinition, CardDefinition, CardOrientation, GameDefinition, UNIQUE_CARD_BACK_ID,
};

use super::file_system::join_path;

static FILE_EXISTS_CACHE: LazyLock<Mutex<HashMap<String, bool>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn file_exists(path: &str) -> bool {
    let mut cache = FILE_EXISTS_CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *cache.entry(path.to_string()).or_insert_with(|| crate::data::game_files::is_file(path))
}

/// `R.clearCache()`: forget every remembered existence check.
pub fn clear_existence_cache() {
    FILE_EXISTS_CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
}

/// `<folder>/<set id>/<image>`, or `<folder>/<image>` for cards without a set.
pub fn resolve_bare_image_path(folder_path: &str, bare_image_path: &str, set_id: Option<&str>) -> String {
    match set_id {
        Some(set_id) => join_path(&[&join_path(&[folder_path, set_id]), bare_image_path]),
        None => join_path(&[folder_path, bare_image_path]),
    }
}

pub fn resolved_image_path_for_display(
    folder_path: &str,
    bare_image_path: Option<&str>,
    set_id: Option<&str>,
) -> Option<String> {
    bare_image_path.map(|bare| resolve_bare_image_path(folder_path, bare, set_id))
}

/// `goblin.png` -> `goblin_BACK.png`; a path whose file name has no extension (or only a leading
/// dot) gets the suffix appended.
pub fn unique_card_back_path_for(image_path: &str) -> String {
    let last_separator = image_path.rfind(['/', '\\']);
    let last_dot = image_path.rfind('.');
    match last_dot {
        // Lua positions are 1-based: `lastDot <= 1` is a dot at index 0.
        Some(dot) if dot > 0 && last_separator.is_none_or(|separator| dot > separator) => {
            format!("{}{CARD_BACK_FILE_SUFFIX}{}", &image_path[..dot], &image_path[dot..])
        }
        _ => format!("{image_path}{CARD_BACK_FILE_SUFFIX}"),
    }
}

/// The back image a card shows.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedCardBack {
    pub path: Option<String>,
    /// The card wants its own `_BACK` image but it doesn't exist.
    pub missing: bool,
    pub orientation: CardOrientation,
}

/// `R.resolveCardBackImagePath(cardBacks, card, scenarioCardBackId)`. The game's
/// `scenarioCardBackId` is the fallback for a scenario card with no back of its own; an explicit
/// choice (including "unique") always wins. Otherwise the first back is the default.
pub fn resolve_card_back_image_path(
    card_backs: &[CardBackDefinition],
    card: Option<&CardDefinition>,
    scenario_card_back_id: Option<&str>,
) -> ResolvedCardBack {
    let card_back_id = card.and_then(|card| card.card_back_id.as_deref());
    if card_back_id == Some(UNIQUE_CARD_BACK_ID)
        && let Some(card) = card
    {
        let orientation = card.unique_back_orientation;
        let Some(image_path) = &card.image_path else {
            return ResolvedCardBack { path: None, missing: true, orientation };
        };
        let candidate = unique_card_back_path_for(image_path);
        if file_exists(&candidate) {
            return ResolvedCardBack { path: Some(candidate), missing: false, orientation };
        }
        return ResolvedCardBack { path: None, missing: true, orientation };
    }
    let find_back = |id: &str| card_backs.iter().find(|back| back.id == id);
    let chosen = match (card_back_id, card) {
        (Some(id), _) => find_back(id),
        (None, Some(card)) if card.is_scenario_card => scenario_card_back_id.and_then(find_back),
        _ => None,
    };
    match chosen.or(card_backs.first()) {
        Some(back) => ResolvedCardBack { path: back.image_path.clone(), missing: false, orientation: back.orientation },
        None => ResolvedCardBack { path: None, missing: false, orientation: CardOrientation::Portrait },
    }
}

/// The pack generator widget's art, when the game folder has one.
pub const PACK_GENERATOR_IMAGE_FILE_NAME: &str = "packgen.png";

pub fn resolve_pack_generator_image_path(folder_path: Option<&str>) -> Option<String> {
    let candidate = join_path(&[folder_path?, PACK_GENERATOR_IMAGE_FILE_NAME]);
    file_exists(&candidate).then_some(candidate)
}

const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "webp", "gif", "bmp"];

/// Probes `<folder>/<base name>.<ext>` for each known extension, in order.
pub fn resolve_image_any_extension(folder_path: Option<&str>, base_name: &str) -> Option<String> {
    let folder_path = folder_path?;
    IMAGE_EXTENSIONS
        .iter()
        .map(|extension| join_path(&[folder_path, &format!("{base_name}.{extension}")]))
        .find(|candidate| file_exists(candidate))
}

/// The game's logo (`gamelogo.<ext>` in its folder).
pub fn resolve_game_logo_image_path(folder_path: Option<&str>) -> Option<String> {
    resolve_image_any_extension(folder_path, "gamelogo")
}

/// Client side: keeps the host's card/zone/tag/set data but uses this machine's own image paths
/// (none where the client lacks the card), card backs and folder.
///
/// Like the original this rebuilds the game from a fixed field list, so the table, playmat
/// border and current-turn colours are not carried over.
pub fn merge_local_image_paths(remote: &GameDefinition, local: &GameDefinition) -> GameDefinition {
    let local_image_by_id: HashMap<&str, Option<&String>> =
        local.cards().iter().map(|card| (card.id.as_str(), card.image_path.as_ref())).collect();
    let cards = remote
        .cards()
        .iter()
        .map(|card| CardDefinition {
            image_path: local_image_by_id.get(card.id.as_str()).copied().flatten().cloned(),
            ..card.clone()
        })
        .collect();
    let mut merged = GameDefinition::new(remote.id.clone(), remote.name.clone()).with_cards(cards);
    merged.card_backs = local.card_backs.clone();
    merged.zones = remote.zones.clone();
    merged.tag_groups = remote.tag_groups.clone();
    merged.sets = remote.sets.clone();
    merged.folder_path = local.folder_path.clone();
    merged.has_scenarios = remote.has_scenarios;
    merged.scenarios = remote.scenarios.clone();
    merged.scenario_card_back_id = remote.scenario_card_back_id.clone();
    merged.uses_tokens = remote.uses_tokens;
    merged.tokens = remote.tokens.clone();
    merged.game_boards = remote.game_boards.clone();
    merged.same_orientation_for_all = remote.same_orientation_for_all;
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_back_paths() {
        assert_eq!(unique_card_back_path_for("goblin.png"), "goblin_BACK.png");
        assert_eq!(unique_card_back_path_for("C:\\a.b\\goblin"), "C:\\a.b\\goblin_BACK");
        assert_eq!(unique_card_back_path_for(".png"), ".png_BACK");
        assert_eq!(unique_card_back_path_for("x/y.z.png"), "x/y.z_BACK.png");
    }

    #[test]
    fn card_back_choice() {
        let back = |id: &str| CardBackDefinition {
            id: id.to_string(),
            name: id.to_string(),
            image_path: Some(format!("{id}.png")),
            orientation: CardOrientation::Portrait,
        };
        let backs = [back("default"), back("scenario"), back("other")];
        let mut card = CardDefinition::new("c", "Card");
        let path_of = |card: Option<&CardDefinition>, scenario: Option<&str>| {
            resolve_card_back_image_path(&backs, card, scenario).path
        };
        assert_eq!(path_of(Some(&card), Some("scenario")).as_deref(), Some("default.png"));
        card.is_scenario_card = true;
        assert_eq!(path_of(Some(&card), Some("scenario")).as_deref(), Some("scenario.png"));
        card.card_back_id = Some("other".to_string());
        assert_eq!(path_of(Some(&card), Some("scenario")).as_deref(), Some("other.png"));
        card.card_back_id = Some("gone".to_string());
        assert_eq!(path_of(Some(&card), None).as_deref(), Some("default.png"));
        assert_eq!(path_of(None, None).as_deref(), Some("default.png"));
        card.card_back_id = Some(UNIQUE_CARD_BACK_ID.to_string());
        let unique = resolve_card_back_image_path(&backs, Some(&card), None);
        assert_eq!((unique.path, unique.missing), (None, true));
        assert_eq!(resolve_card_back_image_path(&[], None, None).path, None);
    }

    #[test]
    #[cfg(windows)]
    fn bare_paths_join_the_set_folder() {
        assert_eq!(resolve_bare_image_path("C:\\g", "a.png", Some("set1")), "C:\\g\\set1\\a.png");
        assert_eq!(resolve_bare_image_path("C:\\g", "a.png", None), "C:\\g\\a.png");
    }
}
