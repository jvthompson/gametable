//! Game boards on the table (a GameTable addition; the original's playmats): which boards are
//! shown right now and where each one goes in a viewer's world.
//!
//! A game's boards are player boards (every player gets their own copy) or shared boards. Boards
//! the designer placed on the table are always there when the game loads, except toggleable ones,
//! which F12 shows and hides. A placed shared board sits at its spot on the table; a player board
//! placed for every seat sits at its offset from each player's seat origin (where their seat meets
//! the table's middle line, which is also where the original's mats start), so players across the
//! table get theirs turned around. (The table's outer edges sit under the hand areas or off
//! screen, depending on the window, so they make no anchor.) A player board can also be placed
//! for single players (by seat number), each at its own spot, which replaces that player's
//! every-seat copy. A game with nothing placed works as the original's playmats did: F12 shows
//! each player's chosen player board in rows either side of the middle, or (with only shared
//! boards) the one shared board, centred.
//!
//! During the game a player can move and turn their own player boards (see `MovedBoard`); a moved
//! board sits where they left it, whatever the game would have done.
//!
//! Everything here is plain geometry over world units, shared by the table and the game editor's
//! Table Preview so the two always agree.

use std::collections::HashSet;

use gametable_core::geometry::{WORLD_HEIGHT, WORLD_WIDTH};
use gametable_core::models::{
    BoardKind, BoardPlayerPlacement, BoardTablePlacement, GameBoardDefinition, GameDefinition, MovedBoard, PlayerInfo,
    TableState,
};

use crate::table::card_render::CARD_WIDTH;
use crate::ui::context::Rect;

/// The key for boards (and their zones) that belong to no player.
pub const SHARED_OWNER_KEY: &str = "__shared";

/// A standard card is 2.5in wide, so this many world units make an inch.
pub const PIXELS_PER_INCH: f32 = CARD_WIDTH / 2.5;
/// A player board's size while its image loads (a 24 x 12in playmat).
pub const PLAYER_BOARD_FALLBACK_SIZE: (f32, f32) = (24.0 * PIXELS_PER_INCH, 12.0 * PIXELS_PER_INCH);
/// A shared board's size while its image loads.
pub const SHARED_BOARD_FALLBACK_SIZE: (f32, f32) = (1280.0, 720.0);
/// The gap between neighbouring seats' boards.
pub const SEAT_GAP: f32 = 2.0 * CARD_WIDTH;

const WORLD_W: f32 = WORLD_WIDTH as f32;
const WORLD_H: f32 = WORLD_HEIGHT as f32;

/// One board on the table this frame, in a viewer's world.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardOnTable {
    /// The owning player's id, or [`SHARED_OWNER_KEY`].
    pub owner_key: String,
    /// 1-based, into the game's boards.
    pub board_index: i64,
    /// The board's bounds after turning.
    pub world_rect: Rect,
    /// Degrees clockwise (0, 90, 180 or 270) the board is turned as this viewer sees it.
    pub rotation: i64,
}

impl BoardOnTable {
    /// Zones on a board turned upside down (an opponent's, across the table) are drawn turned
    /// too; on a sideways board they stay upright so their cards read.
    pub fn turns_its_zones(&self) -> bool {
        self.rotation == 180
    }
}

/// The board at a 1-based index.
pub fn board_at(game: &GameDefinition, board_index: i64) -> Option<&GameBoardDefinition> {
    usize::try_from(board_index - 1).ok().and_then(|index| game.game_boards.get(index))
}

/// Whether the designer put any board on the table.
pub fn has_placed_boards(game: &GameDefinition) -> bool {
    game.game_boards.iter().any(GameBoardDefinition::is_on_table)
}

/// A player's seat number (1 for the first seat), as player placements name them.
pub fn seat_number(state: &TableState, player_id: &str) -> Option<i64> {
    state.players.iter().position(|player| player.id == player_id).map(|seat| seat as i64 + 1)
}

/// The chosen board when it is of `kind`, else the game's first board of that kind.
fn chosen_or_first(game: &GameDefinition, chosen: i64, kind: BoardKind) -> Option<i64> {
    if board_at(game, chosen).is_some_and(|board| board.kind == kind) {
        return Some(chosen);
    }
    game.game_boards.iter().position(|board| board.kind == kind).map(|index| index as i64 + 1)
}

