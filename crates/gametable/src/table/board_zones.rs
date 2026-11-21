//! Zones placed on game boards (a GameTable addition): which zones sit on the boards shown right
//! now, and where on each board. A placed zone is drawn on its board instead of in the player
//! area or a sidebar; when no shown board holds it (the board is hidden, or a player picked a
//! board without it), the zone goes back to its usual slot. A zone placed on several shown boards
//! of one owner sits on the first of them. A player can Ctrl-drag the zones on their own player
//! boards (the host, on shared boards) to new spots; those positions replace the designer's.

use std::collections::HashMap;

use gametable_core::models::{BoardZonePosition, GameDefinition, TableState, ZoneDefinition, zone_fits_board_kind};

use crate::table::board_layout::{SHARED_OWNER_KEY, board_at, shown_boards};
use crate::ui::context::Rect;

/// A zone on a board, with its centre as fractions of the board seen the right way up.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedZone {
    pub zone: ZoneDefinition,
    /// 1-based, into the game's boards.
    pub board_index: i64,
    pub fraction_x: f32,
    pub fraction_y: f32,
}

/// The zones on one board (1-based `board_index`), in placement order, each where it was moved
/// to on this board (`moved_positions`) or else where the designer put it. Placements of zones
/// that don't exist right now (a scenario that isn't loaded, a deleted zone) or that don't fit
/// the board's kind are skipped.
pub fn zones_on_board<'a>(
    game: &GameDefinition,
    effective_zones: impl Iterator<Item = &'a ZoneDefinition> + Clone,
    board_index: i64,
    moved_positions: &[BoardZonePosition],
) -> Vec<PlacedZone> {
    let Some(board) = board_at(game, board_index) else { return Vec::new() };
    board
        .zone_placements
        .iter()
        .filter_map(|placement| {
            let zone = effective_zones.clone().find(|zone| zone.id == placement.zone_id)?;
            let moved = moved_positions
                .iter()
                .find(|moved| moved.board_index == board_index && moved.zone_id == placement.zone_id);
            let (x, y) = moved.map_or((placement.x, placement.y), |moved| (moved.x, moved.y));
            zone_fits_board_kind(zone, board.kind).then(|| PlacedZone {
                zone: zone.clone(),
                board_index,
                fraction_x: x as f32,
                fraction_y: y as f32,
            })
        })
        .collect()
}

/// A loaded scenario's zone that the scenario sets at a fixed spot on the table, centred on table
/// fractions as the first seat sees them. Nobody can move it during the game.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneOnTable {
    pub zone: ZoneDefinition,
    pub fraction_x: f64,
    pub fraction_y: f64,
}

/// The zones the loaded scenarios set on the table, each once (the first scenario to set it wins).
/// A zone the session didn't take from that scenario (its id was already in use) is skipped.
pub fn scenario_zones_on_table(game: &GameDefinition, state: &TableState) -> Vec<ZoneOnTable> {
    let mut zones_on_table: Vec<ZoneOnTable> = Vec::new();
    for scenario_id in &state.loaded_scenario_ids {
        let Some(scenario) = game.scenario_by_id(scenario_id) else { continue };
        for placement in &scenario.zone_placements {
            let already_set = zones_on_table.iter().any(|on_table| on_table.zone.id == placement.zone_id);
            let is_this_scenarios_zone = scenario.zones.iter().any(|zone| zone.id == placement.zone_id);
            let loaded_zone = state.loaded_zones.iter().find(|zone| zone.id == placement.zone_id);
            if let Some(zone) = loaded_zone.filter(|_| !already_set && is_this_scenarios_zone) {
                zones_on_table.push(ZoneOnTable {
                    zone: zone.clone(),
                    fraction_x: placement.x,
                    fraction_y: placement.y,
                });
            }
        }
    }
    zones_on_table
}

/// The zones on every shown board by owner key: each player's id for their player boards, or
/// [`SHARED_OWNER_KEY`] for shared boards. Each zone appears at most once per owner.
pub fn placed_zones_by_owner(game: &GameDefinition, state: &TableState) -> HashMap<String, Vec<PlacedZone>> {
    let effective_zones = game.zones.iter().chain(state.loaded_zones.iter());
    let mut by_owner: HashMap<String, Vec<PlacedZone>> = HashMap::new();
    for (owner_key, board_index) in shown_boards(game, state) {
        let moved_positions = if owner_key == SHARED_OWNER_KEY {
            state.shared_board_zone_positions.as_slice()
        } else {
            state
                .players
                .iter()
                .find(|player| player.id == owner_key)
                .map_or(&[][..], |player| player.board_zone_positions.as_slice())
        };
        let zones = zones_on_board(game, effective_zones.clone(), board_index, moved_positions);
        let owned = by_owner.entry(owner_key).or_default();
        for placed in zones {
            if !owned.iter().any(|already| already.zone.id == placed.zone.id) {
                owned.push(placed);
            }
        }
    }
    by_owner
}

