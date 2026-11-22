//! The Game Boards tab's Table Preview (a GameTable addition): the whole table, where the
//! designer puts boards so they are there whenever the game loads.
//!
//! Right-click the table for "Add To Table...", a list of the boards not on it yet: a shared board
//! goes in the middle of the table (locked there until unlocked), a player board in front of
//! every seat. The viewer (player 1) sits at the bottom middle; a player count previews how every
//! other seat gets its own copy of the player boards (dimmed, turned for seats across the table).
//! A player board can instead be placed for single players, each at its own spot: "Add For
//! Player" on the table's menu, or "Place Separately For Player N" on a seat's copy, which
//! replaces that seat's copy. Drag a board (your copy, for every-seat player boards) to move it;
//! right-click it to turn it a quarter turn, mark it "Can Be Toggled (F12)", give a separately
//! placed board to another player, lock or unlock a shared board from the middle, or take it off
//! the table. The wheel zooms, dragging the felt pans, and the percentage fits the whole table
//! again. The geometry is the table's own (`table::board_layout`), so what you see here is what
//! players get. "The whole table" is everything players can reach: the table's 2560x1440 middle
//! (outlined) plus the margin around it that the table lets players pan and drag into
//! (`TABLE_PAN_MARGIN_FRACTION`), drawn in a darker felt.

use std::f32::consts::PI;

use gametable_core::geometry::{TABLE_PAN_MARGIN_FRACTION, WORLD_HEIGHT, WORLD_WIDTH};
use gametable_core::models::{
    BoardKind, BoardPlayerPlacement, BoardTablePlacement, GameBoardDefinition, GameDefinition, PlayerInfo, PlayerRole,
    TableState,
};
use gametable_core::seat_utils::compute_hand_row_layout;

use crate::app::Frame;
use crate::data::display_settings::TableColor;
use crate::data::file_system::join_path;
use crate::graphics::draw_list::{Color, StencilCompare};
use crate::graphics::tessellation::DrawMode;
use crate::screens::gamedef::EditorModel;
use crate::screens::gamedef::board_zone_placement::{draw_zone_stand_in, placeable_zones};
use crate::screens::gamedef::common::{PAD, column, display_card_backs};
use crate::screens::gamedef::list_picker_dialog::{ListPickerDialog, PickerEntry, PickerResponse};
use crate::table::board_layout::{
    BoardOnTable, board_size, has_placed_boards, layout_boards, rect_around, seat_mirrors_its_boards, seat_origin_xs,
    turned_size,
};
use crate::table::board_zones::placed_zone_world_center;
use crate::table::selection::clamp_world_center;
use crate::ui::components::argb_color;
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::overlays::{ContextMenu, MenuItem, MenuResponse};
use crate::ui::theme::with_alpha;
use crate::ui::widgets::{Choice, IconButtonOptions};

const WORLD_W: f32 = WORLD_WIDTH as f32;
const WORLD_H: f32 = WORLD_HEIGHT as f32;
/// How far past each edge of the table players can reach, in world units.
const REACHABLE_MARGIN_X: f32 = WORLD_W * TABLE_PAN_MARGIN_FRACTION as f32;
const REACHABLE_MARGIN_Y: f32 = WORLD_H * TABLE_PAN_MARGIN_FRACTION as f32;
/// The whole reachable table, in world units: (left, top, width, height).
pub const REACHABLE_WORLD_RECT: (f32, f32, f32, f32) = (
    -REACHABLE_MARGIN_X,
    -REACHABLE_MARGIN_Y,
    WORLD_W + 2.0 * REACHABLE_MARGIN_X,
    WORLD_H + 2.0 * REACHABLE_MARGIN_Y,
);
/// How much darker the reachable margin's felt is than the table's.
const MARGIN_FELT_BRIGHTNESS: f32 = 0.7;
/// The seat the preview is seen from.
const VIEWER_SEAT_ID: &str = "seat1";
/// The most seats the preview offers.
pub const MAXIMUM_PREVIEW_PLAYERS: usize = 6;
/// The selected board's outline (GameTable's neon cyan).
const SELECTED_OUTLINE: Color = Color { red: 0.0, green: 0.898, blue: 1.0, alpha: 1.0 };

/// Which of a board's placements: its every-seat one (a shared board's only one), or the one for a
/// single player (1-based seat).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlacementOwner {
    EverySeat,
    Player(i64),
}

/// One placement of a board (0-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlacementRef {
    board_index: usize,
    owner: PlacementOwner,
}

/// A board being dragged: which placement, where it was grabbed (offset from its centre on
/// screen), and for an every-seat player board the viewer's seat origin when the drag began (so
/// the board doesn't drift as the seats re-space around it).
struct DraggedBoard {
    placement: PlacementRef,
    grab_offset: (f32, f32),
    seat_origin_x: f32,
}

pub struct TableBoardEditor {
    player_count: usize,
    dragged: Option<DraggedBoard>,
    camera: PreviewCamera,
    /// The mouse position while the view is being dragged around.
    pan: Option<(f32, f32)>,
    menu: Option<ContextMenu<TableMenuAction>>,
    picker: Option<ListPickerDialog>,
    /// Who the open picker adds a board for (`None`: every seat, or the shared spot), and where on
    /// the table (world units) the menu was opened.
    adding_for: Option<i64>,
    add_at: (f32, f32),
}

impl Default for TableBoardEditor {
    fn default() -> Self {
        TableBoardEditor {
            player_count: 2,
            dragged: None,
            camera: PreviewCamera::default(),
            pan: None,
            menu: None,
            picker: None,
            adding_for: None,
            add_at: (WORLD_W / 2.0, WORLD_H / 2.0),
        }
    }
}

/// A board's name: its image's file name without the extension.
pub fn board_display_name(board: &GameBoardDefinition) -> String {
    let file_name = board.image_path.rsplit(['/', '\\']).next().unwrap_or(&board.image_path);
    file_name.rsplit_once('.').map_or(file_name, |(stem, _)| stem).to_string()
}

pub fn board_kind_label(kind: BoardKind) -> &'static str {
    match kind {
        BoardKind::Player => "Player board",
        BoardKind::Shared => "Shared board",
    }
}

/// Where a newly placed board goes: a shared board in the table's middle, a player board in front
/// of its player's seat with its top edge on the middle line (where the original's mats sit).
pub fn default_table_placement(kind: BoardKind, size: (f32, f32)) -> BoardTablePlacement {
    match kind {
        BoardKind::Shared => BoardTablePlacement::new(f64::from(WORLD_W / 2.0), f64::from(WORLD_H / 2.0)),
        BoardKind::Player => BoardTablePlacement::new(0.0, f64::from(size.1 / 2.0)),
    }
}