/// The player board someone uses while no board is placed: the one they picked from the table
/// menu, or the first.
pub fn default_player_board(game: &GameDefinition, chosen: i64) -> Option<i64> {
    chosen_or_first(game, chosen, BoardKind::Player)
}

/// The shared board shown in a game with only shared boards and none placed.
pub fn default_shared_board(game: &GameDefinition, chosen: i64) -> Option<i64> {
    chosen_or_first(game, chosen, BoardKind::Shared)
}

/// Whether players pick their player board from the table menu: only while none is placed.
pub fn players_choose_their_board(game: &GameDefinition) -> bool {
    !has_placed_boards(game) && game.game_boards.iter().any(|board| board.kind == BoardKind::Player)
}

/// What F12 shows and hides in a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardToggle {
    /// Nothing: no boards, or every placed board is always shown.
    Nothing,
    /// The placed boards marked "Can be toggled": each player's own player boards, and (for the
    /// host) the shared ones.
    PlacedBoards { player_boards: bool, shared_boards: bool },
    /// Nothing placed: each player's chosen player board.
    DefaultPlayerBoard,
    /// Nothing placed and only shared boards: the host's chosen shared board.
    DefaultSharedBoard,
}

pub fn board_toggle(game: &GameDefinition) -> BoardToggle {
    if has_placed_boards(game) {
        let toggleable_of =
            |kind: BoardKind| game.game_boards.iter().any(|board| board.kind == kind && board.is_toggleable_on_table());
        let (player_boards, shared_boards) = (toggleable_of(BoardKind::Player), toggleable_of(BoardKind::Shared));
        if player_boards || shared_boards {
            return BoardToggle::PlacedBoards { player_boards, shared_boards };
        }
        return BoardToggle::Nothing;
    }
    if game.game_boards.iter().any(|board| board.kind == BoardKind::Player) {
        BoardToggle::DefaultPlayerBoard
    } else if game.game_boards.iter().any(|board| board.kind == BoardKind::Shared) {
        BoardToggle::DefaultSharedBoard
    } else {
        BoardToggle::Nothing
    }
}

/// Every board shown right now as (owner key, 1-based board index), in board order. A placed
/// board is shown unless it is toggleable and its owner (the host, for shared boards) hid it.
pub fn shown_boards(game: &GameDefinition, state: &TableState) -> Vec<(String, i64)> {
    let mut shown = Vec::new();
    if has_placed_boards(game) {
        for (index, board) in game.game_boards.iter().enumerate() {
            if let (Some(placement), BoardKind::Shared) = (&board.table_placement, board.kind)
                && (!placement.toggleable || state.shared_boards_shown)
            {
                shown.push((SHARED_OWNER_KEY.to_string(), index as i64 + 1));
            }
        }
        for (index, board) in game.game_boards.iter().enumerate() {
            for (seat, player) in state.players.iter().enumerate() {
                let Some(toggleable) = board.toggleable_for_player(seat as i64 + 1) else { continue };
                if !toggleable || player.player_boards_shown {
                    shown.push((player.id.clone(), index as i64 + 1));
                }
            }
        }
        return shown;
    }
    match board_toggle(game) {
        BoardToggle::DefaultPlayerBoard => {
            for player in state.players.iter().filter(|player| player.player_boards_shown) {
                if let Some(board_index) = default_player_board(game, player.chosen_player_board) {
                    shown.push((player.id.clone(), board_index));
                }
            }
        }
        BoardToggle::DefaultSharedBoard if state.shared_boards_shown => {
            if let Some(board_index) = default_shared_board(game, state.chosen_shared_board) {
                shown.push((SHARED_OWNER_KEY.to_string(), board_index));
            }
        }
        _ => {}
    }
    shown
}

/// A board's size on the table before turning: its image's pixel size (`image_size`, `None` while
/// it loads) times its scale.
pub fn board_size(board: &GameBoardDefinition, image_size: Option<(f32, f32)>) -> (f32, f32) {
    let fallback = match board.kind {
        BoardKind::Player => PLAYER_BOARD_FALLBACK_SIZE,
        BoardKind::Shared => SHARED_BOARD_FALLBACK_SIZE,
    };
    let (width, height) = image_size.unwrap_or(fallback);
    let scale = board.scale as f32;
    (width * scale, height * scale)
}

