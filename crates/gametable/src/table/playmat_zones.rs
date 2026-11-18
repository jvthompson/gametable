//! Zones placed on playmats (a GameTable addition): which zones sit on a player's or the shared
//! mat right now, and where on the mat each one goes. A placed zone is drawn on the mat instead
//! of in the player area or a sidebar; when the mat is off, or a mat without that zone is
//! chosen, the zone goes back to its usual slot. A player can Ctrl-drag their own mat's zones
//! (the host, the shared mat's) to new spots; those positions replace the designer's.

use std::collections::HashMap;

use gametable_core::models::{
    GameDefinition, PlaymatType, PlaymatZonePosition, TableState, ZoneDefinition, zone_fits_playmat_type,
};

use crate::table::screen::SHARED_OWNER_KEY;
use crate::ui::context::Rect;

/// A zone on a mat, with its centre as fractions of the mat seen the right way up.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedZone {
    pub zone: ZoneDefinition,
    pub fraction_x: f32,
    pub fraction_y: f32,
}

/// The zones on the playmat chosen by `on` / `index` (1-based; 0 means none), in placement
/// order, each where it was moved to on this mat (`moved_positions`) or else where the designer
/// put it. Placements of zones that don't exist right now (a scenario that isn't loaded, a
/// deleted zone) or that don't fit the playmat type are skipped.
pub fn zones_on_playmat<'a>(
    game: &GameDefinition,
    effective_zones: impl Iterator<Item = &'a ZoneDefinition> + Clone,
    playmat_on: bool,
    playmat_index: i64,
    moved_positions: &[PlaymatZonePosition],
) -> Vec<PlacedZone> {
    if !playmat_on {
        return Vec::new();
    }
    let Some(playmat) = usize::try_from(playmat_index - 1).ok().and_then(|index| game.playmats.get(index)) else {
        return Vec::new();
    };
    playmat
        .zone_placements
        .iter()
        .filter_map(|placement| {
            let zone = effective_zones.clone().find(|zone| zone.id == placement.zone_id)?;
            let moved = moved_positions
                .iter()
                .find(|moved| moved.playmat_index == playmat_index && moved.zone_id == placement.zone_id);
            let (x, y) = moved.map_or((placement.x, placement.y), |moved| (moved.x, moved.y));
            zone_fits_playmat_type(zone, game.playmat_type).then(|| PlacedZone {
                zone: zone.clone(),
                fraction_x: x as f32,
                fraction_y: y as f32,
            })
        })
        .collect()
}

/// Every mat's placed zones by owner key: each player's id for individual mats, or
/// [`SHARED_OWNER_KEY`] for the shared mat.
pub fn placed_zones_by_owner(game: &GameDefinition, state: &TableState) -> HashMap<String, Vec<PlacedZone>> {
    let effective_zones = game.zones.iter().chain(state.loaded_zones.iter());
    let mut by_owner = HashMap::new();
    match game.playmat_type {
        PlaymatType::Shared => {
            let zones = zones_on_playmat(
                game,
                effective_zones,
                state.shared_playmat_on,
                state.shared_playmat_index,
                &state.shared_playmat_zone_positions,
            );
            by_owner.insert(SHARED_OWNER_KEY.to_string(), zones);
        }
        PlaymatType::Individual => {
            for player in &state.players {
                let zones = zones_on_playmat(
                    game,
                    effective_zones.clone(),
                    player.individual_playmat_on,
                    player.individual_playmat_index,
                    &player.playmat_zone_positions,
                );
                by_owner.insert(player.id.clone(), zones);
            }
        }
    }
    by_owner
}

/// The fraction of the mat a world point is at, the inverse of [`placed_zone_world_center`],
/// clamped to the mat.
pub fn placed_zone_fraction(mat_world_rect: Rect, rotated: bool, world_x: f32, world_y: f32) -> (f32, f32) {
    let fraction_x = ((world_x - mat_world_rect.x) / mat_world_rect.width).clamp(0.0, 1.0);
    let fraction_y = ((world_y - mat_world_rect.y) / mat_world_rect.height).clamp(0.0, 1.0);
    if rotated { (1.0 - fraction_x, 1.0 - fraction_y) } else { (fraction_x, fraction_y) }
}

