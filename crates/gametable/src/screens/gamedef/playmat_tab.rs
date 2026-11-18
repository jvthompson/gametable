//! The Playmats tab: the playmat images and whether the game uses one shared centrepiece mat or
//! one mat per player. A mode switch above a list + detail split, with a table preview showing
//! the selected playmat at its authored scale and a draggable reference card for comparison.
//! Port of `src/ui/screens/gamedef/playmat_tab.lua`, plus GameTable's zones placed on playmats
//! (`playmat_zone_placement.rs`).

use gametable_core::geometry::{WORLD_HEIGHT, WORLD_WIDTH};
use gametable_core::models::{CardDefinition, PlaymatDefinition, PlaymatType};

use crate::app::Frame;
use crate::data::display_settings::TableColor;
use crate::data::file_system::join_path;
use crate::graphics::draw_list::{Color, StencilCompare};
use crate::graphics::tessellation::DrawMode;
use crate::screens::gamedef::EditorModel;
use crate::screens::gamedef::common::{PAD, SectionAction, StableKeys, column};
use crate::screens::gamedef::playmat_zone_placement::{MatScreenRect, ZonePlacementEditor};
use crate::screens::token_choosers::{ChooserResponse, TokenChooser};
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
/// Mat sizes before scaling, as on the table (`SINGLE_MARKER_W/H`, `PLAYMAT_W/H`).
const PIXELS_PER_INCH: f32 = CARD_WIDTH / 2.5;
const SHARED_SIZE: (f32, f32) = (1280.0, 720.0);
const INDIVIDUAL_SIZE: (f32, f32) = (24.0 * PIXELS_PER_INCH, 12.0 * PIXELS_PER_INCH);

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

pub struct PlaymatTab {
    keys: StableKeys,
    selected: Option<u64>,
    /// The reference card's position in the preview, as canvas fractions (never saved).
    card_fraction: (f32, f32),
    chooser: Option<(TokenChooser, ChooserTarget)>,
    pending_remove: Option<usize>,
    zone_placement: ZonePlacementEditor,
}

impl Default for PlaymatTab {
    fn default() -> Self {
        PlaymatTab::new()
    }
}

impl PlaymatTab {
    pub fn new() -> PlaymatTab {
        PlaymatTab {
            keys: StableKeys::default(),
            selected: None,
            card_fraction: (0.5, 0.72),
            chooser: None,
            pending_remove: None,
            zone_placement: ZonePlacementEditor::default(),
        }
    }

    /// Development hook: selects the first playmat.
    pub fn select_first(&mut self, model: &EditorModel) {
        self.keys.sync(model.game.playmats.len());
        if !model.game.playmats.is_empty() {
            self.selected = Some(self.keys.key(0));
        }
    }

    fn mat_path(folder: &str, playmat: &PlaymatDefinition) -> String {
        join_path(&[folder, "_playmats", &playmat.image_path])
    }