/// A size after turning `rotation` degrees: a quarter turn swaps width and height.
pub fn turned_size((width, height): (f32, f32), rotation: i64) -> (f32, f32) {
    if rotation.rem_euclid(180) == 90 { (height, width) } else { (width, height) }
}

pub fn rect_around((center_x, center_y): (f32, f32), (width, height): (f32, f32)) -> Rect {
    Rect::new(center_x - width / 2.0, center_y - height / 2.0, width, height)
}

/// `rowLefts(widths, gap)`: left edges of boards placed edge to edge outward from the table's
/// vertical centre line (no gap at the centre split, `gap` between other neighbours).
pub fn row_lefts(widths: &[f32], gap: f32) -> Vec<f32> {
    let count = widths.len();
    let mut lefts = vec![0.0; count];
    if count == 0 {
        return lefts;
    }
    if count % 2 == 1 {
        let middle = count / 2;
        lefts[middle] = WORLD_W / 2.0 - widths[middle] / 2.0;
        let mut x = lefts[middle];
        for index in (0..middle).rev() {
            x = x - gap - widths[index];
            lefts[index] = x;
        }
        x = lefts[middle] + widths[middle];
        for index in middle + 1..count {
            x += gap;
            lefts[index] = x;
            x += widths[index];
        }
    } else {
        let half = count / 2;
        let mut x = WORLD_W / 2.0;
        for index in (0..half).rev() {
            x -= widths[index];
            lefts[index] = x;
            x -= gap;
        }
        x = WORLD_W / 2.0;
        for index in half..count {
            lefts[index] = x;
            x += widths[index] + gap;
        }
    }
    lefts
}

/// The seat origins' x for a row of `count` seats: the row is split evenly across the table, as
/// the players' hand areas are, and each seat's origin is the middle of its share. So every seat's
/// copy of a player board sits at the same offset from its own seat, wherever the designer put it.
pub fn seat_origin_xs(count: usize) -> Vec<f32> {
    let count = count.max(1);
    (0..count).map(|seat| WORLD_W * (seat as f32 + 0.5) / count as f32).collect()
}

/// Whether the seat at `origin_x` (on the far side when `far_side`) is in the right half of the
/// table as its player sees it, where the player-board layout is mirrored left to right. A seat in
/// the middle isn't.
pub fn seat_mirrors_its_boards(origin_x: f32, far_side: bool) -> bool {
    let middle = WORLD_W / 2.0;
    if far_side { origin_x < middle - 0.5 } else { origin_x > middle + 0.5 }
}

/// A placed player board for the seat whose origin is at `origin_x` on the middle line, on the
/// near side (offsets growing toward the table's bottom) or turned around on the far side:
/// (bounds, rotation).
pub fn player_board_on_table(
    origin_x: f32,
    far_side: bool,
    placement: &BoardTablePlacement,
    size: (f32, f32),
) -> (Rect, i64) {
    // Seats in the right half of the table (as their player sees it) get the layout mirrored, so
    // the boards sit symmetrically around the table: a board placed toward the middle from the
    // viewer's seat is toward the middle from every seat.
    let mirrored = seat_mirrors_its_boards(origin_x, far_side);
    let offset_x = if mirrored { -placement.x as f32 } else { placement.x as f32 };
    let offset_y = placement.y as f32;
    let placed_rotation = if mirrored { (360 - placement.rotation).rem_euclid(360) } else { placement.rotation };
    let (center, rotation) = if far_side {
        ((origin_x - offset_x, WORLD_H / 2.0 - offset_y), placed_rotation + 180)
    } else {
        ((origin_x + offset_x, WORLD_H / 2.0 + offset_y), placed_rotation)
    };
    let rotation = rotation.rem_euclid(360);
    (rect_around(center, turned_size(size, rotation)), rotation)
}