/// The preview seat id of player `number` (1-based).
fn seat_id(number: i64) -> String {
    format!("seat{number}")
}

/// The player number of a preview seat id.
fn seat_number_of(owner_key: &str) -> Option<i64> {
    owner_key.strip_prefix("seat").and_then(|number| number.parse().ok())
}

/// Seats for previewing `count` players, the viewer first, every board shown.
fn preview_state(count: usize) -> TableState {
    let mut state = TableState::new("preview");
    state.players = (1..=count)
        .map(|seat| {
            let mut player = PlayerInfo::new(seat_id(seat as i64), format!("Player {seat}"), PlayerRole::Client, 0);
            player.player_boards_shown = true;
            player
        })
        .collect();
    state.shared_boards_shown = true;
    state
}

/// A preview table of `count` seats as the table lays it out for the viewer (player 1, bottom
/// middle): the state, then the far and near rows of seats.
fn preview_seat_rows(count: usize) -> (TableState, Vec<PlayerInfo>, Vec<PlayerInfo>) {
    let state = preview_state(count);
    let rows = compute_hand_row_layout(&state.players, VIEWER_SEAT_ID);
    let top_row: Vec<PlayerInfo> = rows.top_row.into_iter().cloned().collect();
    let bottom_row: Vec<PlayerInfo> = rows.bottom_row.into_iter().cloned().collect();
    (state, top_row, bottom_row)
}

/// The boards on a preview table, where the table puts them for the viewer.
fn preview_boards(
    game: &GameDefinition,
    state: &TableState,
    top_row: &[PlayerInfo],
    bottom_row: &[PlayerInfo],
    sizes: &[(f32, f32)],
) -> Vec<BoardOnTable> {
    if has_placed_boards(game) { layout_boards(game, state, top_row, bottom_row, false, sizes) } else { Vec::new() }
}

/// The zones placed on a board, as card backs and widgets on the preview table.
#[allow(clippy::too_many_arguments)]
fn draw_zones_on_board(
    frame: &mut Frame,
    folder_path: &str,
    game: &GameDefinition,
    definition: &GameBoardDefinition,
    board: &BoardOnTable,
    world_rect: crate::ui::context::Rect,
    view: &PreviewView,
    alpha: f32,
    display_backs: &[gametable_core::models::CardBackDefinition],
) {
    let fitting = placeable_zones(game, definition.kind);
    for placement in &definition.zone_placements {
        let Some(candidate) = fitting.iter().find(|candidate| candidate.zone.id == placement.zone_id) else {
            continue;
        };
        let (world_x, world_y) =
            placed_zone_world_center(world_rect, board.rotation, placement.x as f32, placement.y as f32);
        draw_zone_stand_in(
            frame,
            folder_path,
            &candidate.zone,
            candidate.is_scenario_zone,
            view.to_screen(world_x, world_y),
            view.scale * placement.scale as f32,
            alpha,
            display_backs,
            game.scenario_card_back_id.as_deref(),
        );
    }
}

/// Draws the boards placed on the table for `player_count` seats, read-only, exactly where the
/// table puts them for player 1 (with the zones on them and every seat's label): the Scenarios
/// tab's Card Placement canvas shows them under the cards.
pub fn draw_placed_boards(
    frame: &mut Frame,
    folder_path: &str,
    game: &GameDefinition,
    view: &PreviewView,
    player_count: usize,
) {
    let (state, top_row, bottom_row) = preview_seat_rows(player_count);
    let sizes = board_sizes(frame, folder_path, game);
    let boards = preview_boards(game, &state, &top_row, &bottom_row, &sizes);
    let border = argb_color(TableColor::PlaymatBorder.resolve(frame.settings, game.playmat_border_color.as_deref()));
    let display_backs = display_card_backs(folder_path, &game.card_backs);
    for board in &boards {
        let Some(definition) =
            usize::try_from(board.board_index - 1).ok().and_then(|index| game.game_boards.get(index))
        else {
            continue;
        };
        let rect = board.world_rect;
        let (screen_x, screen_y) = view.to_screen(rect.x, rect.y);
        let image_path = join_path(&[folder_path, "_playmats", &definition.image_path]);
        draw_board_image(
            frame,
            &image_path,
            (screen_x, screen_y, rect.width * view.scale, rect.height * view.scale),
            board.rotation,
            12.0 * view.scale.max(0.2),
            1.0,
            with_alpha(border, 1.0),
            2.0,
        );
        draw_zones_on_board(frame, folder_path, game, definition, board, rect, view, 1.0, &display_backs);
    }
    draw_seat_labels(frame, view, &top_row, &bottom_row);
}

/// The zoom controls of a preview: out, the percentage (click to fit the whole table), in.
/// `id_prefix` keeps their regions apart between editors. Returns the width they take.
pub fn draw_zoom_controls(frame: &mut Frame, camera: &mut PreviewCamera, id_prefix: &str, right: f32, y: f32) -> f32 {
    let palette = frame.palette();
    let (button, row_height, percent_width) = (32.0, 32.0, 56.0);
    let zoom_in_x = right - button;
    let percent_x = zoom_in_x - percent_width;
    let zoom_out_x = percent_x - button;
    let (zoom_out_id, zoom_in_id) = (format!("{id_prefix}:zoom:out"), format!("{id_prefix}:zoom:in"));
    let zoom_out_options = IconButtonOptions::tooltip("Zoom out").id(&zoom_out_id).size(button, 20.0);
    let zoom_in_options = IconButtonOptions::tooltip("Zoom in").id(&zoom_in_id).size(button, 20.0);
    let mut zoom_factor = None;
    if frame.icon_button("zoom_out", zoom_out_x, y, &zoom_out_options) {
        zoom_factor = Some(1.0 / ZOOM_STEP);
    }
    if frame.icon_button("zoom_in", zoom_in_x, y, &zoom_in_options) {
        zoom_factor = Some(ZOOM_STEP);
    }
    let fit_region = frame.ui.region(
        &format!("{id_prefix}:zoom:fit"),
        percent_x,
        y,
        percent_width,
        row_height,
        RegionOptions::hand().with_tooltip(Some("Fit the whole table")),
    );
    if fit_region.hovered {
        frame.fill_rect(percent_x, y, percent_width, row_height, 8.0, with_alpha(palette.on_surface, 0.08));
    }
    let percent = format!("{}%", (camera.zoom * 100.0).round() as i64);
    let percent_options =
        TextOptions::new().size(13.0).width(percent_width).centered().middle_of(row_height).color(palette.on_surface);
    frame.text(&percent, percent_x, y, percent_options);
    if fit_region.clicked {
        camera.fit();
    }
    if let Some(factor) = zoom_factor {
        camera.zoom = (camera.zoom * factor).clamp(PREVIEW_MIN_ZOOM, PREVIEW_MAX_ZOOM);
    }
    right - zoom_out_x
}