    fn draw_list(&mut self, frame: &mut Frame, model: &mut EditorModel, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let action = SectionAction { label: "Add Playmat", icon: Some("add"), id: "pt:add", disabled: false };
        let (header_height, add) = frame.section_header(
            "pt:list",
            "Playmats",
            x + 16.0,
            y + 12.0,
            width - 32.0,
            Some(model.game.playmats.len()),
            None,
            Some(action),
        );
        if add {
            self.chooser =
                Some((TokenChooser::for_folder(Some(model.folder_path.clone()), "_playmats"), ChooserTarget::Add));
        }
        let list_y = y + 12.0 + header_height + 8.0;
        let list_height = y + height - list_y;
        let offset = frame.begin_scroll("pt:list", x, list_y, width, list_height);
        let reorder = Reorder::begin(frame, "pt:playmats");
        let mut row_y = list_y - offset;
        let thumb = 40.0;
        for index in 0..model.game.playmats.len() {
            let key = self.keys.key(index);
            let playmat = &model.game.playmats[index];
            let (row_x, row_width) = (x + 8.0, width - 16.0);
            let is_selected = self.selected == Some(key);
            let region = frame.ui.region(
                &format!("pt:row:{key}"),
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
            let path = PlaymatTab::mat_path(&model.folder_path, playmat);
            frame.image_preview_box(Some(&path), row_x + 42.0, row_y + (ROW_HEIGHT - thumb) / 2.0, thumb, thumb);
            let foreground = if is_selected { palette.on_secondary_container } else { palette.on_surface };
            let text_x = row_x + 42.0 + thumb + 12.0;
            let mut name = TextOptions::new().width(row_x + row_width - 8.0 - text_x).ellipsis().color(foreground);
            if is_selected {
                name = name.medium();
            }
            frame.text(&playmat.image_path, text_x, row_y + 9.0, name);
            let percent = format!("{}%", (playmat.scale * 100.0 + 0.5).floor() as i64);
            let options = TextOptions::new()
                .width(row_x + row_width - 8.0 - text_x)
                .ellipsis()
                .size(12.0)
                .color(palette.on_surface_variant);
            frame.text(&percent, text_x, row_y + 30.0, options);
            if region.clicked {
                self.selected = Some(key);
                frame.ui.blur();
            }
            row_y += ROW_HEIGHT + 2.0;
        }
        if let Some((from, to)) = reorder.finish(frame, x + 8.0, width - 16.0) {
            move_item(&mut model.game.playmats, from, to);
            self.keys.moved(from, to);
        }
        frame.end_scroll(
            "pt:list",
            model.game.playmats.len() as f32 * (ROW_HEIGHT + 2.0) + 12.0,
            &ScrollbarStyle::default(),
        );
    }

    /// Letterboxes the table into the box: (x, y, width, height, scale).
    fn canvas_rect(x: f32, y: f32, width: f32, height: f32) -> (f32, f32, f32, f32, f32) {
        let scale = (width / WORLD_WIDTH as f32).min(height / WORLD_HEIGHT as f32);
        let (canvas_width, canvas_height) = (WORLD_WIDTH as f32 * scale, WORLD_HEIGHT as f32 * scale);
        (x + (width - canvas_width) / 2.0, y + (height - canvas_height) / 2.0, canvas_width, canvas_height, scale)
    }

    /// `T:drawTablePreview`: the playmat's size on the table (centred for a shared mat, near the
    /// viewer's edge for individual mats) and the draggable reference card. Returns where the mat
    /// was drawn, for the zones placed on it.
    #[allow(clippy::too_many_arguments)]
    fn draw_table_preview(
        &mut self,
        frame: &mut Frame,
        model: &EditorModel,
        playmat: &PlaymatDefinition,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> MatScreenRect {
        let game = &model.game;
        let (canvas_x, canvas_y, canvas_width, canvas_height, scale) = PlaymatTab::canvas_rect(x, y, width, height);
        let felt = argb_color(TableColor::Felt.resolve(frame.settings, game.table_color.as_deref()));
        let border =
            argb_color(TableColor::PlaymatBorder.resolve(frame.settings, game.playmat_border_color.as_deref()));
        frame.fill_rect(canvas_x, canvas_y, canvas_width, canvas_height, 0.0, felt);
        frame.outline_rect(canvas_x + 1.0, canvas_y + 1.0, canvas_width - 2.0, canvas_height - 2.0, 0.0, border, 1.0);

        // A mat's own size is its image's pixel size at 100%, falling back while it loads.
        let path = PlaymatTab::mat_path(&model.folder_path, playmat);
        let texture = frame.images.get(&path);
        let mat_scale = playmat.scale as f32;
        let size_of = |fallback: (f32, f32)| match texture {
            Some(texture) => (texture.width * mat_scale, texture.height * mat_scale),
            None => (fallback.0 * mat_scale, fallback.1 * mat_scale),
        };
        let (world_width, world_height) = (WORLD_WIDTH as f32, WORLD_HEIGHT as f32);
        let (mat_width, mat_height, mat_left, mat_top) = if game.playmat_type == PlaymatType::Shared {
            let (mat_width, mat_height) = size_of(SHARED_SIZE);
            (mat_width, mat_height, (world_width - mat_width) / 2.0, (world_height - mat_height) / 2.0)
        } else {
            let (mat_width, mat_height) = size_of(INDIVIDUAL_SIZE);
            (mat_width, mat_height, (world_width - mat_width) / 2.0, world_height - mat_height - 0.12 * world_height)
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
            "pt:refcard",
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
        let card = reference_card();
        frame.draw_card(&CardDraw::new(Some(&card), true, center.0, center.1, &[], None).scale(scale));
        MatScreenRect { x: mat_x, y: mat_y, width: mat_screen_width, height: mat_screen_height, preview_scale: scale }
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
        let id = format!("pt:{key}");
        let (column_x, column_width) = column(x, width);
        let mut row_y = y + PAD;
        let thumb = 64.0;
        let path = PlaymatTab::mat_path(&model.folder_path, &model.game.playmats[index]);
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
        let delete_width = frame.button_width("Delete Playmat", &delete_options);
        if frame.button(
            "Delete Playmat",
            column_x + column_width - delete_width,
            row_y,
            Some(delete_width),
            Some(40.0),
            &delete_options,
        ) {
            self.pending_remove = Some(index);
        }
        row_y += thumb + 16.0;

        let format = |value: f32| format!("{}%", (value * 100.0 + 0.5).floor() as i64);
        let options = SliderOptions { step: Some(0.01), disabled: false };
        let (scale, _) = frame.slider_row(
            &format!("{id}:scale"),
            "Scale",
            column_x,
            row_y,
            column_width.min(400.0),
            model.game.playmats[index].scale as f32,
            0.10,
            2.00,
            &options,
            Some(&format),
        );
        if (scale as f64 - model.game.playmats[index].scale).abs() > 1e-6 {
            model.game.playmats[index].scale = scale as f64;
        }
        row_y += 64.0 + 16.0;
        row_y += self.zone_placement.draw_zone_chips(frame, model, index, column_x, row_y, column_width) + 16.0;
        frame.group_label("Table Preview", column_x, row_y, column_width);
        row_y += 24.0;
        // The preview spans the whole pane and fills the height down to the gutter.
        let preview_height = (y + height - PAD - row_y).max(100.0);
        let playmat = model.game.playmats[index].clone();
        let mat = self.draw_table_preview(frame, model, &playmat, x + PAD, row_y, width - 2.0 * PAD, preview_height);
        self.zone_placement.draw_placed_zones(frame, model, index, mat);
    }

    pub fn draw(&mut self, frame: &mut Frame, model: &mut EditorModel, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        self.keys.sync(model.game.playmats.len());
        frame.fill_rect(x, y, width, MODE_BAR_HEIGHT, 0.0, palette.surface_container);
        let options = TextOptions::new().size(12.0).medium().color(palette.on_surface_variant);
        frame.text("Playmat Type", x + 16.0, y + 10.0, options);
        let choices = [Choice::new(PlaymatType::Shared, "Shared"), Choice::new(PlaymatType::Individual, "Individual")];
        model.game.playmat_type =
            frame.segmented("pt:mode", x + 16.0, y + 28.0, 280.0, &choices, model.game.playmat_type, None, false);
        frame.fill_rect(x, y + MODE_BAR_HEIGHT, width, 1.0, 0.0, palette.outline_variant);

        self.pending_remove = None;
        let keys: Vec<u64> = (0..model.game.playmats.len()).map(|index| self.keys.key(index)).collect();
        clamp_selection(&mut self.selected, &keys);
        let selected_index = self.selected.and_then(|key| self.keys.index_of(key));
        let list_y = y + MODE_BAR_HEIGHT + 1.0;
        frame.list_detail(
            x,
            list_y,
            width,
            height - MODE_BAR_HEIGHT - 1.0,
            LIST_WIDTH,
            selected_index.is_some(),
            "Add a playmat to get started",
            |frame, pane| match pane {
                Pane::List(rect) => self.draw_list(frame, model, rect.x, rect.y, rect.width, rect.height),
                Pane::Detail(rect) => {
                    if let Some(index) = selected_index {
                        self.draw_detail(frame, model, index, rect.x, rect.y, rect.width, rect.height);
                    }
                }
            },
        );
        if let Some(index) = self.pending_remove.take() {
            model.game.playmats.remove(index);
            self.keys.removed(index);
            let count = model.game.playmats.len();
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
                            model.game.playmats.push(PlaymatDefinition::new(file));
                            self.keys.sync(model.game.playmats.len());
                            self.selected = Some(self.keys.key(model.game.playmats.len() - 1));
                        }
                        ChooserTarget::Replace(key) => {
                            if let Some(index) = self.keys.index_of(key) {
                                model.game.playmats[index].image_path = file;
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
    use gametable_core::models::{GameDefinition, PlaymatZonePlacement, ZoneDefinition};

    fn model_with_one_mat() -> EditorModel {
        let mut game = GameDefinition::new("game", "Game");
        game.playmat_type = PlaymatType::Individual;
        game.zones.push(ZoneDefinition::new("deck", "Deck"));
        game.zones.push(ZoneDefinition { shared: true, ..ZoneDefinition::new("market", "Market") });
        game.playmats.push(PlaymatDefinition::new("missing.png"));
        EditorModel { folder_path: String::new(), game, card_keys: StableKeys::default() }
    }

    #[test]
    fn place_a_zone_on_the_mat_then_drag_it_off() {
        let mut harness = UiHarness::new();
        let mut model = model_with_one_mat();
        let mut tab = PlaymatTab::new();
        tab.keys.sync(1);
        tab.selected = Some(tab.keys.key(0));
        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 0.0, 1280.0, 800.0);

        // Only the player zone is offered for an individual mat; clicking it places it centred.
        harness.frame(|frame| draw(frame));
        assert!(harness.region_center("pt:place:market").is_none());
        harness.click_region("pt:place:deck", &mut draw);
        harness.frame(|frame| draw(frame));
        let (zone_x, zone_y) = harness.region_center("pt:zone:deck").expect("placed zone drawn on the mat");
        assert_eq!(model.game.playmats[0].zone_placements, vec![PlaymatZonePlacement::new("deck", 0.5, 0.5)]);

        // Dragging it a little moves it; dragging it off the mat takes it off.
        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 0.0, 1280.0, 800.0);
        let drag_to = |harness: &mut UiHarness, draw: &mut dyn FnMut(&mut Frame), from: (f32, f32), to: (f32, f32)| {
            harness.move_mouse(from.0, from.1);
            harness.frame(|frame| draw(frame));
            harness.press(from.0, from.1, 1, 1);
            harness.frame(|frame| draw(frame));
            harness.move_mouse(to.0, to.1);
            harness.frame(|frame| draw(frame));
            harness.release(to.0, to.1, 1);
            harness.frame(|frame| draw(frame));
            harness.frame(|frame| draw(frame));
        };
        drag_to(&mut harness, &mut draw, (zone_x, zone_y), (zone_x + 30.0, zone_y));
        let moved = &model.game.playmats[0].zone_placements[0];
        assert!(moved.x > 0.5 && (moved.y - 0.5).abs() < 1e-3, "{moved:?}");

        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 0.0, 1280.0, 800.0);
        harness.frame(|frame| draw(frame));
        let (zone_x, zone_y) = harness.region_center("pt:zone:deck").unwrap();
        drag_to(&mut harness, &mut draw, (zone_x, zone_y), (5.0, 5.0));
        assert!(model.game.playmats[0].zone_placements.is_empty());
    }
}