/// Where a placed shared board's centre is on the table (as the host sees it): the middle of the
/// table while it is locked there, else its own spot.
pub fn shared_board_center(placement: &BoardTablePlacement) -> (f32, f32) {
    if placement.unlocked_from_center {
        (placement.x as f32, placement.y as f32)
    } else {
        (WORLD_W / 2.0, WORLD_H / 2.0)
    }
}

/// A board at a spot on the table as the first seat sees it (centre in world units, quarter
/// turns), in a viewer's world: turned around for a mirrored viewer.
pub fn board_at_table_spot((x, y): (f32, f32), rotation: i64, size: (f32, f32), is_mirrored: bool) -> (Rect, i64) {
    let (center, rotation) =
        if is_mirrored { ((WORLD_W - x, WORLD_H - y), rotation + 180) } else { ((x, y), rotation) };
    let rotation = rotation.rem_euclid(360);
    (rect_around(center, turned_size(size, rotation)), rotation)
}

/// A placed shared board in a viewer's world (turned around for a mirrored viewer).
pub fn shared_board_on_table(placement: &BoardTablePlacement, size: (f32, f32), is_mirrored: bool) -> (Rect, i64) {
    board_at_table_spot(shared_board_center(placement), placement.rotation, size, is_mirrored)
}

/// A player board placed for one player, in a viewer's world.
pub fn player_placement_on_table(placement: &BoardPlayerPlacement, size: (f32, f32), is_mirrored: bool) -> (Rect, i64) {
    board_at_table_spot((placement.x as f32, placement.y as f32), placement.rotation, size, is_mirrored)
}

/// A board its player moved during the game, in a viewer's world.
pub fn moved_board_on_table(moved: &MovedBoard, size: (f32, f32), is_mirrored: bool) -> (Rect, i64) {
    let center = (moved.x as f32 * WORLD_W, moved.y as f32 * WORLD_H);
    board_at_table_spot(center, moved.rotation, size, is_mirrored)
}

/// Where a board shown at `world_rect`, turned `rotation`, in a viewer's world is on the table as
/// the first seat sees it: (centre as table fractions, quarter turns) — what a [`MovedBoard`]
/// stores.
pub fn table_spot_of(world_rect: Rect, rotation: i64, is_mirrored: bool) -> ((f64, f64), i64) {
    let (center_x, center_y) = (world_rect.x + world_rect.width / 2.0, world_rect.y + world_rect.height / 2.0);
    let (x, y, rotation) = if is_mirrored {
        (WORLD_W - center_x, WORLD_H - center_y, rotation + 180)
    } else {
        (center_x, center_y, rotation)
    };
    ((f64::from(x / WORLD_W), f64::from(y / WORLD_H)), rotation.rem_euclid(360))
}

/// Where a player's board goes when they moved it during the game.
fn moved_board_of(player: &PlayerInfo, board_index: i64) -> Option<&MovedBoard> {
    player.moved_boards.iter().find(|moved| moved.board_index == board_index)
}

/// The rows of seats boards are laid out in: (players, far side). With players on both sides the
/// top row is across the table; a lone row sits on the near side.
fn seat_rows<'a>(top_row: &'a [PlayerInfo], bottom_row: &'a [PlayerInfo]) -> Vec<(&'a [PlayerInfo], bool)> {
    match (top_row.is_empty(), bottom_row.is_empty()) {
        (false, false) => vec![(top_row, true), (bottom_row, false)],
        (true, false) => vec![(bottom_row, false)],
        (false, true) => vec![(top_row, false)],
        (true, true) => Vec::new(),
    }
}

