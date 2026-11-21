//! Moving and turning your own player boards during the game (a GameTable addition).
//!
//! Right-click one of your player boards and pick "Unlock Board" to show its handle, a small bar
//! just beyond the board's bottom edge (as the board lies). Drag the handle to move the board, and
//! press Q or E while dragging to turn it a quarter turn either way; the zones placed on it go with
//! it, and so do the cards and tokens lying on it (they keep their place on the board and turn with
//! it), as they do when the board is reset. Everyone sees where you leave it. "Lock Board" hides the handle again, and "Reset Board
//! Position" puts the board back where the game puts it.

use gametable_core::models::{BoardCarry, BoardKind, BoardSpot, CardZone, MovedBoard, PlayerInfo};
use gametable_core::table_actions::carry_with_board;

use crate::app::Frame;
use crate::graphics::draw_list::Color;
use crate::graphics::tessellation::DrawMode;
use crate::table::TableAction;
use crate::table::board_layout::{BoardOnTable, board_at, layout_boards, rect_around, table_spot_of, turned_size};
use crate::table::screen::TableScreen;
use crate::ui::context::Rect;

/// The handle's height and its gap from the board, in screen pixels (so it stays easy to grab at
/// any zoom); its width is a quarter of the board's.
pub const HANDLE_HEIGHT: f32 = 15.0;
pub const HANDLE_GAP: f32 = 30.0;

/// A board being moved by its handle: where it is now (world units, this viewer's world), its
/// size before turning, its turn as this viewer sees it, and where the handle was grabbed
/// (offset from the board's centre). `start_rect` and `start_rotation` are where the board lay
/// when the drag began, and `carried` the cards and tokens lying on it then, which go with it.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardDrag {
    pub board_index: i64,
    pub center: (f32, f32),
    pub size: (f32, f32),
    pub rotation: i64,
    pub grab_offset: (f32, f32),
    pub start_rect: Rect,
    pub start_rotation: i64,
    pub carried: ItemsOnBoard,
}

/// The ids of the cards and tokens lying on a board.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemsOnBoard {
    pub card_ids: Vec<String>,
    pub widget_ids: Vec<String>,
}

impl BoardDrag {
    /// The board's bounds where it is being dragged.
    pub fn world_rect(&self) -> Rect {
        rect_around(self.center, turned_size(self.size, self.rotation))
    }

    /// Turns the board a quarter turn (clockwise unless `counterclockwise`) around its centre.
    pub fn turn(&mut self, counterclockwise: bool) {
        let step = if counterclockwise { -90 } else { 90 };
        self.rotation = (self.rotation + step).rem_euclid(360);
    }
}

/// A board's handle in world units, at `zoom` screen pixels per world unit: a quarter of the
/// board's width, centred beyond its bottom edge as the board lies, so it turns with the board.
pub fn handle_rect(board: &BoardOnTable, zoom: f32) -> Rect {
    let rect = board.world_rect;
    let (width, height) = turned_size((rect.width, rect.height), board.rotation);
    let zoom = zoom.max(0.01);
    let handle_size = (width / 4.0, HANDLE_HEIGHT / zoom);
    let reach = height / 2.0 + HANDLE_GAP / zoom + handle_size.1 / 2.0;
    let (offset_x, offset_y) = match board.rotation.rem_euclid(360) {
        90 => (-reach, 0.0),
        180 => (0.0, -reach),
        270 => (reach, 0.0),
        _ => (0.0, reach),
    };
    let center = (rect.x + rect.width / 2.0 + offset_x, rect.y + rect.height / 2.0 + offset_y);
    rect_around(center, turned_size(handle_size, board.rotation))
}

impl TableScreen {
    /// This player's own player boards shown on the table last frame.
    fn my_movable_boards(&self) -> impl Iterator<Item = &BoardOnTable> {
        let can_move = !self.spectator && self.player_by_id(&self.acting_player_id).is_some();
        self.boards_shown.iter().filter(move |board| {
            can_move
                && board.owner_key == self.acting_player_id
                && board_at(&self.game, board.board_index).is_some_and(|board| board.kind == BoardKind::Player)
        })
    }

