//! The Game Boards tab (the original's Playmats tab): the game's boards, each a player board
//! (every player gets a copy) or a shared board. A list + detail split under a preview switch:
//! - Board Preview shows the selected board at its authored scale with a draggable reference card,
//!   and the zones placed on it (`board_zone_placement.rs`).
//! - Table Preview shows the whole table, where boards are placed so they are there when the game
//!   loads (`table_board_placement.rs`).
//!
//! Port of `src/ui/screens/gamedef/playmat_tab.lua`, plus GameTable's additions.

use gametable_core::geometry::{WORLD_HEIGHT, WORLD_WIDTH};
use gametable_core::models::{BoardKind, CardDefinition, GameBoardDefinition};

use crate::app::Frame;
use crate::data::display_settings::TableColor;
use crate::data::file_system::join_path;
use crate::graphics::draw_list::{Color, StencilCompare};
use crate::graphics::tessellation::DrawMode;
use crate::screens::gamedef::EditorModel;
use crate::screens::gamedef::board_zone_placement::{BoardScreenRect, ZonePlacementEditor};
use crate::screens::gamedef::common::{PAD, SectionAction, StableKeys, column};
use crate::screens::gamedef::table_board_placement::{
    PreviewCamera, TableBoardEditor, board_display_name, board_kind_label, canvas_rect, draw_center_marker,
};
use crate::screens::token_choosers::{ChooserResponse, TokenChooser};
use crate::table::board_layout::board_size;
use crate::table::card_render::{CARD_HEIGHT, CARD_WIDTH, CardDraw};
use crate::ui::components::{Reorder, argb_color, move_item};
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::panel::{Pane, clamp_selection};
use crate::ui::theme::with_alpha;
use crate::ui::widgets::{ButtonOptions, ButtonVariant, Choice, ScrollbarStyle, SliderOptions};

const LIST_WIDTH: f32 = 300.0;
const ROW_HEIGHT: f32 = 56.0;
const MODE_BAR_HEIGHT: f32 = 64.0;

/// A standard playing card, purely as a fixed size reference.
fn reference_card() -> CardDefinition {
    CardDefinition {
        suit: Some("hearts".to_string()),
        color_hex: Some("#D32F2F".to_string()),
        ..CardDefinition::new("", "A")
    }
}

enum ChooserTarget {
    Add,
    Replace(u64),
}

/// What the detail pane previews.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewMode {
    /// The selected board and the zones placed on it.
    Board,
    /// The whole table and the boards placed on it.
    Table,
}

pub struct GameBoardTab {
    keys: StableKeys,
    selected: Option<u64>,
    preview: PreviewMode,
    /// The reference card's position in the preview, as canvas fractions (never saved).
    card_fraction: (f32, f32),
    chooser: Option<(TokenChooser, ChooserTarget)>,
    pending_remove: Option<usize>,
    zone_placement: ZonePlacementEditor,
    table_editor: TableBoardEditor,
}

impl Default for GameBoardTab {
    fn default() -> Self {
        GameBoardTab::new()
    }
}

impl GameBoardTab {
    pub fn new() -> GameBoardTab {
        GameBoardTab {
            keys: StableKeys::default(),
            selected: None,
            preview: PreviewMode::Board,
            card_fraction: (0.5, 0.72),
            chooser: None,
            pending_remove: None,
            zone_placement: ZonePlacementEditor::default(),
            table_editor: TableBoardEditor::default(),
        }
    }

    /// Development hook: selects the first board.
    pub fn select_first(&mut self, model: &EditorModel) {
        self.keys.sync(model.game.game_boards.len());
        if !model.game.game_boards.is_empty() {
            self.selected = Some(self.keys.key(0));
        }
    }

    /// Development hook: switches to the Table Preview.
    pub fn show_table_preview_for_development(&mut self) {
        self.preview = PreviewMode::Table;
    }

    /// Boards keep their images in the game's `_playmats/` folder (named from before boards).
    fn board_image_path(folder: &str, board: &GameBoardDefinition) -> String {
        join_path(&[folder, "_playmats", &board.image_path])
    }