/// Every shown board laid out in a viewer's world. `top_row` / `bottom_row` are the viewer's
/// rows of players, `sizes` every board's unturned size (see [`board_size`]).
pub fn layout_boards(
    game: &GameDefinition,
    state: &TableState,
    top_row: &[PlayerInfo],
    bottom_row: &[PlayerInfo],
    is_mirrored: bool,
    sizes: &[(f32, f32)],
) -> Vec<BoardOnTable> {
    let shown: HashSet<(String, i64)> = shown_boards(game, state).into_iter().collect();
    let is_shown = |owner_key: &str, board_index: i64| shown.contains(&(owner_key.to_string(), board_index));
    let size_of = |board_index: i64| {
        usize::try_from(board_index - 1).ok().and_then(|index| sizes.get(index).copied()).unwrap_or_else(|| {
            board_at(game, board_index).map_or(PLAYER_BOARD_FALLBACK_SIZE, |board| board_size(board, None))
        })
    };
    let mut boards = Vec::new();
    if has_placed_boards(game) {
        for (index, board) in game.game_boards.iter().enumerate() {
            let board_index = index as i64 + 1;
            if let (Some(placement), BoardKind::Shared) = (&board.table_placement, board.kind)
                && is_shown(SHARED_OWNER_KEY, board_index)
            {
                let (world_rect, rotation) = shared_board_on_table(placement, size_of(board_index), is_mirrored);
                boards.push(BoardOnTable {
                    owner_key: SHARED_OWNER_KEY.to_string(),
                    board_index,
                    world_rect,
                    rotation,
                });
            }
        }
        for (players, far_side) in seat_rows(top_row, bottom_row) {
            let origins = seat_origin_xs(players.len());
            for (player, origin_x) in players.iter().zip(origins) {
                let Some(seat) = seat_number(state, &player.id) else { continue };
                for (index, board) in game.game_boards.iter().enumerate() {
                    let board_index = index as i64 + 1;
                    if board.kind != BoardKind::Player || !is_shown(&player.id, board_index) {
                        continue;
                    }
                    let size = size_of(board_index);
                    let (world_rect, rotation) = if let Some(moved) = moved_board_of(player, board_index) {
                        moved_board_on_table(moved, size, is_mirrored)
                    } else if let Some(placement) = board.player_placement_for(seat) {
                        player_placement_on_table(placement, size, is_mirrored)
                    } else if let Some(placement) = &board.table_placement {
                        player_board_on_table(origin_x, far_side, placement, size)
                    } else {
                        continue;
                    };
                    boards.push(BoardOnTable { owner_key: player.id.clone(), board_index, world_rect, rotation });
                }
            }
        }
        return boards;
    }
    match board_toggle(game) {
        BoardToggle::DefaultPlayerBoard => {
            let solo = top_row.is_empty() || bottom_row.is_empty();
            for (players, far_side) in seat_rows(top_row, bottom_row) {
                // Every player's board size counts toward the row, even when it's off, so
                // toggling one doesn't move the others (`drawPlayerRowMarkers`).
                let chosen: Vec<Option<i64>> =
                    players.iter().map(|player| default_player_board(game, player.chosen_player_board)).collect();
                let sizes: Vec<(f32, f32)> =
                    chosen.iter().map(|board_index| board_index.map_or(PLAYER_BOARD_FALLBACK_SIZE, size_of)).collect();
                let widths: Vec<f32> = sizes.iter().map(|size| size.0).collect();
                let lefts = row_lefts(&widths, SEAT_GAP);
                for (seat, player) in players.iter().enumerate() {
                    let Some(board_index) = chosen[seat].filter(|&board_index| is_shown(&player.id, board_index))
                    else {
                        continue;
                    };
                    let (width, height) = sizes[seat];
                    let top = if solo {
                        (WORLD_H - height) / 2.0
                    } else if far_side {
                        WORLD_H / 2.0 - height
                    } else {
                        WORLD_H / 2.0
                    };
                    let (world_rect, rotation) = match moved_board_of(player, board_index) {
                        Some(moved) => moved_board_on_table(moved, (width, height), is_mirrored),
                        None => (Rect::new(lefts[seat], top, width, height), if far_side { 180 } else { 0 }),
                    };
                    boards.push(BoardOnTable { owner_key: player.id.clone(), board_index, world_rect, rotation });
                }
            }
        }
        BoardToggle::DefaultSharedBoard => {
            if let Some(board_index) = default_shared_board(game, state.chosen_shared_board)
                && is_shown(SHARED_OWNER_KEY, board_index)
            {
                let (width, height) = size_of(board_index);
                boards.push(BoardOnTable {
                    owner_key: SHARED_OWNER_KEY.to_string(),
                    board_index,
                    world_rect: Rect::new((WORLD_W - width) / 2.0, (WORLD_H - height) / 2.0, width, height),
                    rotation: 0,
                });
            }
        }
        _ => {}
    }
    boards
}

