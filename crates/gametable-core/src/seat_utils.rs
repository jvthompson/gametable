//! Seat arrangement around the table. Port of `src/core/seat_utils.lua`. Seats are 0-based.

use crate::models::PlayerInfo;

/// Whether a seat is on the far side of the table (and therefore sees it mirrored).
pub fn is_far_mirror_seat(seat_index: usize, player_count: usize) -> bool {
    match player_count {
        2 => seat_index == 1,
        3 => seat_index >= 1,
        4 => seat_index >= 2,
        _ => seat_index >= player_count.div_ceil(2),
    }
}

/// Which players' hands are drawn along the bottom and top edges for one viewer.
#[derive(Debug, Clone, PartialEq)]
pub struct HandRowLayout<'players> {
    pub bottom_row: Vec<&'players PlayerInfo>,
    pub top_row: Vec<&'players PlayerInfo>,
}

pub fn compute_hand_row_layout<'players>(
    players: &'players [PlayerInfo],
    local_player_id: &str,
) -> HandRowLayout<'players> {
    let player_count = players.len();
    if player_count <= 1 {
        return HandRowLayout { bottom_row: players.iter().collect(), top_row: Vec::new() };
    }
    let (far_side, near_side): (Vec<(usize, &PlayerInfo)>, Vec<(usize, &PlayerInfo)>) =
        players.iter().enumerate().partition(|(seat, _)| is_far_mirror_seat(*seat, player_count));
    let far: Vec<&PlayerInfo> = far_side.into_iter().map(|(_, player)| player).collect();
    let near: Vec<&PlayerInfo> = near_side.into_iter().map(|(_, player)| player).collect();

    let local_seat = players.iter().position(|player| player.id == local_player_id);
    // Lua uses seat -1 for an unknown viewer, which is never a far seat.
    let viewer_is_far = local_seat.is_some_and(|seat| is_far_mirror_seat(seat, player_count));
    if viewer_is_far {
        HandRowLayout { bottom_row: far.into_iter().rev().collect(), top_row: near.into_iter().rev().collect() }
    } else {
        HandRowLayout { bottom_row: near, top_row: far }
    }
}

/// Whether this viewer sees the shared table mirrored on both axes.
pub fn is_viewer_mirrored(players: &[PlayerInfo], local_player_id: &str) -> bool {
    let player_count = players.len();
    if player_count <= 1 {
        return false;
    }
    players
        .iter()
        .position(|player| player.id == local_player_id)
        .is_some_and(|seat| is_far_mirror_seat(seat, player_count))
}