    fn draw_list(&mut self, frame: &mut Frame, model: &mut EditorModel, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let action = SectionAction { label: "Add Board", icon: Some("add"), id: "gb:add", disabled: false };
        let (header_height, add) = frame.section_header(
            "gb:list",
            "Game Boards",
            x + 16.0,
            y + 12.0,
            width - 32.0,
            Some(model.game.game_boards.len()),
            None,
            Some(action),
        );
        if add {
            self.chooser =
                Some((TokenChooser::for_folder(Some(model.folder_path.clone()), "_playmats"), ChooserTarget::Add));
        }
        let list_y = y + 12.0 + header_height + 8.0;
        let list_height = y + height - list_y;
        let offset = frame.begin_scroll("gb:list", x, list_y, width, list_height);
        let reorder = Reorder::begin(frame, "gb:boards");
        let mut row_y = list_y - offset;
        let thumb = 40.0;
        for index in 0..model.game.game_boards.len() {
            let key = self.keys.key(index);
            let board = &model.game.game_boards[index];
            let (row_x, row_width) = (x + 8.0, width - 16.0);
            let is_selected = self.selected == Some(key);
            let region = frame.ui.region(
                &format!("gb:row:{key}"),
                row_x + 40.0,
                row_y,
                row_width - 40.0,
                ROW_HEIGHT,
                RegionOptions::hand(),
            );
            if is_selected {
                frame.fill_rect(row_x, row_y, row_width, ROW_HEIGHT, 8.0, palette.secondary_container);
            } else if region.hovered {
                frame.fill_rect(row_x, row_y, row_width, ROW_HEIGHT, 8.0, with_alpha(palette.on_surface, 0.05));
            }
            reorder.handle(frame, index, row_x, row_y + (ROW_HEIGHT - 40.0) / 2.0, row_y, ROW_HEIGHT);
            let path = GameBoardTab::board_image_path(&model.folder_path, board);
            frame.image_preview_box(Some(&path), row_x + 42.0, row_y + (ROW_HEIGHT - thumb) / 2.0, thumb, thumb);
            let foreground = if is_selected { palette.on_secondary_container } else { palette.on_surface };
            let text_x = row_x + 42.0 + thumb + 12.0;
            let mut name = TextOptions::new().width(row_x + row_width - 8.0 - text_x).ellipsis().color(foreground);
            if is_selected {
                name = name.medium();
            }
            frame.text(&board_display_name(board), text_x, row_y + 9.0, name);
            let percent = format!("{}%", (board.scale * 100.0 + 0.5).floor() as i64);
            let details = format!("{} · {percent}", board_kind_label(board.kind));
            let on_table_icon = 16.0;
            let options = TextOptions::new()
                .width(row_x + row_width - 8.0 - text_x - on_table_icon - 4.0)
                .ellipsis()
                .size(12.0)
                .color(palette.on_surface_variant);
            frame.text(&details, text_x, row_y + 30.0, options);
            if board.is_on_table() {
                let icon_x = row_x + row_width - 8.0 - on_table_icon;
                frame.icon("layers", icon_x, row_y + 30.0, on_table_icon, palette.on_surface_variant);
            }
            if region.clicked {
                self.selected = Some(key);
                frame.ui.blur();
            }
            row_y += ROW_HEIGHT + 2.0;
        }
        if let Some((from, to)) = reorder.finish(frame, x + 8.0, width - 16.0) {
            move_item(&mut model.game.game_boards, from, to);
            self.keys.moved(from, to);
        }
        frame.end_scroll(
            "gb:list",
            model.game.game_boards.len() as f32 * (ROW_HEIGHT + 2.0) + 12.0,
            &ScrollbarStyle::default(),
        );
    }

