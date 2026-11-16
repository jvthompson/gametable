//! Loading a saved game into a new session with (possibly different) players.
//! Port of `src/core/save_remap.lua`.

use std::collections::{HashMap, HashSet};

use crate::models::{PlayerInfo, TableState};

/// How saved players line up with the players now at the table.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlayerNameMatch {
    /// Saved player id -> current player id.
    pub current_id_by_saved_id: HashMap<String, String>,
    pub unmatched_saved_players: Vec<PlayerInfo>,
    pub unmatched_current_players: Vec<PlayerInfo>,
}

/// Matches players whose name is exact and unique on both sides.
pub fn match_players_by_name(saved_players: &[PlayerInfo], current_players: &[PlayerInfo]) -> PlayerNameMatch {
    fn count_by_name(players: &[PlayerInfo]) -> HashMap<&str, usize> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for player in players {
            *counts.entry(player.name.as_str()).or_default() += 1;
        }
        counts
    }
    let saved_name_counts = count_by_name(saved_players);
    let current_name_counts = count_by_name(current_players);

    let mut current_id_by_saved_id = HashMap::new();
    let mut claimed_current_ids = HashSet::new();
    for saved in saved_players {
        let name = saved.name.as_str();
        let unique_on_both_sides = saved_name_counts.get(name) == Some(&1) && current_name_counts.get(name) == Some(&1);
        if unique_on_both_sides {
            let current = current_players.iter().find(|player| player.name == name).expect("counted above");
            current_id_by_saved_id.insert(saved.id.clone(), current.id.clone());
            claimed_current_ids.insert(current.id.clone());
        }
    }
    PlayerNameMatch {
        unmatched_saved_players: saved_players
            .iter()
            .filter(|player| !current_id_by_saved_id.contains_key(&player.id))
            .cloned()
            .collect(),
        unmatched_current_players: current_players
            .iter()
            .filter(|player| !claimed_current_ids.contains(&player.id))
            .cloned()
            .collect(),
        current_id_by_saved_id,
    }
}

/// The result of rebuilding a saved table for the current players.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedTableState {
    pub state: TableState,
    /// Cards whose definition no longer exists in the game.
    pub dropped_card_count: usize,
}

/// Rebuilds a saved state for the current players: players are mapped by `current_id_by_saved_id`
/// (unmapped saved players are dropped), cards with unknown definitions are dropped, and links to
/// dropped cards are cleared. The game log is kept; active searches and playmat settings are not.
pub fn build_loaded_table_state(
    saved: &TableState,
    current_players: &[PlayerInfo],
    current_id_by_saved_id: &HashMap<String, String>,
    valid_definition_ids: &HashSet<String>,
) -> LoadedTableState {
    let current_player_by_id: HashMap<&str, &PlayerInfo> =
        current_players.iter().map(|player| (player.id.as_str(), player)).collect();
    let players: Vec<PlayerInfo> = saved
        .players
        .iter()
        .filter_map(|saved_player| current_id_by_saved_id.get(&saved_player.id))
        .filter_map(|current_id| current_player_by_id.get(current_id.as_str()).map(|player| (*player).clone()))
        .collect();

    let remap_player_id = |saved_id: &Option<String>| {
        saved_id.as_ref().and_then(|saved_id| current_id_by_saved_id.get(saved_id).cloned())
    };

    let mut dropped_card_count = 0;
    let mut kept_cards = Vec::new();
    for card in saved.cards() {
        if valid_definition_ids.contains(&card.definition_id) {
            let mut kept = card.clone();
            kept.owner_id = remap_player_id(&card.owner_id);
            kept_cards.push(kept);
        } else {
            dropped_card_count += 1;
        }
    }
    let kept_card_ids: HashSet<String> = kept_cards.iter().map(|card| card.instance_id.clone()).collect();
    for card in &mut kept_cards {
        if card.stack_parent_id.as_ref().is_some_and(|parent_id| !kept_card_ids.contains(parent_id)) {
            card.stack_parent_id = None;
        }
    }

    let widgets = saved
        .widgets()
        .iter()
        .map(|widget| {
            let mut remapped = widget.clone();
            remapped.owner_id = remap_player_id(&widget.owner_id);
            remapped.creator_id = remap_player_id(&widget.creator_id);
            if widget.attached_card_id.as_ref().is_some_and(|card_id| !kept_card_ids.contains(card_id)) {
                remapped.attached_card_id = None;
            }
            remapped
        })
        .collect();

    let mut state = TableState::new(saved.game_id.clone()).with_cards(kept_cards).with_widgets(widgets);
    state.players = players;
    state.revision = saved.revision;
    state.log = saved.log.clone();
    state.loaded_zones = saved.loaded_zones.clone();
    state.loaded_scenario_ids = saved.loaded_scenario_ids.clone();
    state.current_player_id = remap_player_id(&saved.current_player_id);
    LoadedTableState { state, dropped_card_count }
}