/// A placed zone's centre in world units. A mat turned 180° (an opponent's, across the table)
/// turns its zones with it.
pub fn placed_zone_world_center(mat_world_rect: Rect, rotated: bool, fraction_x: f32, fraction_y: f32) -> (f32, f32) {
    let (fraction_x, fraction_y) =
        if rotated { (1.0 - fraction_x, 1.0 - fraction_y) } else { (fraction_x, fraction_y) };
    (mat_world_rect.x + fraction_x * mat_world_rect.width, mat_world_rect.y + fraction_y * mat_world_rect.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gametable_core::models::{PlayerInfo, PlayerRole, PlaymatDefinition, PlaymatZonePlacement};

    fn game(playmat_type: PlaymatType) -> GameDefinition {
        let mut game = GameDefinition::new("game", "Game");
        game.playmat_type = playmat_type;
        game.zones.push(ZoneDefinition::new("deck", "Deck"));
        game.zones.push(ZoneDefinition { shared: true, ..ZoneDefinition::new("market", "Market") });
        let mut mat = PlaymatDefinition::new("mat.png");
        mat.zone_placements = vec![
            PlaymatZonePlacement::new("deck", 0.2, 0.8),
            PlaymatZonePlacement::new("market", 0.5, 0.5),
            PlaymatZonePlacement::new("walls", 0.9, 0.1),
        ];
        game.playmats.push(mat);
        game.playmats.push(PlaymatDefinition::new("plain.png"));
        game
    }

    fn ids(zones: &[PlacedZone]) -> Vec<&str> {
        zones.iter().map(|placed| placed.zone.id.as_str()).collect()
    }

    #[test]
    fn an_individual_mat_holds_only_player_zones() {
        let game = game(PlaymatType::Individual);
        let zones = zones_on_playmat(&game, game.zones.iter(), true, 1, &[]);
        assert_eq!(ids(&zones), ["deck"]);
        assert_eq!((zones[0].fraction_x, zones[0].fraction_y), (0.2, 0.8));
    }

    #[test]
    fn the_shared_mat_holds_only_shared_zones() {
        let game = game(PlaymatType::Shared);
        assert_eq!(ids(&zones_on_playmat(&game, game.zones.iter(), true, 1, &[])), ["market"]);
    }

    #[test]
    fn no_zones_when_the_mat_is_off_or_another_mat_is_chosen() {
        let game = game(PlaymatType::Individual);
        assert!(zones_on_playmat(&game, game.zones.iter(), false, 1, &[]).is_empty());
        assert!(zones_on_playmat(&game, game.zones.iter(), true, 2, &[]).is_empty());
        assert!(zones_on_playmat(&game, game.zones.iter(), true, 0, &[]).is_empty());
        assert!(zones_on_playmat(&game, game.zones.iter(), true, 9, &[]).is_empty());
    }

    #[test]
    fn a_scenario_zone_appears_once_its_scenario_is_loaded() {
        let game = game(PlaymatType::Shared);
        let mut state = TableState::new("game");
        state.shared_playmat_on = true;
        state.shared_playmat_index = 1;
        assert_eq!(ids(&placed_zones_by_owner(&game, &state)[SHARED_OWNER_KEY]), ["market"]);
        state.loaded_zones.push(ZoneDefinition { shared: true, ..ZoneDefinition::new("walls", "Walls") });
        assert_eq!(ids(&placed_zones_by_owner(&game, &state)[SHARED_OWNER_KEY]), ["market", "walls"]);
    }

    #[test]
    fn each_player_gets_the_zones_of_their_own_mat() {
        let game = game(PlaymatType::Individual);
        let mut state = TableState::default();
        let mut with_mat = PlayerInfo::new("p1", "One", PlayerRole::Client, 0xFF00_0000);
        with_mat.individual_playmat_on = true;
        with_mat.individual_playmat_index = 1;
        state.players = vec![with_mat, PlayerInfo::new("p2", "Two", PlayerRole::Client, 0xFF00_0000)];
        let by_owner = placed_zones_by_owner(&game, &state);
        assert_eq!(ids(&by_owner["p1"]), ["deck"]);
        assert!(by_owner["p2"].is_empty());
    }

    #[test]
    fn a_moved_zone_uses_its_new_spot_on_that_mat_only() {
        let game = game(PlaymatType::Individual);
        let moved = [PlaymatZonePosition::new(1, "deck", 0.6, 0.3), PlaymatZonePosition::new(2, "deck", 0.1, 0.1)];
        let zones = zones_on_playmat(&game, game.zones.iter(), true, 1, &moved);
        assert_eq!((zones[0].fraction_x, zones[0].fraction_y), (0.6, 0.3));
    }

    #[test]
    fn fractions_come_back_from_world_points() {
        let mat = Rect::new(100.0, 200.0, 1000.0, 500.0);
        let close =
            |(x, y): (f32, f32), expected: (f32, f32)| (x - expected.0).abs() < 1e-5 && (y - expected.1).abs() < 1e-5;
        assert!(close(placed_zone_fraction(mat, false, 300.0, 600.0), (0.2, 0.8)));
        assert!(close(placed_zone_fraction(mat, true, 900.0, 300.0), (0.2, 0.8)));
        assert_eq!(placed_zone_fraction(mat, false, -50.0, 900.0), (0.0, 1.0));
    }

    #[test]
    fn a_turned_mat_turns_its_zones() {
        let mat = Rect::new(100.0, 200.0, 1000.0, 500.0);
        assert_eq!(placed_zone_world_center(mat, false, 0.2, 0.8), (300.0, 600.0));
        assert_eq!(placed_zone_world_center(mat, true, 0.2, 0.8), (900.0, 300.0));
    }
}