    /// `T:drawTablePreview`: the board's size on the table (a shared board centred, a player
    /// board just below the middle line, in front of the viewer) and the draggable reference card. Returns where the board was
    /// drawn, for the zones placed on it, and whether the preview was right-clicked.
    #[allow(clippy::too_many_arguments)]
    fn draw_board_preview(
        &mut self,
        frame: &mut Frame,
        model: &EditorModel,
        board: &GameBoardDefinition,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> (BoardScreenRect, bool) {
        let game = &model.game;
        let (canvas_x, canvas_y, canvas_width, canvas_height, scale) = canvas_rect(x, y, width, height);
        let felt = argb_color(TableColor::Felt.resolve(frame.settings, game.table_color.as_deref()));
        let border =
            argb_color(TableColor::PlaymatBorder.resolve(frame.settings, game.playmat_border_color.as_deref()));
        frame.fill_rect(canvas_x, canvas_y, canvas_width, canvas_height, 0.0, felt);
        frame.outline_rect(canvas_x + 1.0, canvas_y + 1.0, canvas_width - 2.0, canvas_height - 2.0, 0.0, border, 1.0);
        let canvas = frame.ui.region(
            "gb:preview:canvas",
            canvas_x,
            canvas_y,
            canvas_width,
            canvas_height,
            RegionOptions::default(),
        );

        // A board's own size is its image's pixel size at 100%, falling back while it loads.
        let path = GameBoardTab::board_image_path(&model.folder_path, board);
        let texture = frame.images.get(&path);
        let (mat_width, mat_height) = board_size(board, texture.map(|texture| (texture.width, texture.height)));
        let (world_width, world_height) = (WORLD_WIDTH as f32, WORLD_HEIGHT as f32);
        let mat_left = (world_width - mat_width) / 2.0;
        let mat_top = match board.kind {
            BoardKind::Shared => (world_height - mat_height) / 2.0,
            // In front of the viewer's seat, from the middle line, as boards first land on the table.
            BoardKind::Player => world_height / 2.0,
        };
        let (mat_x, mat_y, mat_screen_width, mat_screen_height) =
            (canvas_x + mat_left * scale, canvas_y + mat_top * scale, mat_width * scale, mat_height * scale);
        let radius = 16.0;
        if let Some(texture) = texture {
            let list = frame.ui.list();
            list.stencil(1, |list| {
                list.rectangle(DrawMode::Fill, mat_x, mat_y, mat_screen_width, mat_screen_height, radius, radius)
            });
            list.set_stencil_test(Some((StencilCompare::Greater, 0)));
            let cover = (mat_screen_width / texture.width).max(mat_screen_height / texture.height);
            list.set_color(Color::rgba(1.0, 1.0, 1.0, 0.5));
            list.draw_texture(
                texture,
                None,
                mat_x + (mat_screen_width - texture.width * cover) / 2.0,
                mat_y + (mat_screen_height - texture.height * cover) / 2.0,
                0.0,
                cover,
                cover,
                0.0,
                0.0,
            );
            list.set_stencil_test(None);
        }
        let list = frame.ui.list();
        list.set_color(border);
        list.set_line_width(2.0);
        list.rectangle(
            DrawMode::Line,
            mat_x + 1.0,
            mat_y + 1.0,
            mat_screen_width - 2.0,
            mat_screen_height - 2.0,
            radius,
            radius,
        );
        list.set_line_width(1.0);

        // The draggable reference card.
        let (card_width, card_height) = (CARD_WIDTH * scale, CARD_HEIGHT * scale);
        let mut center =
            (canvas_x + self.card_fraction.0 * canvas_width, canvas_y + self.card_fraction.1 * canvas_height);
        let region = frame.ui.region(
            "gb:refcard",
            center.0 - card_width / 2.0,
            center.1 - card_height / 2.0,
            card_width,
            card_height,
            RegionOptions::hand(),
        );
        if region.held {
            let (mouse_x, mouse_y) = frame.ui.mouse();
            let (half_width, half_height) = ((card_width / 2.0) / canvas_width, (card_height / 2.0) / canvas_height);
            self.card_fraction = (
                ((mouse_x - canvas_x) / canvas_width).clamp(half_width, 1.0 - half_width),
                ((mouse_y - canvas_y) / canvas_height).clamp(half_height, 1.0 - half_height),
            );
            center = (canvas_x + self.card_fraction.0 * canvas_width, canvas_y + self.card_fraction.1 * canvas_height);
        }
        draw_center_marker(frame, &PreviewCamera::default().view(x, y, width, height));
        let card = reference_card();
        frame.draw_card(&CardDraw::new(Some(&card), true, center.0, center.1, &[], None).scale(scale));
        let board_rect = BoardScreenRect {
            x: mat_x,
            y: mat_y,
            width: mat_screen_width,
            height: mat_screen_height,
            preview_scale: scale,
        };
        (board_rect, canvas.right_clicked)
    }

    /// `T:drawDetail`: no scroll here; the preview fills whatever space is left.
    #[allow(clippy::too_many_arguments)]
    fn draw_detail(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        index: usize,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let key = self.keys.key(index);
        let id = format!("gb:{key}");
        let (column_x, column_width) = column(x, width);
        let mut row_y = y + PAD;
        let thumb = 64.0;
        let path = GameBoardTab::board_image_path(&model.folder_path, &model.game.game_boards[index]);
        frame.image_preview_box(Some(&path), column_x, row_y, thumb, thumb);
        let choose_id = format!("{id}:choose");
        let choose_options = ButtonOptions::variant(ButtonVariant::Outlined).id(&choose_id);
        let choose_width = frame.button_width("Choose Image...", &choose_options);
        if frame.button(
            "Choose Image...",
            column_x + thumb + 12.0,
            row_y,
            Some(choose_width),
            Some(40.0),
            &choose_options,
        ) {
            self.chooser = Some((
                TokenChooser::for_folder(Some(model.folder_path.clone()), "_playmats"),
                ChooserTarget::Replace(key),
            ));
        }
        let delete_id = format!("{id}:del");
        let delete_options = ButtonOptions::variant(ButtonVariant::Text).icon("delete_outline").id(&delete_id);
        let delete_width = frame.button_width("Delete Board", &delete_options);
        if frame.button(
            "Delete Board",
            column_x + column_width - delete_width,
            row_y,
            Some(delete_width),
            Some(40.0),
            &delete_options,
        ) {
            self.pending_remove = Some(index);
        }
        row_y += thumb + 16.0;

        let options = TextOptions::new().size(12.0).medium().color(frame.palette().on_surface_variant);
        frame.text("Kind", column_x, row_y, options);
        let kinds = [Choice::new(BoardKind::Player, "Player Board"), Choice::new(BoardKind::Shared, "Shared Board")];
        let kind = model.game.game_boards[index].kind;
        model.game.game_boards[index].kind =
            frame.segmented(&format!("{id}:kind"), column_x, row_y + 18.0, 320.0, &kinds, kind, None, false);
        row_y += 18.0 + 40.0 + 16.0;

        let format = |value: f32| format!("{}%", (value * 100.0 + 0.5).floor() as i64);
        let options = SliderOptions { step: Some(0.01), disabled: false };
        let (scale, _) = frame.slider_row(
            &format!("{id}:scale"),
            "Scale",
            column_x,
            row_y,
            column_width.min(400.0),
            model.game.game_boards[index].scale as f32,
            0.10,
            2.00,
            &options,
            Some(&format),
        );
        if (scale as f64 - model.game.game_boards[index].scale).abs() > 1e-6 {
            model.game.game_boards[index].scale = scale as f64;
        }
        row_y += 64.0 + 16.0;
        let label_height = frame.group_label("Board Preview", column_x, row_y, column_width);
        let hint =
            TextOptions::new().size(12.0).width(column_width).ellipsis().color(frame.palette().on_surface_variant);
        frame.text(
            "Right-click the preview to add a zone. Drag a zone to move it, or off the board to remove it.",
            column_x,
            row_y + label_height + 2.0,
            hint,
        );
        row_y += label_height + 2.0 + 20.0;
        // The preview spans the whole pane and fills the height down to the gutter.
        let preview_height = (y + height - PAD - row_y).max(100.0);
        let board = model.game.game_boards[index].clone();
        let (board_rect, right_clicked) =
            self.draw_board_preview(frame, model, &board, x + PAD, row_y, width - 2.0 * PAD, preview_height);
        self.zone_placement.draw_placed_zones(frame, model, index, board_rect);
        if right_clicked {
            let (mouse_x, mouse_y) = frame.ui.mouse();
            self.zone_placement.open_menu(model, index, board_rect, mouse_x, mouse_y);
        }
        self.zone_placement.show_menu_and_picker(frame, model, index);
    }

    pub fn draw(&mut self, frame: &mut Frame, model: &mut EditorModel, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        self.keys.sync(model.game.game_boards.len());
        frame.fill_rect(x, y, width, MODE_BAR_HEIGHT, 0.0, palette.surface_container);
        let options = TextOptions::new().size(12.0).medium().color(palette.on_surface_variant);
        frame.text("Preview", x + 16.0, y + 10.0, options);
        let choices =
            [Choice::new(PreviewMode::Board, "Board Preview"), Choice::new(PreviewMode::Table, "Table Preview")];
        self.preview = frame.segmented("gb:mode", x + 16.0, y + 28.0, 320.0, &choices, self.preview, Some(32.0), false);
        frame.fill_rect(x, y + MODE_BAR_HEIGHT, width, 1.0, 0.0, palette.outline_variant);

        self.pending_remove = None;
        let keys: Vec<u64> = (0..model.game.game_boards.len()).map(|index| self.keys.key(index)).collect();
        clamp_selection(&mut self.selected, &keys);
        let selected_index = self.selected.and_then(|key| self.keys.index_of(key));
        let list_y = y + MODE_BAR_HEIGHT + 1.0;
        let table_preview = self.preview == PreviewMode::Table && !model.game.game_boards.is_empty();
        let mut clicked_on_table = None;
        frame.list_detail(
            x,
            list_y,
            width,
            height - MODE_BAR_HEIGHT - 1.0,
            LIST_WIDTH,
            selected_index.is_some() || table_preview,
            "Add a game board to get started",
            |frame, pane| match pane {
                Pane::List(rect) => self.draw_list(frame, model, rect.x, rect.y, rect.width, rect.height),
                Pane::Detail(rect) if table_preview => {
                    clicked_on_table =
                        self.table_editor.draw(frame, model, selected_index, rect.x, rect.y, rect.width, rect.height);
                }
                Pane::Detail(rect) => {
                    if let Some(index) = selected_index {
                        self.draw_detail(frame, model, index, rect.x, rect.y, rect.width, rect.height);
                    }
                }
            },
        );
        if let Some(index) = clicked_on_table {
            self.selected = Some(self.keys.key(index));
        }
        if let Some(index) = self.pending_remove.take() {
            model.game.game_boards.remove(index);
            self.keys.removed(index);
            let count = model.game.game_boards.len();
            self.selected = (count > 0).then(|| self.keys.key(index.min(count - 1)));
        }
        if let Some((chooser, _)) = &mut self.chooser {
            match chooser.show(frame) {
                ChooserResponse::Open => {}
                ChooserResponse::Cancelled => self.chooser = None,
                ChooserResponse::Picked(file) => {
                    let Some((_, target)) = self.chooser.take() else { return };
                    match target {
                        ChooserTarget::Add => {
                            model.game.game_boards.push(GameBoardDefinition::new(file));
                            self.keys.sync(model.game.game_boards.len());
                            self.selected = Some(self.keys.key(model.game.game_boards.len() - 1));
                        }
                        ChooserTarget::Replace(key) => {
                            if let Some(index) = self.keys.index_of(key) {
                                model.game.game_boards[index].image_path = file;
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::harness::UiHarness;
    use gametable_core::models::{BoardZonePlacement, GameDefinition, ZoneDefinition};

    fn model_with_one_board() -> EditorModel {
        let mut game = GameDefinition::new("game", "Game");
        game.zones.push(ZoneDefinition::new("deck", "Deck"));
        game.zones.push(ZoneDefinition { shared: true, ..ZoneDefinition::new("market", "Market") });
        game.game_boards.push(GameBoardDefinition::new("missing.png"));
        EditorModel { folder_path: String::new(), game, card_keys: StableKeys::default() }
    }

    /// Presses and releases a mouse button at a point, a frame each.
    fn click_at(harness: &mut UiHarness, draw: &mut dyn FnMut(&mut Frame), (x, y): (f32, f32), button: u8) {
        harness.move_mouse(x, y);
        harness.frame(|frame| draw(frame));
        harness.press(x, y, button, 1);
        harness.frame(|frame| draw(frame));
        harness.release(x, y, button);
        harness.frame(|frame| draw(frame));
        harness.frame(|frame| draw(frame));
    }

    fn drag(harness: &mut UiHarness, draw: &mut dyn FnMut(&mut Frame), from: (f32, f32), to: (f32, f32)) {
        harness.move_mouse(from.0, from.1);
        harness.frame(|frame| draw(frame));
        harness.press(from.0, from.1, 1, 1);
        harness.frame(|frame| draw(frame));
        harness.move_mouse(to.0, to.1);
        harness.frame(|frame| draw(frame));
        harness.release(to.0, to.1, 1);
        harness.frame(|frame| draw(frame));
        harness.frame(|frame| draw(frame));
    }

    #[test]
    fn add_a_zone_from_the_right_click_menu_then_drag_it_off() {
        let mut harness = UiHarness::new();
        let mut model = model_with_one_board();
        let mut tab = GameBoardTab::new();
        tab.keys.sync(1);
        tab.selected = Some(tab.keys.key(0));
        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 0.0, 1280.0, 800.0);

        // Right-click the preview off the board (above it: a player board starts at the middle line):
        // "Add Zone..."
        // lists only the player zone, and the picked zone goes in the board's centre.
        harness.frame(|frame| draw(frame));
        let (preview_x, preview_y) = harness.region_center("gb:preview:canvas").unwrap();
        click_at(&mut harness, &mut draw, (preview_x, preview_y - 100.0), 2);
        harness.click_region("gb:zones:menu:menu1:1:Add Zone...", &mut draw);
        harness.frame(|frame| draw(frame));
        assert!(harness.region_center("gb:zones:add:row:market").is_none());
        harness.click_region("gb:zones:add:row:deck", &mut draw);
        harness.frame(|frame| draw(frame));
        let (zone_x, zone_y) = harness.region_center("gb:zone:deck").expect("placed zone drawn on the board");
        assert_eq!(model.game.game_boards[0].zone_placements, vec![BoardZonePlacement::new("deck", 0.5, 0.5)]);

        // Dragging it a little moves it; dragging it off the board takes it off.
        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 0.0, 1280.0, 800.0);
        drag(&mut harness, &mut draw, (zone_x, zone_y), (zone_x + 30.0, zone_y));
        let moved = &model.game.game_boards[0].zone_placements[0];
        assert!(moved.x > 0.5 && (moved.y - 0.5).abs() < 1e-3, "{moved:?}");

        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 0.0, 1280.0, 800.0);
        harness.frame(|frame| draw(frame));
        let (zone_x, zone_y) = harness.region_center("gb:zone:deck").unwrap();
        drag(&mut harness, &mut draw, (zone_x, zone_y), (5.0, 5.0));
        assert!(model.game.game_boards[0].zone_placements.is_empty());

        // With the zone placed where the preview is right-clicked on the board itself.
        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 0.0, 1280.0, 800.0);
        harness.frame(|frame| draw(frame));
        let (preview_x, preview_y) = harness.region_center("gb:preview:canvas").unwrap();
        let on_board = (preview_x + 20.0, preview_y + 30.0);
        click_at(&mut harness, &mut draw, on_board, 2);
        harness.click_region("gb:zones:menu:menu1:1:Add Zone...", &mut draw);
        harness.click_region("gb:zones:add:row:deck", &mut draw);
        let placed = &model.game.game_boards[0].zone_placements[0];
        // Right of the middle, and not in the board's centre: where it was clicked.
        assert!(placed.x > 0.5 && placed.x < 1.0 && placed.y > 0.0 && placed.y < 1.0, "{placed:?}");
    }

    #[test]
    fn add_boards_to_the_table_turn_toggle_lock_and_take_them_off() {
        use crate::table::board_layout::PLAYER_BOARD_FALLBACK_SIZE;
        let mut harness = UiHarness::new();
        let mut model = model_with_one_board();
        model
            .game
            .game_boards
            .push(GameBoardDefinition { kind: BoardKind::Shared, ..GameBoardDefinition::new("centre.png") });
        let mut tab = GameBoardTab::new();
        tab.show_table_preview_for_development();
        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 0.0, 1280.0, 800.0);

        // Right-click the table, "Add To Table...", pick each board in turn.
        harness.frame(|frame| draw(frame));
        let (canvas_x, canvas_y) = harness.region_center("gb:table:canvas").unwrap();
        let corner = (canvas_x - 300.0, canvas_y - 150.0);
        for key in ["1", "0"] {
            click_at(&mut harness, &mut draw, corner, 2);
            harness.click_region("gb:table:menu:menu1:1:Add To Table...", &mut draw);
            harness.click_region(&format!("gb:table:add:row:{key}"), &mut draw);
        }
        let placement = |model: &EditorModel, index: usize| model.game.game_boards[index].table_placement.clone();
        harness.frame(|frame| draw(frame));
        let player = placement(&model, 0).expect("player board placed");
        assert_eq!((player.x, player.y), (0.0, f64::from(PLAYER_BOARD_FALLBACK_SIZE.1 / 2.0)));
        let shared = placement(&model, 1).expect("shared board placed");
        assert!(!shared.unlocked_from_center, "a shared board starts locked to the middle");

        // A locked shared board doesn't move when dragged (its upper half: the player board lies
        // over the lower half and, drawn on top, would take the mouse).
        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 0.0, 1280.0, 800.0);
        harness.frame(|frame| draw(frame));
        let (board_x, board_center_y) = harness.region_center("gb:table:board:1").unwrap();
        let on_shared = (board_x, board_center_y - 40.0);
        drag(&mut harness, &mut draw, on_shared, (on_shared.0 + 40.0, on_shared.1));
        let shared = placement(&model, 1).unwrap();
        assert_eq!((shared.x, shared.y), (1280.0, 720.0));

        // Its menu unlocks it, turns it and makes it toggleable; then dragging moves it.
        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 0.0, 1280.0, 800.0);
        for label in ["1:Locked to Center", "2:Turn 90°", "3:Can Be Toggled (F12)"] {
            click_at(&mut harness, &mut draw, on_shared, 2);
            harness.click_region(&format!("gb:table:menu:menu1:{label}"), &mut draw);
        }
        drag(&mut harness, &mut draw, on_shared, (on_shared.0 + 40.0, on_shared.1));
        let shared = placement(&model, 1).unwrap();
        assert_eq!((shared.rotation, shared.toggleable, shared.unlocked_from_center), (90, true, true));
        assert!(shared.x > 1300.0 && (shared.y - 720.0).abs() < 1e-3, "{shared:?}");
        assert_eq!(tab.selected, Some(tab.keys.key(1)));

        // Locking it again puts it back in the middle; Take Off Table removes it.
        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 0.0, 1280.0, 800.0);
        harness.frame(|frame| draw(frame));
        let (board_x, board_y) = harness.region_center("gb:table:board:1").unwrap();
        let on_shared = (board_x, board_y - 40.0);
        click_at(&mut harness, &mut draw, on_shared, 2);
        harness.click_region("gb:table:menu:menu1:1:Locked to Center", &mut draw);
        assert_eq!(
            placement(&model, 1).map(|shared| (shared.x, shared.y, shared.unlocked_from_center)),
            Some((1280.0, 720.0, false))
        );
        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 0.0, 1280.0, 800.0);
        click_at(&mut harness, &mut draw, on_shared, 2);
        harness.click_region("gb:table:menu:menu1:5:Take Off Table", &mut draw);
        assert!(placement(&model, 1).is_none());
        assert!(placement(&model, 0).is_some());
    }

    #[test]
    fn place_a_seat_copy_separately_give_it_away_and_add_a_board_for_one_player() {
        use crate::table::board_layout::PLAYER_BOARD_FALLBACK_SIZE;
        let mut harness = UiHarness::new();
        let mut model = model_with_one_board();
        let mut tab = GameBoardTab::new();
        tab.show_table_preview_for_development();
        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 0.0, 1280.0, 800.0);
        harness.frame(|frame| draw(frame));
        let (canvas_x, canvas_y) = harness.region_center("gb:table:canvas").unwrap();
        let corner = (canvas_x - 300.0, canvas_y - 150.0);
        click_at(&mut harness, &mut draw, corner, 2);
        harness.click_region("gb:table:menu:menu1:1:Add To Table...", &mut draw);
        harness.click_region("gb:table:add:row:0", &mut draw);
        harness.frame(|frame| draw(frame));

        // Player 2's copy (across the table) becomes their own placement, right where it was.
        let copy = harness.region_center("gb:table:board:0:copy:seat2").unwrap();
        click_at(&mut harness, &mut draw, copy, 2);
        harness.click_region("gb:table:menu:menu1:1:Place Separately For Player 2", &mut draw);
        harness.frame(|frame| draw(frame));
        let own = harness.region_center("gb:table:board:0:player:2").expect("player 2's own board");
        drag(&mut harness, &mut draw, own, (own.0 - 40.0, own.1));
        let placements = model.game.game_boards[0].player_placements.clone();
        assert_eq!(placements.len(), 1);
        let placed = &placements[0];
        assert_eq!((placed.player, placed.rotation), (2, 180));
        assert!(placed.x < 1280.0, "dragged left: {placed:?}");
        assert!((placed.y - f64::from(720.0 - PLAYER_BOARD_FALLBACK_SIZE.1 / 2.0)).abs() < 1e-3, "{placed:?}");
        assert!(model.game.game_boards[0].table_placement.is_some(), "everyone else keeps the every-seat copy");

        // Give it to player 3 (the preview grows to show them), then take it off again.
        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 0.0, 1280.0, 800.0);
        click_at(&mut harness, &mut draw, (own.0 - 40.0, own.1), 2);
        harness.click_region("gb:table:menu:menu1:1:Belongs To", &mut draw);
        harness.click_region("gb:table:menu:menu2:3:Player 3", &mut draw);
        harness.frame(|frame| draw(frame));
        let theirs = harness.region_center("gb:table:board:0:player:3").expect("now player 3's");
        click_at(&mut harness, &mut draw, theirs, 2);
        harness.click_region("gb:table:menu:menu1:5:Take Off Table (Player 3)", &mut draw);
        assert!(model.game.game_boards[0].player_placements.is_empty());

        // "Add For Player" puts a board where the table was right-clicked, for that player only.
        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 0.0, 1280.0, 800.0);
        click_at(&mut harness, &mut draw, corner, 2);
        harness.click_region("gb:table:menu:menu1:2:Add For Player", &mut draw);
        harness.click_region("gb:table:menu:menu2:4:Player 4...", &mut draw);
        harness.click_region("gb:table:add:row:0", &mut draw);
        let placements = model.game.game_boards[0].player_placements.clone();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].player, 4);
        assert!(placements[0].x < 1280.0 && placements[0].y < 720.0, "{:?}", placements[0]);
        assert_eq!(tab.table_editor.player_count(), 4);
    }

    #[test]
    fn the_wheel_zooms_the_table_preview_and_the_percentage_fits_it_again() {
        let mut harness = UiHarness::new();
        let mut model = model_with_one_board();
        let mut tab = GameBoardTab::new();
        tab.show_table_preview_for_development();
        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 0.0, 1280.0, 800.0);
        harness.frame(|frame| draw(frame));
        let (canvas_x, canvas_y) = harness.region_center("gb:table:canvas").unwrap();
        harness.move_mouse(canvas_x, canvas_y);
        harness.frame(|frame| draw(frame));
        harness.wheel(2.0);
        harness.frame(|frame| draw(frame));
        harness.click_region("gb:zoom:out", &mut draw);
        assert!((tab.table_editor.zoom() - 1.2).abs() < 1e-4, "{}", tab.table_editor.zoom());
        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 0.0, 1280.0, 800.0);
        harness.click_region("gb:zoom:fit", &mut draw);
        assert_eq!(tab.table_editor.zoom(), 1.0);
    }
}