#[cfg(test)]
mod tests {
    use super::*;
    use gametable_core::models::PlayerRole;

    fn player(id: &str, shown: bool) -> PlayerInfo {
        let mut player = PlayerInfo::new(id, id, PlayerRole::Client, 0xFF00_0000);
        player.player_boards_shown = shown;
        player
    }

    fn board(kind: BoardKind, placement: Option<BoardTablePlacement>) -> GameBoardDefinition {
        GameBoardDefinition { kind, table_placement: placement, ..GameBoardDefinition::new("b.png") }
    }

    fn game_with(boards: Vec<GameBoardDefinition>) -> GameDefinition {
        let mut game = GameDefinition::new("g", "G");
        game.game_boards = boards;
        game
    }

    fn state_with(players: Vec<PlayerInfo>) -> TableState {
        let mut state = TableState::new("g");
        state.players = players;
        state
    }

    #[test]
    fn boards_meet_at_the_centre_line() {
        assert_eq!(row_lefts(&[100.0, 200.0], 10.0), vec![1180.0, 1280.0]);
        assert_eq!(row_lefts(&[100.0, 50.0, 100.0], 10.0), vec![1145.0, 1255.0, 1315.0]);
        assert!(row_lefts(&[], 10.0).is_empty());
    }

    #[test]
    fn placed_boards_show_unless_toggleable_and_hidden() {
        let toggleable = BoardTablePlacement { toggleable: true, ..BoardTablePlacement::new(0.0, -300.0) };
        let game = game_with(vec![
            board(BoardKind::Player, Some(BoardTablePlacement::new(0.0, -200.0))),
            board(BoardKind::Player, Some(toggleable.clone())),
            board(
                BoardKind::Shared,
                Some(BoardTablePlacement { toggleable: true, ..BoardTablePlacement::new(1280.0, 720.0) }),
            ),
            board(BoardKind::Player, None),
        ]);
        let mut state = state_with(vec![player("a", false), player("b", true)]);
        let shown = shown_boards(&game, &state);
        assert_eq!(shown, [("a".to_string(), 1), ("b".to_string(), 1), ("b".to_string(), 2)]);
        state.shared_boards_shown = true;
        assert!(shown_boards(&game, &state).contains(&(SHARED_OWNER_KEY.to_string(), 3)));
        assert_eq!(board_toggle(&game), BoardToggle::PlacedBoards { player_boards: true, shared_boards: true });
        assert!(!players_choose_their_board(&game));
    }

    #[test]
    fn f12_does_nothing_when_every_placed_board_is_fixed() {
        let game = game_with(vec![board(BoardKind::Player, Some(BoardTablePlacement::new(0.0, -200.0)))]);
        assert_eq!(board_toggle(&game), BoardToggle::Nothing);
        assert_eq!(board_toggle(&game_with(Vec::new())), BoardToggle::Nothing);
    }

    #[test]
    fn with_nothing_placed_each_player_shows_their_chosen_player_board() {
        let game = game_with(vec![
            board(BoardKind::Shared, None),
            board(BoardKind::Player, None),
            board(BoardKind::Player, None),
        ]);
        assert_eq!(board_toggle(&game), BoardToggle::DefaultPlayerBoard);
        assert!(players_choose_their_board(&game));
        let mut chooser = player("a", true);
        chooser.chosen_player_board = 3;
        let mut wrong_kind = player("b", true);
        wrong_kind.chosen_player_board = 1;
        let state = state_with(vec![chooser, wrong_kind, player("c", false)]);
        assert_eq!(shown_boards(&game, &state), [("a".to_string(), 3), ("b".to_string(), 2)]);
    }

    #[test]
    fn a_game_with_only_shared_boards_keeps_the_centred_shared_board() {
        let game = game_with(vec![board(BoardKind::Shared, None), board(BoardKind::Shared, None)]);
        assert_eq!(board_toggle(&game), BoardToggle::DefaultSharedBoard);
        let mut state = state_with(vec![player("a", true)]);
        assert!(shown_boards(&game, &state).is_empty());
        state.shared_boards_shown = true;
        state.chosen_shared_board = 2;
        assert_eq!(shown_boards(&game, &state), [(SHARED_OWNER_KEY.to_string(), 2)]);
        let laid_out = layout_boards(&game, &state, &[], &state.players.clone(), false, &[(1000.0, 500.0); 2]);
        assert_eq!(laid_out[0].world_rect, Rect::new(780.0, 470.0, 1000.0, 500.0));
    }