/// The table preview's letterboxed canvas inside the box: (x, y, width, height, scale).
pub fn canvas_rect(x: f32, y: f32, width: f32, height: f32) -> (f32, f32, f32, f32, f32) {
    let scale = (width / WORLD_W).min(height / WORLD_H);
    let (canvas_width, canvas_height) = (WORLD_W * scale, WORLD_H * scale);
    (x + (width - canvas_width) / 2.0, y + (height - canvas_height) / 2.0, canvas_width, canvas_height, scale)
}

/// Every board's unturned size on the table (its image's size once loaded, times its scale).
pub fn board_sizes(frame: &mut Frame, folder_path: &str, game: &GameDefinition) -> Vec<(f32, f32)> {
    game.game_boards
        .iter()
        .map(|board| {
            let path = join_path(&[folder_path, "_playmats", &board.image_path]);
            let image_size = frame.images.get(&path).map(|texture| (texture.width, texture.height));
            board_size(board, image_size)
        })
        .collect()
}

/// A board's image cover-fit inside its (turned) screen rect, turned with it, then its border.
#[allow(clippy::too_many_arguments)]
pub fn draw_board_image(
    frame: &mut Frame,
    image_path: &str,
    (x, y, width, height): (f32, f32, f32, f32),
    rotation: i64,
    radius: f32,
    image_alpha: f32,
    border: Color,
    border_width: f32,
) {
    if let Some(texture) = frame.images.get(image_path) {
        let (upright_width, upright_height) = turned_size((width, height), rotation);
        let (center_x, center_y) = (x + width / 2.0, y + height / 2.0);
        let list = frame.ui.list();
        list.stencil(1, |list| list.rectangle(DrawMode::Fill, x, y, width, height, radius, radius));
        list.set_stencil_test(Some((StencilCompare::Greater, 0)));
        let cover = (upright_width / texture.width).max(upright_height / texture.height);
        list.set_color(Color::rgba(1.0, 1.0, 1.0, image_alpha));
        list.push();
        list.translate(center_x, center_y);
        list.rotate(rotation as f32 * PI / 180.0);
        list.translate(-center_x, -center_y);
        list.draw_texture(
            texture,
            None,
            center_x - texture.width * cover / 2.0,
            center_y - texture.height * cover / 2.0,
            0.0,
            cover,
            cover,
            0.0,
            0.0,
        );
        list.pop();
        list.set_stencil_test(None);
    }
    let list = frame.ui.list();
    list.set_color(border);
    list.set_line_width(border_width);
    list.rectangle(
        DrawMode::Line,
        x + border_width / 2.0,
        y + border_width / 2.0,
        width - border_width,
        height - border_width,
        radius,
        radius,
    );
    list.set_line_width(1.0);
}

/// The zoom and pan of a whole-table preview: `zoom` 1 fits the whole reachable table
/// ([`REACHABLE_WORLD_RECT`]) in the viewport, and `center` is the world point in the viewport's
/// middle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreviewCamera {
    pub zoom: f32,
    pub center: (f32, f32),
}

impl Default for PreviewCamera {
    fn default() -> Self {
        PreviewCamera { zoom: 1.0, center: (WORLD_W / 2.0, WORLD_H / 2.0) }
    }
}

/// How far the preview zooms out (a little past the fitted size) and in.
pub const PREVIEW_MIN_ZOOM: f32 = 0.75;
pub const PREVIEW_MAX_ZOOM: f32 = 12.0;
/// Each wheel notch or zoom button press zooms by this much.
pub const ZOOM_STEP: f32 = 1.2;

/// A camera applied to a viewport: world <-> screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreviewView {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Screen pixels per world unit.
    pub scale: f32,
    origin_x: f32,
    origin_y: f32,
}

impl PreviewView {
    pub fn to_screen(&self, world_x: f32, world_y: f32) -> (f32, f32) {
        (self.origin_x + world_x * self.scale, self.origin_y + world_y * self.scale)
    }

    pub fn to_world(&self, screen_x: f32, screen_y: f32) -> (f32, f32) {
        ((screen_x - self.origin_x) / self.scale, (screen_y - self.origin_y) / self.scale)
    }

    pub fn contains(&self, screen_x: f32, screen_y: f32) -> bool {
        (self.x..=self.x + self.width).contains(&screen_x) && (self.y..=self.y + self.height).contains(&screen_y)
    }
}

impl PreviewCamera {
    /// The camera applied to a viewport.
    pub fn view(&self, x: f32, y: f32, width: f32, height: f32) -> PreviewView {
        let (_, _, reachable_width, reachable_height) = REACHABLE_WORLD_RECT;
        let fit = (width / reachable_width).min(height / reachable_height);
        let scale = fit * self.zoom;
        PreviewView {
            x,
            y,
            width,
            height,
            scale,
            origin_x: x + width / 2.0 - self.center.0 * scale,
            origin_y: y + height / 2.0 - self.center.1 * scale,
        }
    }

    /// Zooms by `factor`, keeping the world point under `(screen_x, screen_y)` where it is.
    pub fn zoom_at(&mut self, view: &PreviewView, screen_x: f32, screen_y: f32, factor: f32) {
        let (world_x, world_y) = view.to_world(screen_x, screen_y);
        let zoom = (self.zoom * factor).clamp(PREVIEW_MIN_ZOOM, PREVIEW_MAX_ZOOM);
        let scale = view.scale / self.zoom * zoom;
        self.zoom = zoom;
        let (middle_x, middle_y) = (view.x + view.width / 2.0, view.y + view.height / 2.0);
        self.center = (world_x - (screen_x - middle_x) / scale, world_y - (screen_y - middle_y) / scale);
        self.keep_table_in_view();
    }

    /// Moves the view by a screen distance (dragging the table along with the mouse).
    pub fn pan_by(&mut self, view: &PreviewView, delta_x: f32, delta_y: f32) {
        self.center = (self.center.0 - delta_x / view.scale, self.center.1 - delta_y / view.scale);
        self.keep_table_in_view();
    }

    /// The middle of the view stays over the reachable table.
    fn keep_table_in_view(&mut self) {
        let (left, top, width, height) = REACHABLE_WORLD_RECT;
        self.center = (self.center.0.clamp(left, left + width), self.center.1.clamp(top, top + height));
    }

    pub fn fit(&mut self) {
        *self = PreviewCamera::default();
    }
}