    /// The unlocked board whose handle is under a screen point.
    fn handle_under(&self, screen_x: f32, screen_y: f32) -> Option<BoardOnTable> {
        let (world_x, world_y) = self.screen_to_world(screen_x, screen_y);
        self.my_movable_boards()
            .filter(|board| self.unlocked_boards.contains(&board.board_index))
            .find(|board| handle_rect(board, self.zoom).contains(world_x, world_y))
            .cloned()
    }

    /// One of this player's own boards (or its handle) under a screen point, for the table menu.
    pub(super) fn my_board_under(&self, screen_x: f32, screen_y: f32) -> Option<i64> {
        let (world_x, world_y) = self.screen_to_world(screen_x, screen_y);
        self.my_movable_boards()
            .filter(|board| {
                board.world_rect.contains(world_x, world_y)
                    || (self.unlocked_boards.contains(&board.board_index)
                        && handle_rect(board, self.zoom).contains(world_x, world_y))
            })
            .last()
            .map(|board| board.board_index)
    }

    /// Whether this player moved a board during the game.
    pub(super) fn has_moved_board(&self, board_index: i64) -> bool {
        self.player_by_id(&self.acting_player_id)
            .is_some_and(|me| me.moved_boards.iter().any(|moved| moved.board_index == board_index))
    }

    /// "Unlock Board" / "Lock Board": shows or hides a board's handle.
    pub(super) fn switch_board_lock(&mut self, board_index: i64) {
        if !self.unlocked_boards.remove(&board_index) {
            self.unlocked_boards.insert(board_index);
        }
    }

    /// "Reset Board Position": back where the game puts it, carrying what lies on it.
    pub(super) fn reset_board_position(&mut self, frame: &mut Frame, board_index: i64) {
        let carried = self.board_carry_to_home_spot(frame, board_index);
        self.perform(TableAction::SetMovedBoard { board_index, moved: None, carried });
    }

    /// One of this player's own boards as shown last frame.
    fn my_board_shown(&self, board_index: i64) -> Option<BoardOnTable> {
        self.boards_shown
            .iter()
            .find(|board| board.owner_key == self.acting_player_id && board.board_index == board_index)
            .cloned()
    }

    /// What lies on one of this player's boards and where it goes when the board goes back to
    /// where the game puts it; `None` when the board isn't shown.
    fn board_carry_to_home_spot(&self, frame: &mut Frame, board_index: i64) -> Option<BoardCarry> {
        let current = self.my_board_shown(board_index)?;
        // The layout reads moved boards from the seat rows' players.
        let forget_the_move = |players: &[PlayerInfo]| -> Vec<PlayerInfo> {
            let mut players = players.to_vec();
            for player in players.iter_mut().filter(|player| player.id == self.acting_player_id) {
                player.moved_boards.retain(|moved| moved.board_index != board_index);
            }
            players
        };
        let (top_row, bottom_row) = (forget_the_move(&self.derived.top_row), forget_the_move(&self.derived.bottom_row));
        let sizes = self.board_sizes(frame);
        let home = layout_boards(&self.game, &self.state, &top_row, &bottom_row, self.is_mirrored, &sizes)
            .into_iter()
            .find(|board| board.owner_key == self.acting_player_id && board.board_index == board_index)?;
        let items = self.items_on_board(current.world_rect);
        Some(self.board_carry(items, (current.world_rect, current.rotation), (home.world_rect, home.rotation)))
    }

    /// The cards and tokens lying on a board shown at `rect` (this viewer's world): every table
    /// card and free (not docked, not attached) widget whose centre is on the board. Tokens
    /// attached to those cards follow their cards; the session leaves behind anything this player
    /// may not move.
    fn items_on_board(&self, rect: Rect) -> ItemsOnBoard {
        let lies_on_board = |x: f64, y: f64| {
            let (world_x, world_y) = self.canonical_to_world(x, y);
            rect.contains(world_x, world_y)
        };
        let card_ids = self
            .state
            .cards()
            .iter()
            .filter(|card| card.zone == CardZone::Table && lies_on_board(card.x, card.y))
            .map(|card| card.instance_id.clone())
            .collect();
        let widget_ids = self
            .state
            .widgets()
            .iter()
            .filter(|widget| {
                widget.zone_id.is_none() && widget.attached_card_id.is_none() && lies_on_board(widget.x, widget.y)
            })
            .map(|widget| widget.instance_id.clone())
            .collect();
        ItemsOnBoard { card_ids, widget_ids }
    }