    #[test]
    fn quarter_turns_swap_width_and_height() {
        assert_eq!(turned_size((300.0, 100.0), 0), (300.0, 100.0));
        assert_eq!(turned_size((300.0, 100.0), 90), (100.0, 300.0));
        assert_eq!(turned_size((300.0, 100.0), 180), (300.0, 100.0));
        assert_eq!(turned_size((300.0, 100.0), 270), (100.0, 300.0));
    }

    #[test]
    fn each_seat_gets_its_player_board_in_front_of_it() {
        let placement = BoardTablePlacement::new(0.0, 150.0);
        let game = game_with(vec![board(BoardKind::Player, Some(placement))]);
        let sizes = [(1000.0, 300.0)];
        let state = state_with(vec![player("me", false), player("them", false)]);
        let laid_out = layout_boards(&game, &state, &state.players[1..], &state.players[..1], false, &sizes);
        assert_eq!(laid_out.len(), 2);
        // Across the table: just above the middle line, turned around.
        assert_eq!(laid_out[0].owner_key, "them");
        assert_eq!(laid_out[0].world_rect, Rect::new(780.0, 420.0, 1000.0, 300.0));
        assert_eq!(laid_out[0].rotation, 180);
        // Mine: just below it, upright (where the original's mats sit).
        assert_eq!(laid_out[1].world_rect, Rect::new(780.0, 720.0, 1000.0, 300.0));
        assert_eq!(laid_out[1].rotation, 0);
    }

    #[test]
    fn with_two_seats_a_side_every_copy_sits_at_the_same_offset_from_its_seat() {
        // A board placed 300 left of the seat origin.
        let game = game_with(vec![board(BoardKind::Player, Some(BoardTablePlacement::new(-300.0, 150.0)))]);
        let state = state_with(vec![player("a", true), player("b", true), player("c", true), player("d", true)]);
        let (top, bottom) = (&state.players[2..], &state.players[..2]);
        let laid_out = layout_boards(&game, &state, top, bottom, false, &[(400.0, 300.0)]);
        let center_x = |owner: &str| {
            let rect = laid_out.iter().find(|board| board.owner_key == owner).unwrap().world_rect;
            rect.x + rect.width / 2.0
        };
        // Mirrored around the table: on the near side, 300 left of the left seat's middle and 300
        // right of the right seat's; across the table the same, as those players see it.
        assert_eq!((center_x("a"), center_x("b")), (340.0, 2220.0));
        assert_eq!((center_x("c"), center_x("d")), (340.0, 2220.0));
    }

    #[test]
    fn seats_in_a_row_sit_side_by_side_and_an_offset_turns_with_the_far_side() {
        // Seats split the table evenly: a lone seat in the middle, two at a quarter and three quarters.
        assert_eq!(seat_origin_xs(1), [1280.0]);
        assert_eq!(seat_origin_xs(2), [640.0, 1920.0]);
        let placement = BoardTablePlacement { rotation: 90, ..BoardTablePlacement::new(200.0, -100.0) };
        // A far seat right of the middle as the viewer sees it is in its player's left half.
        let (rect, rotation) = player_board_on_table(1500.0, true, &placement, (400.0, 200.0));
        assert_eq!(rotation, 270);
        assert_eq!(rect, Rect::new(1200.0, 620.0, 200.0, 400.0));
        // The far seat left of the middle is in its player's right half: mirrored, the offset goes
        // the other way and the quarter turn is the other way round.
        let (rect, rotation) = player_board_on_table(1000.0, true, &placement, (400.0, 200.0));
        assert_eq!(rotation, 90);
        assert_eq!(rect, Rect::new(1100.0, 620.0, 200.0, 400.0));
    }