/// The felt under a preview: the reachable margin in a darker felt, then the table's own
/// 2560x1440 middle in the felt colour with a border.
pub fn draw_preview_felt(frame: &mut Frame, view: &PreviewView, felt: Color, border: Color) {
    let (left, top, width, height) = REACHABLE_WORLD_RECT;
    let (margin_x, margin_y) = view.to_screen(left, top);
    let margin_felt = Color {
        red: felt.red * MARGIN_FELT_BRIGHTNESS,
        green: felt.green * MARGIN_FELT_BRIGHTNESS,
        blue: felt.blue * MARGIN_FELT_BRIGHTNESS,
        alpha: felt.alpha,
    };
    frame.fill_rect(margin_x, margin_y, width * view.scale, height * view.scale, 0.0, margin_felt);
    let (table_x, table_y) = view.to_screen(0.0, 0.0);
    let (table_width, table_height) = (WORLD_W * view.scale, WORLD_H * view.scale);
    frame.fill_rect(table_x, table_y, table_width, table_height, 0.0, felt);
    frame.outline_rect(table_x + 1.0, table_y + 1.0, table_width - 2.0, table_height - 2.0, 0.0, border, 1.0);
}

/// Whether a world point is on the reachable table.
pub fn is_on_reachable_table(world_x: f32, world_y: f32) -> bool {
    let (left, top, width, height) = REACHABLE_WORLD_RECT;
    (left..=left + width).contains(&world_x) && (top..=top + height).contains(&world_y)
}

/// Keeps something with these half extents (world units) wholly on the reachable table, as the
/// table does when players drag things.
pub fn clamp_to_reachable_table(center_x: f32, center_y: f32, half_width: f32, half_height: f32) -> (f32, f32) {
    clamp_world_center(center_x, center_y, half_width, half_height)
}

/// A small white + on the middle of the table.
pub fn draw_center_marker(frame: &mut Frame, view: &PreviewView) {
    let (center_x, center_y) = view.to_screen(WORLD_W / 2.0, WORLD_H / 2.0);
    let arm = 7.0;
    let list = frame.ui.list();
    list.set_color(Color::rgba(1.0, 1.0, 1.0, 0.9));
    list.set_line_width(2.0);
    list.line(&[center_x - arm, center_y, center_x + arm, center_y]);
    list.line(&[center_x, center_y - arm, center_x, center_y + arm]);
    list.set_line_width(1.0);
}

/// A board's every-seat (or shared) placement (0-based), to change it.
fn placement_of(model: &mut EditorModel, index: usize) -> Option<&mut BoardTablePlacement> {
    model.game.game_boards.get_mut(index).and_then(|board| board.table_placement.as_mut())
}

/// A board's placement for one player, to change it.
fn player_placement_of(model: &mut EditorModel, index: usize, player: i64) -> Option<&mut BoardPlayerPlacement> {
    model
        .game
        .game_boards
        .get_mut(index)
        .and_then(|board| board.player_placements.iter_mut().find(|placement| placement.player == player))
}

/// Whether the placement is still there.
fn placement_exists(game: &GameDefinition, placement: PlacementRef) -> bool {
    game.game_boards.get(placement.board_index).is_some_and(|board| match placement.owner {
        PlacementOwner::EverySeat => board.table_placement.is_some(),
        PlacementOwner::Player(player) => board.player_placement_for(player).is_some(),
    })
}

/// The player numbers a board can still be placed separately for.
fn players_without_own_placement(board: &GameBoardDefinition) -> Vec<i64> {
    (1..=MAXIMUM_PREVIEW_PLAYERS as i64).filter(|&player| board.player_placement_for(player).is_none()).collect()
}

/// What the Table Preview's right-click menus do.
#[derive(Debug, Clone, PartialEq)]
enum TableMenuAction {
    /// Opens the board picker: for every seat (or the shared spot), or for one player.
    AddToTable {
        for_player: Option<i64>,
    },
    Turn(PlacementRef),
    SwitchToggleable(PlacementRef),
    SwitchCenterLock(usize),
    /// Gives a board placed for one player to another.
    GiveTo {
        board_index: usize,
        from: i64,
        to: i64,
    },
    /// Turns a seat's copy of an every-seat board into that player's own placement, where it is.
    PlaceSeparately {
        board_index: usize,
        player: i64,
        center: (f32, f32),
        rotation: i64,
    },
    TakeOff(PlacementRef),
}

const ADD_TO_TABLE_DIALOG: &str = "gb:table:add";

impl TableBoardEditor {
    /// How far the preview is zoomed (1 fits the whole table).
    pub fn zoom(&self) -> f32 {
        self.camera.zoom
    }

    /// How many seats the preview shows.
    pub fn player_count(&self) -> usize {
        self.player_count
    }

    /// Development and test hook: how many seats the preview shows.
    pub fn set_player_count(&mut self, count: usize) {
        self.player_count = count.clamp(1, MAXIMUM_PREVIEW_PLAYERS);
    }