/// Where an upright board's fraction lands inside its bounds once turned `rotation` degrees
/// clockwise.
fn turned_fraction(rotation: i64, fraction_x: f32, fraction_y: f32) -> (f32, f32) {
    match rotation.rem_euclid(360) {
        90 => (1.0 - fraction_y, fraction_x),
        180 => (1.0 - fraction_x, 1.0 - fraction_y),
        270 => (fraction_y, 1.0 - fraction_x),
        _ => (fraction_x, fraction_y),
    }
}

/// The inverse of [`turned_fraction`].
fn unturned_fraction(rotation: i64, bounds_x: f32, bounds_y: f32) -> (f32, f32) {
    match rotation.rem_euclid(360) {
        90 => (bounds_y, 1.0 - bounds_x),
        180 => (1.0 - bounds_x, 1.0 - bounds_y),
        270 => (1.0 - bounds_y, bounds_x),
        _ => (bounds_x, bounds_y),
    }
}

/// The fraction of the board a world point is at, the inverse of [`placed_zone_world_center`],
/// clamped to the board. `board_world_rect` is the board's bounds after turning.
pub fn placed_zone_fraction(board_world_rect: Rect, rotation: i64, world_x: f32, world_y: f32) -> (f32, f32) {
    let bounds_x = ((world_x - board_world_rect.x) / board_world_rect.width).clamp(0.0, 1.0);
    let bounds_y = ((world_y - board_world_rect.y) / board_world_rect.height).clamp(0.0, 1.0);
    unturned_fraction(rotation, bounds_x, bounds_y)
}