    /// `items` on a board going from `from` to `to` (bounds in this viewer's world, turn).
    fn board_carry(&self, items: ItemsOnBoard, from: (Rect, i64), to: (Rect, i64)) -> BoardCarry {
        let spot_of = |(rect, rotation): (Rect, i64)| {
            let ((x, y), rotation) = table_spot_of(rect, rotation, self.is_mirrored);
            BoardSpot { x, y, rotation }
        };
        BoardCarry { from: spot_of(from), to: spot_of(to), card_ids: items.card_ids, widget_ids: items.widget_ids }
    }

    /// While a board is dragged, shows what lies on it riding along: this frame's copy of the
    /// table has those cards and tokens carried to where the board is now. The session only
    /// changes when the drag ends.
    pub(super) fn show_items_riding_on_the_dragged_board(&mut self) {
        let Some(drag) = &self.board_drag else { return };
        let carry = self.board_carry(
            drag.carried.clone(),
            (drag.start_rect, drag.start_rotation),
            (drag.world_rect(), drag.rotation),
        );
        self.state = carry_with_board(&self.state, &carry, &self.acting_player_id);
    }

    /// Where a board being dragged is shown (this frame's layout, with the dragged board moved).
    pub(super) fn with_dragged_board(&self, mut boards: Vec<BoardOnTable>) -> Vec<BoardOnTable> {
        if let Some(drag) = &self.board_drag
            && let Some(board) = boards
                .iter_mut()
                .find(|board| board.owner_key == self.acting_player_id && board.board_index == drag.board_index)
        {
            board.world_rect = drag.world_rect();
            board.rotation = drag.rotation;
        }
        boards
    }

    /// A press on a handle starts dragging its board; true when it did.
    pub(super) fn start_board_drag(&mut self, screen_x: f32, screen_y: f32) -> bool {
        let Some(board) = self.handle_under(screen_x, screen_y) else { return false };
        let rect = board.world_rect;
        let center = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        let (world_x, world_y) = self.screen_to_world(screen_x, screen_y);
        let carried = self.items_on_board(rect);
        self.board_drag = Some(BoardDrag {
            board_index: board.board_index,
            center,
            size: turned_size((rect.width, rect.height), board.rotation),
            rotation: board.rotation,
            grab_offset: (world_x - center.0, world_y - center.1),
            start_rect: rect,
            start_rotation: board.rotation,
            carried,
        });
        true
    }

    /// Follows the mouse while a board is dragged; on release the board's new spot goes to
    /// everyone. A right click cancels.
    pub(super) fn update_board_drag(&mut self, frame: &mut Frame, screen_x: f32, screen_y: f32) {
        let Some(mut drag) = self.board_drag.clone() else { return };
        frame.ui.set_cursor("sizeall");
        if frame.ui.mouse_pressed(2) {
            self.board_drag = None;
            return;
        }
        let (world_x, world_y) = self.screen_to_world(screen_x, screen_y);
        drag.center = (world_x - drag.grab_offset.0, world_y - drag.grab_offset.1);
        if frame.ui.mouse_released(1) || !frame.ui.mouse_down(1) {
            self.board_drag = None;
            let ((x, y), rotation) = table_spot_of(drag.world_rect(), drag.rotation, self.is_mirrored);
            let moved = MovedBoard::new(drag.board_index, x, y, rotation);
            let carried = self.board_carry(
                drag.carried.clone(),
                (drag.start_rect, drag.start_rotation),
                (drag.world_rect(), drag.rotation),
            );
            self.perform(TableAction::SetMovedBoard {
                board_index: drag.board_index,
                moved: Some(moved),
                carried: Some(carried),
            });
            return;
        }
        self.board_drag = Some(drag);
    }

    /// Q / E while dragging a board turn it; true when a board is being dragged.
    pub(super) fn turn_dragged_board(&mut self, counterclockwise: bool) -> bool {
        let Some(drag) = &mut self.board_drag else { return false };
        // Turn around the grab point, so the handle stays under the mouse.
        let (grab_x, grab_y) = (drag.center.0 + drag.grab_offset.0, drag.center.1 + drag.grab_offset.1);
        let (offset_x, offset_y) = drag.grab_offset;
        drag.grab_offset = if counterclockwise { (offset_y, -offset_x) } else { (-offset_y, offset_x) };
        drag.center = (grab_x - drag.grab_offset.0, grab_y - drag.grab_offset.1);
        drag.turn(counterclockwise);
        true
    }