    /// Draws the Table Preview in the detail pane. `selected` is the board selected in the list
    /// (0-based); returns a board clicked on the table, to select it.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        selected: Option<usize>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> Option<usize> {
        let (column_x, column_width) = column(x, width);
        let header_y = y + PAD;
        let header_height = self.draw_header(frame, column_x, header_y, column_width);
        let canvas_y = header_y + header_height + 12.0;
        let canvas_height = (y + height - PAD - canvas_y).max(100.0);
        let clicked = self.draw_canvas(frame, model, selected, x + PAD, canvas_y, width - 2.0 * PAD, canvas_height);
        self.show_menu_and_picker(frame, model);
        clicked
    }

    /// Preview players, a hint about the right-click menus, and the zoom controls.
    fn draw_header(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32) -> f32 {
        let palette = frame.palette();
        let row_height = 32.0;
        let label = TextOptions::new().size(12.0).medium().middle_of(row_height).color(palette.on_surface_variant);
        frame.text("Players", x, y, label);
        let choices: Vec<Choice<usize>> =
            (1..=MAXIMUM_PREVIEW_PLAYERS).map(|count| Choice::new(count, count.to_string())).collect();
        self.player_count =
            frame.segmented("gb:players", x + 56.0, y, 228.0, &choices, self.player_count, Some(row_height), false);

        let zoom_controls_width = draw_zoom_controls(frame, &mut self.camera, "gb", x + width, y);
        let zoom_out_x = x + width - zoom_controls_width;

        let hint_x = x + 56.0 + 228.0 + 24.0;
        let hint_width = (zoom_out_x - 16.0 - hint_x).max(0.0);
        let hint = TextOptions::new()
            .size(12.0)
            .width(hint_width)
            .ellipsis()
            .middle_of(row_height)
            .color(palette.on_surface_variant);
        frame.text(
            "Right-click to add or edit boards (or a seat's copy, to place it separately) · wheel zooms · drag the felt to pan",
            hint_x,
            y,
            hint,
        );
        row_height
    }

    /// Puts a board (0-based) on the table: for every seat (or the shared spot) at its default
    /// spot, or for one player where the menu was opened.
    fn place_board(&mut self, frame: &mut Frame, model: &mut EditorModel, index: usize) {
        let sizes = board_sizes(frame, &model.folder_path, &model.game);
        let Some(board) = model.game.game_boards.get_mut(index) else { return };
        match self.adding_for {
            None => {
                if board.table_placement.is_none() {
                    let size = sizes.get(index).copied().unwrap_or((0.0, 0.0));
                    board.table_placement = Some(default_table_placement(board.kind, size));
                }
            }
            Some(player) => {
                if board.kind == BoardKind::Player && board.player_placement_for(player).is_none() {
                    let (x, y) = self.add_at;
                    board.player_placements.push(BoardPlayerPlacement::new(player, f64::from(x), f64::from(y)));
                    // So the new board shows.
                    self.player_count = self.player_count.max(player as usize);
                }
            }
        }
    }

    fn show_menu_and_picker(&mut self, frame: &mut Frame, model: &mut EditorModel) {
        if let Some(menu) = &mut self.menu {
            match menu.show(frame) {
                MenuResponse::Open => {}
                MenuResponse::Closed => self.menu = None,
                MenuResponse::Selected(action) => {
                    self.menu = None;
                    self.run_menu_action(model, action);
                }
            }
        }
        if let Some(picker) = &mut self.picker {
            match picker.show(frame) {
                PickerResponse::Open => {}
                PickerResponse::Cancelled => self.picker = None,
                PickerResponse::Picked(key) => {
                    self.picker = None;
                    if let Ok(index) = key.parse::<usize>() {
                        self.place_board(frame, model, index);
                    }
                }
            }
        }
    }

    fn run_menu_action(&mut self, model: &mut EditorModel, action: TableMenuAction) {
        match action {
            TableMenuAction::AddToTable { for_player } => {
                self.adding_for = for_player;
                let entries: Vec<PickerEntry> = model
                    .game
                    .game_boards
                    .iter()
                    .enumerate()
                    .filter(|(_, board)| match for_player {
                        None => board.table_placement.is_none(),
                        Some(player) => board.kind == BoardKind::Player && board.player_placement_for(player).is_none(),
                    })
                    .map(|(index, board)| {
                        let detail = match (for_player, board.table_placement.is_some()) {
                            (Some(_), true) => "Player board · replaces this player's every-seat copy",
                            (None, _) if board.kind == BoardKind::Player => "Player board · in front of every seat",
                            _ => board_kind_label(board.kind),
                        };
                        PickerEntry::new(index.to_string(), board_display_name(board)).detail(detail)
                    })
                    .collect();
                let (title, empty) = match for_player {
                    None => ("Add To Table".to_string(), "Every board is already on the table."),
                    Some(player) => {
                        (format!("Add For Player {player}"), "Every player board is already placed for this player.")
                    }
                };
                self.picker = Some(ListPickerDialog::new(ADD_TO_TABLE_DIALOG, title, empty, entries));
            }
            TableMenuAction::Turn(placement) => match placement.owner {
                PlacementOwner::EverySeat => {
                    if let Some(placement) = placement_of(model, placement.board_index) {
                        placement.rotate_clockwise();
                    }
                }
                PlacementOwner::Player(player) => {
                    if let Some(placement) = player_placement_of(model, placement.board_index, player) {
                        placement.rotate_clockwise();
                    }
                }
            },
            TableMenuAction::SwitchToggleable(placement) => match placement.owner {
                PlacementOwner::EverySeat => {
                    if let Some(placement) = placement_of(model, placement.board_index) {
                        placement.toggleable = !placement.toggleable;
                    }
                }
                PlacementOwner::Player(player) => {
                    if let Some(placement) = player_placement_of(model, placement.board_index, player) {
                        placement.toggleable = !placement.toggleable;
                    }
                }
            },
            TableMenuAction::GiveTo { board_index, from, to } => {
                let already_has_one = model
                    .game
                    .game_boards
                    .get(board_index)
                    .is_some_and(|board| board.player_placement_for(to).is_some());
                if !already_has_one && let Some(placement) = player_placement_of(model, board_index, from) {
                    placement.player = to;
                    self.player_count = self.player_count.max(to as usize);
                }
            }
            TableMenuAction::PlaceSeparately { board_index, player, center, rotation } => {
                if let Some(board) = model.game.game_boards.get_mut(board_index)
                    && board.player_placement_for(player).is_none()
                {
                    let toggleable = board.table_placement.as_ref().is_some_and(|placement| placement.toggleable);
                    board.player_placements.push(BoardPlayerPlacement {
                        rotation,
                        toggleable,
                        ..BoardPlayerPlacement::new(player, f64::from(center.0), f64::from(center.1))
                    });
                }
            }
            TableMenuAction::SwitchCenterLock(index) => {
                if let Some(placement) = placement_of(model, index) {
                    // Either way the board starts from the middle: unlocked, it moves on from there;
                    // locked, it is forced back there.
                    placement.unlocked_from_center = !placement.unlocked_from_center;
                    placement.x = f64::from(WORLD_W / 2.0);
                    placement.y = f64::from(WORLD_H / 2.0);
                }
            }
            TableMenuAction::TakeOff(placement) => {
                if let Some(board) = model.game.game_boards.get_mut(placement.board_index) {
                    match placement.owner {
                        PlacementOwner::EverySeat => board.table_placement = None,
                        PlacementOwner::Player(player) => {
                            board.player_placements.retain(|placement| placement.player != player)
                        }
                    }
                }
                self.dragged = None;
            }
        }
    }

    /// The right-click menu for one of a board's placements.
    fn board_menu_items(board: &GameBoardDefinition, placement: PlacementRef) -> Vec<MenuItem<TableMenuAction>> {
        let index = placement.board_index;
        let mut items = Vec::new();
        let toggleable = match placement.owner {
            PlacementOwner::EverySeat => board.table_placement.as_ref().is_some_and(|placement| placement.toggleable),
            PlacementOwner::Player(player) => {
                board.player_placement_for(player).is_some_and(|placement| placement.toggleable)
            }
        };
        match placement.owner {
            PlacementOwner::EverySeat if board.kind == BoardKind::Shared => {
                let locked = board.table_placement.as_ref().is_some_and(|placement| !placement.unlocked_from_center);
                items.push(MenuItem::new("Locked to Center", TableMenuAction::SwitchCenterLock(index)).checked(locked));
            }
            PlacementOwner::EverySeat => {}
            PlacementOwner::Player(player) => {
                let free = players_without_own_placement(board);
                let owners: Vec<MenuItem<TableMenuAction>> = (1..=MAXIMUM_PREVIEW_PLAYERS as i64)
                    .map(|to| {
                        MenuItem::new(
                            format!("Player {to}"),
                            TableMenuAction::GiveTo { board_index: index, from: player, to },
                        )
                        .checked(to == player)
                        .disabled(to != player && !free.contains(&to))
                    })
                    .collect();
                items.push(MenuItem::submenu("Belongs To", owners));
            }
        }
        items.push(MenuItem::new("Turn 90°", TableMenuAction::Turn(placement)).icon("rotate_right"));
        items.push(
            MenuItem::new("Can Be Toggled (F12)", TableMenuAction::SwitchToggleable(placement)).checked(toggleable),
        );
        items.push(MenuItem::divider());
        let take_off = match placement.owner {
            PlacementOwner::EverySeat if board.kind == BoardKind::Player => "Take Off Table (Every Seat)".to_string(),
            PlacementOwner::EverySeat => "Take Off Table".to_string(),
            PlacementOwner::Player(player) => format!("Take Off Table (Player {player})"),
        };
        items.push(MenuItem::new(take_off, TableMenuAction::TakeOff(placement)).icon("close"));
        items
    }

    /// The right-click menu for a seat's copy of an every-seat board.
    fn seat_copy_menu_items(board_on_table: &BoardOnTable, player: i64) -> Vec<MenuItem<TableMenuAction>> {
        let rect = board_on_table.world_rect;
        let action = TableMenuAction::PlaceSeparately {
            board_index: (board_on_table.board_index - 1) as usize,
            player,
            center: (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0),
            rotation: board_on_table.rotation,
        };
        vec![MenuItem::new(format!("Place Separately For Player {player}"), action)]
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_canvas(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        selected: Option<usize>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> Option<usize> {
        let palette = frame.palette();
        let (mouse_x, mouse_y) = frame.ui.mouse();
        let mut view = self.camera.view(x, y, width, height);

        // The wheel zooms around the mouse.
        let wheel = frame.ui.input.wheel_y;
        if wheel != 0.0 && view.contains(mouse_x, mouse_y) && !frame.ui.modal_open() {
            self.camera.zoom_at(&view, mouse_x, mouse_y, ZOOM_STEP.powf(wheel));
            frame.ui.consume_wheel();
            view = self.camera.view(x, y, width, height);
        }
        // Dragging the felt (or the middle button anywhere) pans.
        if let Some((last_x, last_y)) = self.pan {
            if frame.ui.input.is_down(1) || frame.ui.input.is_down(3) {
                self.camera.pan_by(&view, mouse_x - last_x, mouse_y - last_y);
                self.pan = Some((mouse_x, mouse_y));
                view = self.camera.view(x, y, width, height);
            } else {
                self.pan = None;
            }
        }

        // Around the table, then the table itself.
        frame.fill_rect(x, y, width, height, 8.0, palette.surface_container_highest);
        frame.ui.push_clip(x, y, width, height);
        let canvas = frame.ui.region("gb:table:canvas", x, y, width, height, RegionOptions::default());
        let game = &model.game;
        let felt = argb_color(TableColor::Felt.resolve(frame.settings, game.table_color.as_deref()));
        let border =
            argb_color(TableColor::PlaymatBorder.resolve(frame.settings, game.playmat_border_color.as_deref()));
        draw_preview_felt(frame, &view, felt, border);

        // The seats, viewer at the bottom middle, as the table lays them out.
        let (state, top_row, bottom_row) = preview_seat_rows(self.player_count);
        // The viewer's seat origin, which their copy of a player board is dragged relative to.
        let viewer_row = if bottom_row.is_empty() { &top_row } else { &bottom_row };
        let viewer_seat_origin_x = viewer_row
            .iter()
            .position(|player| player.id == VIEWER_SEAT_ID)
            .and_then(|seat| seat_origin_xs(viewer_row.len()).get(seat).copied())
            .unwrap_or(WORLD_W / 2.0);
        let sizes = board_sizes(frame, &model.folder_path, game);
        let boards = preview_boards(game, &state, &top_row, &bottom_row, &sizes);
        draw_seat_labels(frame, &view, &top_row, &bottom_row);

        let display_backs = display_card_backs(&model.folder_path, &game.card_backs);
        let mut clicked = None;
        let mut right_clicked: Option<Vec<MenuItem<TableMenuAction>>> = None;
        let mut moved: Option<(PlacementRef, f32, f32)> = None;
        // Which placement a board on the preview table is, and whether it can be edited there: the
        // shared boards, the viewer's copy of every-seat boards, and boards placed for one player.
        // Other seats' copies of every-seat boards are dimmed (right-click places them separately).
        let placement_shown = |board: &BoardOnTable| -> (PlacementRef, bool) {
            let board_index = (board.board_index - 1) as usize;
            let seat = seat_number_of(&board.owner_key);
            match seat {
                Some(player) if game.game_boards[board_index].player_placement_for(player).is_some() => {
                    (PlacementRef { board_index, owner: PlacementOwner::Player(player) }, true)
                }
                Some(player) => (PlacementRef { board_index, owner: PlacementOwner::EverySeat }, player == 1),
                None => (PlacementRef { board_index, owner: PlacementOwner::EverySeat }, true),
            }
        };
        let (others, editable): (Vec<&BoardOnTable>, Vec<&BoardOnTable>) =
            boards.iter().partition(|board| !placement_shown(board).1);
        for board in others.into_iter().chain(editable) {
            let (placement_ref, editable) = placement_shown(board);
            let index = placement_ref.board_index;
            let definition = &game.game_boards[index];
            // A shared board locked to the middle of the table can be picked but not moved.
            let movable = editable
                && (definition.kind == BoardKind::Player
                    || definition.table_placement.as_ref().is_some_and(|placement| placement.unlocked_from_center));
            let mut rect = board.world_rect;
            let dragged = self
                .dragged
                .as_ref()
                .filter(|dragged| movable && dragged.placement == placement_ref)
                .map(|dragged| (dragged.grab_offset, dragged.seat_origin_x));
            let (mut screen_x, mut screen_y) = view.to_screen(rect.x, rect.y);
            let (screen_width, screen_height) = (rect.width * view.scale, rect.height * view.scale);
            let region_id = match placement_ref.owner {
                PlacementOwner::EverySeat if editable => format!("gb:table:board:{index}"),
                PlacementOwner::EverySeat => format!("gb:table:board:{index}:copy:{}", board.owner_key),
                PlacementOwner::Player(player) => format!("gb:table:board:{index}:player:{player}"),
            };
            let cursor = if movable { RegionOptions::hand() } else { RegionOptions::default() };
            let region = frame.ui.region(&region_id, screen_x, screen_y, screen_width, screen_height, cursor);
            if editable && region.pressed {
                let center = (screen_x + screen_width / 2.0, screen_y + screen_height / 2.0);
                self.dragged = Some(DraggedBoard {
                    placement: placement_ref,
                    grab_offset: (mouse_x - center.0, mouse_y - center.1),
                    seat_origin_x: viewer_seat_origin_x,
                });
                clicked = Some(index);
            }
            if region.right_clicked {
                right_clicked = Some(match (editable, seat_number_of(&board.owner_key)) {
                    (false, Some(player)) => TableBoardEditor::seat_copy_menu_items(board, player),
                    _ => {
                        let mut items = TableBoardEditor::board_menu_items(definition, placement_ref);
                        if placement_ref.owner == PlacementOwner::EverySeat
                            && definition.kind == BoardKind::Player
                            && definition.player_placement_for(1).is_none()
                        {
                            items.splice(0..0, TableBoardEditor::seat_copy_menu_items(board, 1));
                        }
                        items
                    }
                });
                if editable {
                    clicked = Some(index);
                }
            }
            if let Some((grab_offset, _)) = dragged {
                let (center_x, center_y) = view.to_world(mouse_x - grab_offset.0, mouse_y - grab_offset.1);
                // Kept on the reachable table.
                let (center_x, center_y) =
                    clamp_to_reachable_table(center_x, center_y, rect.width / 2.0, rect.height / 2.0);
                rect = rect_around((center_x, center_y), (rect.width, rect.height));
                (screen_x, screen_y) = view.to_screen(rect.x, rect.y);
                moved = Some((placement_ref, center_x, center_y));
            }
            if !region.held && self.dragged.as_ref().is_some_and(|dragged| dragged.placement == placement_ref) {
                self.dragged = None;
            }
            let image_path = join_path(&[&model.folder_path, "_playmats", &definition.image_path]);
            let is_selected = editable && selected == Some(index);
            let (image_alpha, outline, outline_width) = match (editable, is_selected) {
                (_, true) => (0.6, SELECTED_OUTLINE, 3.0),
                (true, false) => (0.5, with_alpha(border, 1.0), 2.0),
                (false, _) => (0.22, with_alpha(border, 0.45), 1.0),
            };
            draw_board_image(
                frame,
                &image_path,
                (screen_x, screen_y, screen_width, screen_height),
                board.rotation,
                12.0 * view.scale.max(0.2),
                image_alpha,
                outline,
                outline_width,
            );
            // The zones on it, as card backs and widgets.
            let zone_alpha = if editable { 1.0 } else { 0.35 };
            draw_zones_on_board(
                frame,
                &model.folder_path,
                game,
                definition,
                board,
                rect,
                &view,
                zone_alpha,
                &display_backs,
            );
            if editable {
                let size = 18.0;
                let mut icon_x = screen_x + 8.0;
                let toggleable = match placement_ref.owner {
                    PlacementOwner::EverySeat => {
                        definition.table_placement.as_ref().is_some_and(|placement| placement.toggleable)
                    }
                    PlacementOwner::Player(player) => {
                        definition.player_placement_for(player).is_some_and(|placement| placement.toggleable)
                    }
                };
                if toggleable {
                    frame.icon("visibility", icon_x, screen_y + 8.0, size, with_alpha(Color::WHITE, 0.85));
                    icon_x += size + 4.0;
                }
                if definition.kind == BoardKind::Shared && !movable {
                    frame.icon("lock", icon_x, screen_y + 8.0, size, with_alpha(Color::WHITE, 0.85));
                    icon_x += size + 4.0;
                }
                // Whose a separately placed board is.
                if let PlacementOwner::Player(player) = placement_ref.owner {
                    let label = format!("Player {player}");
                    let text_width = frame.text_width(&label, 11.0, crate::ui::theme::Weight::Medium);
                    let (pill_width, pill_height) = (text_width + 12.0, 18.0);
                    frame.fill_rect(
                        icon_x,
                        screen_y + 8.0,
                        pill_width,
                        pill_height,
                        9.0,
                        Color::rgba(0.0, 0.0, 0.0, 0.55),
                    );
                    let options = TextOptions::new()
                        .size(11.0)
                        .medium()
                        .width(text_width)
                        .middle_of(pill_height)
                        .color(Color::WHITE);
                    frame.text(&label, icon_x + 6.0, screen_y + 8.0, options);
                }
            }
        }
        // The table's centre, over the boards so it always shows.
        draw_center_marker(frame, &view);
        if boards.is_empty() {
            let message = if model.game.game_boards.is_empty() {
                "No game boards yet."
            } else {
                "Nothing on the table yet. Right-click to add a board; with none placed, F12 shows each player's player board in front of them."
            };
            let options =
                TextOptions::new().size(14.0).width(width - 48.0).centered().color(with_alpha(palette.on_surface, 0.8));
            frame.text(message, x + 24.0, y + height / 2.0 - 10.0, options);
        }
        frame.ui.pop_clip();

        // A press on bare felt (or the middle button anywhere) starts panning.
        if (canvas.pressed && clicked.is_none()) || (canvas.hovered && frame.ui.input.was_pressed(3)) {
            self.pan = Some((mouse_x, mouse_y));
        }
        if let Some(items) = right_clicked {
            self.menu = Some(ContextMenu::new("gb:table:menu", mouse_x, mouse_y, items));
        } else if canvas.right_clicked {
            self.add_at = view.to_world(mouse_x, mouse_y);
            let nothing_to_add = model.game.game_boards.iter().all(|board| board.table_placement.is_some());
            let mut items = vec![
                MenuItem::new("Add To Table...", TableMenuAction::AddToTable { for_player: None })
                    .disabled(nothing_to_add),
            ];
            if model.game.game_boards.iter().any(|board| board.kind == BoardKind::Player) {
                let players: Vec<MenuItem<TableMenuAction>> = (1..=MAXIMUM_PREVIEW_PLAYERS as i64)
                    .map(|player| {
                        let nothing_for_them = model.game.game_boards.iter().all(|board| {
                            board.kind != BoardKind::Player || board.player_placement_for(player).is_some()
                        });
                        MenuItem::new(
                            format!("Player {player}..."),
                            TableMenuAction::AddToTable { for_player: Some(player) },
                        )
                        .disabled(nothing_for_them)
                    })
                    .collect();
                items.push(MenuItem::submenu("Add For Player", players));
            }
            self.menu = Some(ContextMenu::new("gb:table:menu", mouse_x, mouse_y, items));
        }
        if let Some((placement_ref, center_x, center_y)) = moved {
            let seat_origin_x = self.dragged.as_ref().map_or(WORLD_W / 2.0, |dragged| dragged.seat_origin_x);
            let index = placement_ref.board_index;
            let kind = model.game.game_boards[index].kind;
            match placement_ref.owner {
                PlacementOwner::Player(player) => {
                    if let Some(placement) = player_placement_of(model, index, player) {
                        (placement.x, placement.y) = (f64::from(center_x), f64::from(center_y));
                    }
                }
                PlacementOwner::EverySeat => {
                    if let Some(placement) = placement_of(model, index) {
                        match kind {
                            BoardKind::Shared => {
                                (placement.x, placement.y) = (f64::from(center_x), f64::from(center_y))
                            }
                            BoardKind::Player => {
                                // The viewer's seat may be one whose layout is mirrored.
                                let offset_x = center_x - seat_origin_x;
                                let mirrored = seat_mirrors_its_boards(seat_origin_x, false);
                                placement.x = f64::from(if mirrored { -offset_x } else { offset_x });
                                placement.y = f64::from(center_y - WORLD_H / 2.0);
                            }
                        }
                    }
                }
            }
        }
        if self.dragged.as_ref().is_some_and(|dragged| !placement_exists(&model.game, dragged.placement)) {
            self.dragged = None;
        }
        clicked
    }
}

/// "You" and "Player N" pills at the middle of each seat's table edge.
fn draw_seat_labels(frame: &mut Frame, view: &PreviewView, top_row: &[PlayerInfo], bottom_row: &[PlayerInfo]) {
    let rows: Vec<(&[PlayerInfo], bool)> = if top_row.is_empty() || bottom_row.is_empty() {
        vec![(if bottom_row.is_empty() { top_row } else { bottom_row }, false)]
    } else {
        vec![(top_row, true), (bottom_row, false)]
    };
    for (players, far_side) in rows {
        let count = players.len().max(1) as f32;
        for (seat, player) in players.iter().enumerate() {
            let label = if player.id == VIEWER_SEAT_ID { "You".to_string() } else { player.name.clone() };
            let text_width = frame.text_width(&label, 11.0, crate::ui::theme::Weight::Medium);
            let (pill_width, pill_height) = (text_width + 12.0, 18.0);
            let (center_x, edge_y) =
                view.to_screen(WORLD_W * (seat as f32 + 0.5) / count, if far_side { 0.0 } else { WORLD_H });
            let pill_y = if far_side { edge_y + 4.0 } else { edge_y - pill_height - 4.0 };
            let pill_x = center_x - pill_width / 2.0;
            frame.fill_rect(pill_x, pill_y, pill_width, pill_height, 9.0, Color::rgba(0.0, 0.0, 0.0, 0.55));
            let options =
                TextOptions::new().size(11.0).medium().width(text_width).middle_of(pill_height).color(Color::WHITE);
            frame.text(&label, pill_x + 6.0, pill_y, options);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_boards_land_in_the_middle_or_in_front_of_the_seat() {
        let shared = default_table_placement(BoardKind::Shared, (1000.0, 500.0));
        assert_eq!((shared.x, shared.y), (1280.0, 720.0));
        let player = default_table_placement(BoardKind::Player, (1000.0, 500.0));
        assert_eq!((player.x, player.y), (0.0, 250.0));
        assert!(!player.toggleable && player.rotation == 0);
    }

    #[test]
    fn zooming_keeps_the_point_under_the_mouse_still_and_panning_stays_on_the_table() {
        let mut camera = PreviewCamera::default();
        let view = camera.view(0.0, 0.0, 1280.0, 720.0);
        // Fitted, the whole reachable table fills the viewport and the table sits in its middle.
        let (left, top, width, height) = REACHABLE_WORLD_RECT;
        assert_eq!(view.to_screen(left, top), (0.0, 0.0));
        assert_eq!(view.to_screen(left + width, top + height), (1280.0, 720.0));
        assert_eq!(view.to_screen(WORLD_W / 2.0, WORLD_H / 2.0), (640.0, 360.0));
        let mouse = (320.0, 180.0);
        let under_mouse = view.to_world(mouse.0, mouse.1);
        camera.zoom_at(&view, mouse.0, mouse.1, 2.0);
        let zoomed = camera.view(0.0, 0.0, 1280.0, 720.0);
        let (x, y) = zoomed.to_world(mouse.0, mouse.1);
        assert!((x - under_mouse.0).abs() < 1e-3 && (y - under_mouse.1).abs() < 1e-3);
        camera.zoom_at(&zoomed, mouse.0, mouse.1, 100.0);
        assert_eq!(camera.zoom, PREVIEW_MAX_ZOOM);
        camera.pan_by(&zoomed, 1e6, 1e6);
        assert_eq!(camera.center, (left, top));
        camera.fit();
        assert_eq!(camera, PreviewCamera::default());
    }

    #[test]
    fn the_reachable_table_reaches_half_a_table_past_each_edge() {
        assert_eq!(REACHABLE_WORLD_RECT, (-1280.0, -720.0, 5120.0, 2880.0));
        assert!(is_on_reachable_table(-1200.0, 2100.0));
        assert!(!is_on_reachable_table(-1300.0, 720.0));
        // A board dragged far off the table stops at the reachable edge, as on the table.
        let (x, y) = clamp_to_reachable_table(-5000.0, 9000.0, 100.0, 50.0);
        assert!(x > -1280.0 + 100.0 - 1.0 && x < 0.0);
        assert!(y > WORLD_H && y < 2160.0 - 50.0 + 1.0);
    }

    #[test]
    fn boards_are_named_after_their_image() {
        assert_eq!(board_display_name(&GameBoardDefinition::new("mats/Blue Mat.png")), "Blue Mat");
        assert_eq!(board_display_name(&GameBoardDefinition::new("board")), "board");
    }

    #[test]
    fn the_preview_seats_every_player_with_boards_shown() {
        let state = preview_state(3);
        assert_eq!(state.players.len(), 3);
        assert_eq!(state.players[0].id, VIEWER_SEAT_ID);
        assert!(state.players.iter().all(|player| player.player_boards_shown) && state.shared_boards_shown);
    }
}