/// A placed zone's centre in world units. A turned board (an opponent's across the table, or one
/// the designer turned) turns its zones' positions with it.
pub fn placed_zone_world_center(board_world_rect: Rect, rotation: i64, fraction_x: f32, fraction_y: f32) -> (f32, f32) {
    let (bounds_x, bounds_y) = turned_fraction(rotation, fraction_x, fraction_y);
    (board_world_rect.x + bounds_x * board_world_rect.width, board_world_rect.y + bounds_y * board_world_rect.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gametable_core::models::{
        BoardKind, BoardTablePlacement, BoardZonePlacement, GameBoardDefinition, PlayerInfo, PlayerRole,
    };

    /// Board 1 holds a player zone, a shared zone and a scenario zone; board 2 is plain.
    fn game(kind: BoardKind) -> GameDefinition {
        let mut game = GameDefinition::new("game", "Game");
        game.zones.push(ZoneDefinition::new("deck", "Deck"));
        game.zones.push(ZoneDefinition { shared: true, ..ZoneDefinition::new("market", "Market") });
        let mut board = GameBoardDefinition { kind, ..GameBoardDefinition::new("mat.png") };
        board.zone_placements = vec![
            BoardZonePlacement::new("deck", 0.2, 0.8),
            BoardZonePlacement::new("market", 0.5, 0.5),
            BoardZonePlacement::new("walls", 0.9, 0.1),
        ];
        game.game_boards.push(board);
        game.game_boards.push(GameBoardDefinition { kind, ..GameBoardDefinition::new("plain.png") });
        game
    }

    fn ids(zones: &[PlacedZone]) -> Vec<&str> {
        zones.iter().map(|placed| placed.zone.id.as_str()).collect()
    }

    #[test]
    fn a_player_board_holds_only_player_zones() {
        let game = game(BoardKind::Player);
        let zones = zones_on_board(&game, game.zones.iter(), 1, &[]);
        assert_eq!(ids(&zones), ["deck"]);
        assert_eq!((zones[0].fraction_x, zones[0].fraction_y, zones[0].board_index), (0.2, 0.8, 1));
    }

    #[test]
    fn a_shared_board_holds_only_shared_zones() {
        let game = game(BoardKind::Shared);
        assert_eq!(ids(&zones_on_board(&game, game.zones.iter(), 1, &[])), ["market"]);
    }

    #[test]
    fn no_zones_on_a_board_without_placements_or_that_does_not_exist() {
        let game = game(BoardKind::Player);
        assert!(zones_on_board(&game, game.zones.iter(), 2, &[]).is_empty());
        assert!(zones_on_board(&game, game.zones.iter(), 0, &[]).is_empty());
        assert!(zones_on_board(&game, game.zones.iter(), 9, &[]).is_empty());
    }

    #[test]
    fn a_scenario_zone_appears_once_its_scenario_is_loaded() {
        let game = game(BoardKind::Shared);
        let mut state = TableState::new("game");
        state.shared_boards_shown = true;
        state.chosen_shared_board = 1;
        assert_eq!(ids(&placed_zones_by_owner(&game, &state)[SHARED_OWNER_KEY]), ["market"]);
        state.loaded_zones.push(ZoneDefinition { shared: true, ..ZoneDefinition::new("walls", "Walls") });
        assert_eq!(ids(&placed_zones_by_owner(&game, &state)[SHARED_OWNER_KEY]), ["market", "walls"]);
    }

    #[test]
    fn each_player_gets_the_zones_of_their_own_shown_board() {
        let game = game(BoardKind::Player);
        let mut state = TableState::default();
        let mut with_board = PlayerInfo::new("p1", "One", PlayerRole::Client, 0xFF00_0000);
        with_board.player_boards_shown = true;
        with_board.chosen_player_board = 1;
        let mut other_board = PlayerInfo::new("p2", "Two", PlayerRole::Client, 0xFF00_0000);
        other_board.player_boards_shown = true;
        other_board.chosen_player_board = 2;
        state.players = vec![with_board, other_board, PlayerInfo::new("p3", "Three", PlayerRole::Client, 0)];
        let by_owner = placed_zones_by_owner(&game, &state);
        assert_eq!(ids(&by_owner["p1"]), ["deck"]);
        assert!(by_owner["p2"].is_empty());
        assert!(!by_owner.contains_key("p3"));
    }

    #[test]
    fn a_zone_on_two_shown_boards_sits_on_the_first() {
        let mut game = game(BoardKind::Player);
        game.game_boards[1].zone_placements = vec![BoardZonePlacement::new("deck", 0.5, 0.5)];
        for board in &mut game.game_boards {
            board.table_placement = Some(BoardTablePlacement::new(0.0, -200.0));
        }
        let mut state = TableState::default();
        state.players = vec![PlayerInfo::new("p1", "One", PlayerRole::Client, 0)];
        let zones = &placed_zones_by_owner(&game, &state)["p1"];
        assert_eq!(ids(zones), ["deck"]);
        assert_eq!(zones[0].board_index, 1);
    }

    #[test]
    fn a_moved_zone_uses_its_new_spot_on_that_board_only() {
        let game = game(BoardKind::Player);
        let moved = [BoardZonePosition::new(1, "deck", 0.6, 0.3), BoardZonePosition::new(2, "deck", 0.1, 0.1)];
        let zones = zones_on_board(&game, game.zones.iter(), 1, &moved);
        assert_eq!((zones[0].fraction_x, zones[0].fraction_y), (0.6, 0.3));
    }

    #[test]
    fn fractions_come_back_from_world_points_at_every_turn() {
        let board = Rect::new(100.0, 200.0, 1000.0, 500.0);
        let close =
            |(x, y): (f32, f32), expected: (f32, f32)| (x - expected.0).abs() < 1e-5 && (y - expected.1).abs() < 1e-5;
        for rotation in [0, 90, 180, 270] {
            let (world_x, world_y) = placed_zone_world_center(board, rotation, 0.2, 0.8);
            assert!(close(placed_zone_fraction(board, rotation, world_x, world_y), (0.2, 0.8)), "{rotation}");
        }
        assert_eq!(placed_zone_fraction(board, 0, -50.0, 900.0), (0.0, 1.0));
    }

    #[test]
    fn a_turned_board_turns_its_zones() {
        let board = Rect::new(100.0, 200.0, 1000.0, 500.0);
        assert_eq!(placed_zone_world_center(board, 0, 0.2, 0.8), (300.0, 600.0));
        assert_eq!(placed_zone_world_center(board, 180, 0.2, 0.8), (900.0, 300.0));
        // A quarter turn clockwise: the board's top-left corner goes to the top-right.
        assert_eq!(placed_zone_world_center(board, 90, 0.0, 0.0), (1100.0, 200.0));
        assert_eq!(placed_zone_world_center(board, 270, 0.0, 0.0), (100.0, 700.0));
    }
}