    /// Where the handle of one of this player's unlocked boards is on screen (for scripted UI
    /// checks).
    pub fn board_handle_center(&self, board_index: i64) -> Option<(f32, f32)> {
        let board = self
            .my_movable_boards()
            .find(|board| board.board_index == board_index && self.unlocked_boards.contains(&board_index))?;
        let rect = handle_rect(board, self.zoom);
        Some(self.world_to_screen(rect.x + rect.width / 2.0, rect.y + rect.height / 2.0))
    }

    /// The handles of this player's unlocked boards.
    pub(super) fn draw_board_handles(&self, frame: &mut Frame) {
        let (mouse_x, mouse_y) = frame.ui.mouse();
        let hovered = self.handle_under(mouse_x, mouse_y).map(|board| board.board_index);
        let handles: Vec<(i64, Rect)> = self
            .my_movable_boards()
            .filter(|board| self.unlocked_boards.contains(&board.board_index))
            .map(|board| (board.board_index, handle_rect(board, self.zoom)))
            .collect();
        for (board_index, rect) in handles {
            let active = hovered == Some(board_index)
                || self.board_drag.as_ref().is_some_and(|drag| drag.board_index == board_index);
            let (x, y) = self.world_to_screen(rect.x, rect.y);
            let (width, height) = (rect.width * self.zoom, rect.height * self.zoom);
            let radius = width.min(height) / 2.0;
            let list = frame.ui.list();
            list.set_color(Color::rgba(1.0, 1.0, 1.0, if active { 0.95 } else { 0.7 }));
            list.rectangle(DrawMode::Fill, x, y, width, height, radius, radius);
            list.set_color(Color::rgba(0.0, 0.0, 0.0, 0.35));
            list.set_line_width(1.0);
            list.rectangle(DrawMode::Line, x, y, width, height, radius, radius);
        }
        if hovered.is_some() && self.board_drag.is_none() {
            frame.ui.set_cursor("sizeall");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(world_rect: Rect, rotation: i64) -> BoardOnTable {
        BoardOnTable { owner_key: "me".to_string(), board_index: 1, world_rect, rotation }
    }

    #[test]
    fn the_handle_sits_beyond_the_bottom_edge_as_the_board_lies() {
        // Upright at zoom 1: a quarter of the width, 30 below the bottom edge, 15 tall.
        let upright = board(Rect::new(100.0, 100.0, 400.0, 200.0), 0);
        assert_eq!(handle_rect(&upright, 1.0), Rect::new(250.0, 330.0, 100.0, 15.0));
        // Turned upside down (across the table) it is above the board.
        let across = board(Rect::new(100.0, 100.0, 400.0, 200.0), 180);
        assert_eq!(handle_rect(&across, 1.0), Rect::new(250.0, 55.0, 100.0, 15.0));
        // A quarter turn clockwise puts the board's bottom on the left.
        let turned = board(Rect::new(100.0, 100.0, 200.0, 400.0), 90);
        assert_eq!(handle_rect(&turned, 1.0), Rect::new(55.0, 250.0, 15.0, 100.0));
        // Zoomed out, the gap and height stay the same on screen.
        let zoomed_out = handle_rect(&upright, 0.5);
        assert_eq!((zoomed_out.y, zoomed_out.height), (360.0, 30.0));
    }

    #[test]
    fn turning_a_dragged_board_swaps_its_sides() {
        let mut drag = BoardDrag {
            board_index: 1,
            center: (500.0, 500.0),
            size: (400.0, 200.0),
            rotation: 0,
            grab_offset: (0.0, 0.0),
            start_rect: Rect::new(300.0, 400.0, 400.0, 200.0),
            start_rotation: 0,
            carried: ItemsOnBoard::default(),
        };
        drag.turn(false);
        assert_eq!((drag.rotation, drag.world_rect()), (90, Rect::new(400.0, 300.0, 200.0, 400.0)));
        drag.turn(true);
        drag.turn(true);
        assert_eq!(drag.rotation, 270);
    }
}