    #[test]
    fn a_board_placed_for_one_player_replaces_their_every_seat_copy() {
        let mut mat = board(BoardKind::Player, Some(BoardTablePlacement::new(0.0, 150.0)));
        mat.player_placements =
            vec![BoardPlayerPlacement { rotation: 90, ..BoardPlayerPlacement::new(2, 400.0, 300.0) }];
        let game = game_with(vec![mat]);
        let state = state_with(vec![player("me", false), player("them", false)]);
        let sizes = [(1000.0, 300.0)];
        let (top, bottom) = (&state.players[1..], &state.players[..1]);
        let laid_out = layout_boards(&game, &state, top, bottom, false, &sizes);
        // Player 2 ("them") gets their own spot, as the first seat sees the table.
        assert_eq!(laid_out[0].owner_key, "them");
        assert_eq!((laid_out[0].world_rect, laid_out[0].rotation), (Rect::new(250.0, -200.0, 300.0, 1000.0), 90));
        // Player 1 keeps the every-seat copy.
        assert_eq!(laid_out[1].world_rect, Rect::new(780.0, 720.0, 1000.0, 300.0));
        // Player 2 sees the table turned around, their board with it.
        let mirrored = layout_boards(&game, &state, bottom, top, true, &sizes);
        let theirs = mirrored.iter().find(|board| board.owner_key == "them").unwrap();
        assert_eq!((theirs.world_rect, theirs.rotation), (Rect::new(2010.0, 640.0, 300.0, 1000.0), 270));

        // Placed for player 2 only: player 1 gets nothing.
        let mut only_two = board(BoardKind::Player, None);
        only_two.player_placements = vec![BoardPlayerPlacement::new(2, 400.0, 300.0)];
        let game = game_with(vec![only_two]);
        assert_eq!(shown_boards(&game, &state), [("them".to_string(), 1)]);
    }

    #[test]
    fn a_board_its_player_moved_sits_where_they_left_it_for_every_viewer() {
        let game = game_with(vec![board(BoardKind::Player, Some(BoardTablePlacement::new(0.0, 150.0)))]);
        let mut me = player("me", false);
        me.moved_boards = vec![MovedBoard::new(1, 0.25, 0.75, 90)];
        let state = state_with(vec![me, player("them", false)]);
        let sizes = [(400.0, 200.0)];
        let (top, bottom) = (&state.players[1..], &state.players[..1]);
        let mine = layout_boards(&game, &state, top, bottom, false, &sizes)
            .into_iter()
            .find(|board| board.owner_key == "me")
            .unwrap();
        assert_eq!((mine.world_rect, mine.rotation), (Rect::new(540.0, 880.0, 200.0, 400.0), 90));
        assert_eq!(table_spot_of(mine.world_rect, mine.rotation, false), ((0.25, 0.75), 90));
        // Across the table it is turned around with everything else, and maps back the same.
        let theirs_view = layout_boards(&game, &state, bottom, top, true, &sizes)
            .into_iter()
            .find(|board| board.owner_key == "me")
            .unwrap();
        assert_eq!((theirs_view.world_rect, theirs_view.rotation), (Rect::new(1820.0, 160.0, 200.0, 400.0), 270));
        assert_eq!(table_spot_of(theirs_view.world_rect, theirs_view.rotation, true), ((0.25, 0.75), 90));
    }

    #[test]
    fn a_shared_board_turns_around_for_a_mirrored_viewer() {
        // Locked in the middle until unlocked.
        let locked = BoardTablePlacement::new(500.0, 400.0);
        let (rect, _) = shared_board_on_table(&locked, (400.0, 200.0), false);
        assert_eq!(rect, Rect::new(1080.0, 620.0, 400.0, 200.0));
        let placement =
            BoardTablePlacement { rotation: 90, unlocked_from_center: true, ..BoardTablePlacement::new(500.0, 400.0) };
        let (rect, rotation) = shared_board_on_table(&placement, (400.0, 200.0), false);
        assert_eq!((rect, rotation), (Rect::new(400.0, 200.0, 200.0, 400.0), 90));
        let (rect, rotation) = shared_board_on_table(&placement, (400.0, 200.0), true);
        assert_eq!((rect, rotation), (Rect::new(1960.0, 840.0, 200.0, 400.0), 270));
    }
}
