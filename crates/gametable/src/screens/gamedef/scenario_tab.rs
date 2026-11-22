//! The Scenarios tab: named setups (shared zones, starting entries or a built subdeck, and cards
//! placed on the table) a host can load from the in-game menu. Shown only while the game has
//! scenarios. Port of `src/ui/screens/gamedef/scenario_tab.lua`. GameTable additions: the Card Placement
//! canvas draws the game boards where the table places them (for a chosen number of seats) and zooms and
//! pans like the Game Boards tab's Table Preview; right-clicking it sets one of the scenario's zones at a
//! spot on the table (dragged anywhere here, fixed in the game).

use std::collections::HashMap;
use std::f32::consts::PI;

use gametable_core::geometry::{WORLD_HEIGHT, WORLD_WIDTH};
use gametable_core::models::{
    CardBackDefinition, CardDefinition, CardOrientation, DeckEntry, Scenario, ScenarioPlacement,
    ScenarioTokenPlacement, ScenarioZonePlacement, SubDeck, WidgetSizeOverride, ZoneDefinition, ZoneKind,
    ZoneWidgetKind,
};
use gametable_core::uuid::new_v4_uuid;

use crate::app::Frame;
use crate::data::display_settings::{SHARED_BOARD_BORDER_COLOR, TableColor};
use crate::data::file_system::join_path;
use crate::graphics::draw_list::Color;
use crate::graphics::tessellation::DrawMode;
use crate::screens::gamedef::EditorModel;
use crate::screens::gamedef::board_zone_placement::{
    draw_zone_label, draw_zone_stand_in, placed_zone_label, zone_world_size,
};
use crate::screens::gamedef::common::{
    FIELD_HEIGHT, FileImport, GAP, ImportResponse, PAD, SectionAction, StableKeys, ToggleRow, column, display_card,
    display_card_backs, orientation_choices,
};
use crate::screens::gamedef::table_board_placement::{
    MAXIMUM_PREVIEW_PLAYERS, PreviewCamera, ZOOM_STEP, clamp_to_reachable_table, draw_placed_boards, draw_preview_felt,
    draw_zoom_controls, is_on_reachable_table,
};
use crate::screens::gamedef::widget_scale_dialog::{WidgetScaleDialog, WidgetScaleResponse};
use crate::screens::gamedef::zones_tab::{
    ZonePick, bag_token_images, deck_type_field, draw_card_entries, draw_counter_fields, draw_token_bag_fields,
    kind_choices, side_choices, widget_kind_choices,
};
use crate::screens::token_choosers::{ChooserResponse, TokenChooser};
use crate::table::board_layout::has_placed_boards;
use crate::table::card_pool_grid::{NAME_LINES, TILE_CARD_HEIGHT, TILE_CARD_WIDTH, TILE_SCALE, any_card_turned};
use crate::table::card_render::{
    CARD_HEIGHT, CARD_WIDTH, CardDraw, PreviewFace, definition_faces, orientation_turns, shown_orientation,
};
use crate::table::selection::half_extents;
use crate::table::view::IMAGE_TOKEN_SIZE;
use crate::ui::card_filter_bar::CardFilterState;
use crate::ui::components::{Reorder, argb_color, move_item};
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::dropdown::DropdownOptions;
use crate::ui::overlays::{ContextMenu, MenuItem, MenuResponse};
use crate::ui::panel::{Pane, clamp_selection};
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::{Weight, fixed, with_alpha};
use crate::ui::widgets::{ButtonOptions, ButtonVariant, Choice, IconButtonOptions, ScrollbarStyle};

const ROW_HEIGHT: f32 = 56.0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum ScenarioView {
    Zones,
    Placement,
    Deck,
}

fn view_choices() -> [Choice<ScenarioView>; 3] {
    [
        Choice::new(ScenarioView::Zones, "Zones"),
        Choice::new(ScenarioView::Placement, "Card Placement"),
        Choice::new(ScenarioView::Deck, "Deck Builder"),
    ]
}

fn scenario_mode_choices() -> [Choice<bool>; 2] {
    [Choice::new(false, "Player Cards"), Choice::new(true, "Scenario Cards")]
}

/// A card's authored "quantity" extra field (how many physical copies exist); `None` means no cap.
fn owned_quantity(card: &CardDefinition) -> Option<f64> {
    match card.extra_fields.get("quantity")? {
        gametable_core::json::JsonValue::Number(number) => Some(*number),
        gametable_core::json::JsonValue::String(text) => gametable_core::util::trim_lua_whitespace(text).parse().ok(),
        _ => None,
    }
}

/// The zones sub-view's own selection for one scenario.
#[derive(Default)]
struct ZonesView {
    keys: StableKeys,
    selected: Option<u64>,
}

/// A placement being clone-dragged: the original's index and the clone's.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CloneDrag {
    tokens: bool,
    original: usize,
    clone: usize,
}

/// What the Card Placement canvas's right-click menu adds.
#[derive(Debug, Clone, PartialEq)]
enum AddToCanvas {
    Card,
    Token,
    /// Sets one of the scenario's zones on the table where the canvas was right-clicked.
    Zone(String),
}

/// What the image token Size menu sets: one of the preset sizes (`None` is the default), or opens
/// Set Scale.
#[derive(Debug, Clone, PartialEq)]
enum TokenSizeChoice {
    Preset(Option<f64>),
    SetScale,
}

/// Which placed widget the Set Scale dialog is scaling.
#[derive(Debug, Clone, PartialEq)]
enum ScaledWidget {
    /// A scenario zone set on the table, by zone id.
    Zone(String),
    /// An image token, by index into the scenario's token placements.
    Token(usize),
}

/// An image token's scale: its size against the default token size.
fn token_scale(size_override: Option<WidgetSizeOverride>) -> f64 {
    match size_override {
        Some(WidgetSizeOverride::Multiplier(size)) => size / f64::from(IMAGE_TOKEN_SIZE),
        _ => 1.0,
    }
}

/// The size override that draws an image token at `scale` (none at the usual size).
fn token_size_override_for_scale(scale: f64) -> Option<WidgetSizeOverride> {
    if (scale - 1.0).abs() < 1e-6 {
        None
    } else {
        Some(WidgetSizeOverride::Multiplier(f64::from(IMAGE_TOKEN_SIZE) * scale))
    }
}

/// What a running file import or token chooser is for.
enum ChooserTarget {
    Zone { scenario: u64, zone: u64, pick: ZonePick },
    TokenPlacement { scenario: u64, x: f64, y: f64 },
}

pub struct ScenarioTab {
    keys: StableKeys,
    selected: Option<u64>,
    view: ScenarioView,
    zones_views: HashMap<u64, ZonesView>,
    active_deck_type: HashMap<u64, String>,
    deck_pool_scenario: bool,
    placement_library_scenario: bool,
    deck_filters: Option<CardFilterState>,
    placement_filters: Option<CardFilterState>,
    placement_library_open: bool,
    /// A card dragged from the library, placed where the mouse is released.
    placing_draft: Option<String>,
    selected_placement: Option<usize>,
    selected_token_placement: Option<usize>,
    /// The scenario zone (by id) selected on the placement canvas.
    selected_zone_placement: Option<String>,
    clone_drag: Option<CloneDrag>,
    /// The canvas fraction last right-clicked (where Add Token... drops).
    placement_click: (f64, f64),
    hovered_id: Option<String>,
    hovered_placement_face: Option<(String, bool, CardOrientation)>,
    duplicate_menu: Option<ContextMenu<usize>>,
    placement_menu: Option<ContextMenu<AddToCanvas>>,
    size_menu: Option<ContextMenu<TokenSizeChoice>>,
    /// The Set Scale dialog, open for a placed widget.
    scaling: Option<(ScaledWidget, WidgetScaleDialog)>,
    guide_import: Option<(FileImport, u64)>,
    chooser: Option<(TokenChooser, ChooserTarget)>,
    pending_remove: Option<usize>,
    pending_zone_remove: Option<(u64, usize)>,
    /// The Card Placement canvas's zoom and pan (GameTable addition, as the Table Preview's).
    placement_camera: PreviewCamera,
    /// The mouse position while the placement canvas is being dragged around.
    placement_pan: Option<(f32, f32)>,
    /// How many seats the placement canvas lays the game boards out for.
    board_preview_players: usize,
}

impl Default for ScenarioTab {
    fn default() -> Self {
        ScenarioTab::new()
    }
}

impl ScenarioTab {
    pub fn new() -> ScenarioTab {
        ScenarioTab {
            keys: StableKeys::default(),
            selected: None,
            view: ScenarioView::Zones,
            zones_views: HashMap::new(),
            active_deck_type: HashMap::new(),
            deck_pool_scenario: true,
            placement_library_scenario: true,
            deck_filters: None,
            placement_filters: None,
            placement_library_open: false,
            placing_draft: None,
            selected_placement: None,
            selected_token_placement: None,
            selected_zone_placement: None,
            clone_drag: None,
            placement_click: (0.0, 0.0),
            hovered_id: None,
            hovered_placement_face: None,
            duplicate_menu: None,
            placement_menu: None,
            size_menu: None,
            scaling: None,
            guide_import: None,
            chooser: None,
            pending_remove: None,
            pending_zone_remove: None,
            placement_camera: PreviewCamera::default(),
            placement_pan: None,
            board_preview_players: 2,
        }
    }

    /// Development hook: `"placement"` or `"deck"`.
    pub fn set_view_for_development(&mut self, view: &str) {
        self.view = if view == "placement" { ScenarioView::Placement } else { ScenarioView::Deck };
    }

    fn selected_index(&self) -> Option<usize> {
        self.selected.and_then(|key| self.keys.index_of(key))
    }

    /// `T:duplicateScenario`: a deep copy with fresh scenario and zone ids (so both can load into
    /// one session without their shared zones colliding).
    fn duplicate_scenario(&mut self, model: &mut EditorModel, index: usize) {
        let mut copy = model.game.scenarios[index].clone();
        copy.id = new_v4_uuid();
        copy.name = format!("{} Copy", copy.name);
        for zone in &mut copy.zones {
            let fresh_id = new_v4_uuid();
            // The copy sets its own zones on the table where the original sets them.
            for placement in copy.zone_placements.iter_mut().filter(|placement| placement.zone_id == zone.id) {
                placement.zone_id = fresh_id.clone();
            }
            zone.id = fresh_id;
        }
        model.game.scenarios.push(copy);
        self.keys.sync(model.game.scenarios.len());
        self.selected = Some(self.keys.key(model.game.scenarios.len() - 1));
    }

    /// `T:keypressed`: on the placement canvas, F flips, Q/E rotate (Ctrl for 45°), Delete or
    /// Backspace removes the selected placement (or takes the selected zone off the table).
    pub fn key_pressed(&mut self, frame: &mut Frame, model: &mut EditorModel, key: &str) {
        if self.view != ScenarioView::Placement || frame.ui.focus.is_some() {
            return;
        }
        let Some(index) = self.selected_index() else { return };
        let step = if frame.keyboard.ctrl() { 0.5 } else { 1.0 };
        let scenario = &mut model.game.scenarios[index];
        if let Some(placement_index) =
            self.selected_placement.filter(|&placement| placement < scenario.placements.len())
        {
            let placement = &mut scenario.placements[placement_index];
            match key {
                "f" => placement.face_up = !placement.face_up,
                "q" => placement.rotation_turns -= step,
                "e" => placement.rotation_turns += step,
                "delete" | "backspace" => {
                    scenario.placements.remove(placement_index);
                    self.selected_placement = None;
                }
                _ => {}
            }
        } else if let Some(zone_id) = self.selected_zone_placement.clone() {
            if matches!(key, "delete" | "backspace") {
                scenario.zone_placements.retain(|placement| placement.zone_id != zone_id);
                self.selected_zone_placement = None;
            }
        } else if let Some(token_index) =
            self.selected_token_placement.filter(|&token| token < scenario.token_placements.len())
        {
            let placement = &mut scenario.token_placements[token_index];
            match key {
                "q" => placement.rotation_turns -= step,
                "e" => placement.rotation_turns += step,
                "delete" | "backspace" => {
                    scenario.token_placements.remove(token_index);
                    self.selected_token_placement = None;
                }
                _ => {}
            }
        }
    }

    // -----------------------------------------------------------------------
    // Scenario list / detail
    // -----------------------------------------------------------------------

    fn draw_scenario_list(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let palette = frame.palette();
        frame.fill_rect(x, y, width, height, 0.0, palette.surface_container);
        let (header_height, _) = frame.section_header(
            "scen:list",
            "Scenarios",
            x + 16.0,
            y + 12.0,
            width - 32.0,
            Some(model.game.scenarios.len()),
            None,
            None,
        );
        let (button_x, button_width) = (x + 16.0, width - 32.0);
        let mut button_y = y + 12.0 + header_height + 4.0;
        let add = ButtonOptions::variant(ButtonVariant::Tonal).icon("add").id("scen:add");
        if frame.button("Add Scenario", button_x, button_y, Some(button_width), Some(40.0), &add) {
            model.game.scenarios.push(Scenario::new(new_v4_uuid(), "New Scenario"));
            self.keys.sync(model.game.scenarios.len());
            self.selected = Some(self.keys.key(model.game.scenarios.len() - 1));
        }
        button_y += 40.0 + 8.0;
        let duplicate = ButtonOptions::variant(ButtonVariant::Outlined)
            .icon("copy_all")
            .id("scen:dup")
            .disabled(model.game.scenarios.is_empty());
        if frame.button("Duplicate Scenario...", button_x, button_y, Some(button_width), Some(40.0), &duplicate) {
            let items = model
                .game
                .scenarios
                .iter()
                .enumerate()
                .map(|(index, scenario)| {
                    let name = if scenario.name.is_empty() { "(unnamed scenario)" } else { scenario.name.as_str() };
                    MenuItem::new(name, index)
                })
                .collect();
            self.duplicate_menu = Some(ContextMenu::new("scen:dupmenu", button_x, button_y + 40.0 + 4.0, items));
        }
        let list_y = button_y + 40.0 + 12.0;
        let offset = frame.begin_scroll("scen:list:scroll", x, list_y, width, y + height - list_y);
        let reorder = Reorder::begin(frame, "scen:scenarios");
        let mut row_y = list_y - offset;
        for index in 0..model.game.scenarios.len() {
            let key = self.keys.key(index);
            let scenario = &model.game.scenarios[index];
            let (row_x, row_width) = (x + 8.0, width - 16.0);
            let is_selected = self.selected == Some(key);
            let region = frame.ui.region(
                &format!("scen:row:{key}"),
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
            let foreground = if is_selected { palette.on_secondary_container } else { palette.on_surface };
            let icon_color = if is_selected { foreground } else { palette.on_surface_variant };
            frame.icon("folder", row_x + 42.0, row_y + (ROW_HEIGHT - 20.0) / 2.0, 20.0, icon_color);
            let text_x = row_x + 72.0;
            let name = if scenario.name.is_empty() { "(unnamed scenario)" } else { scenario.name.as_str() };
            let mut name_options =
                TextOptions::new().width(row_x + row_width - 8.0 - text_x).ellipsis().color(foreground);
            if is_selected {
                name_options = name_options.medium();
            }
            frame.text(name, text_x, row_y + 9.0, name_options);
            let subtitle = format!("{} zone(s) · {} placed", scenario.zones.len(), scenario.placements.len());
            let options = TextOptions::new()
                .width(row_x + row_width - 8.0 - text_x)
                .ellipsis()
                .size(12.0)
                .color(palette.on_surface_variant);
            frame.text(&subtitle, text_x, row_y + 30.0, options);
            if region.clicked {
                self.selected = Some(key);
                frame.ui.blur();
            }
            row_y += ROW_HEIGHT + 2.0;
        }
        if let Some((from, to)) = reorder.finish(frame, x + 8.0, width - 16.0) {
            move_item(&mut model.game.scenarios, from, to);
            self.keys.moved(from, to);
        }
        frame.end_scroll(
            "scen:list:scroll",
            model.game.scenarios.len() as f32 * (ROW_HEIGHT + 2.0) + 12.0,
            &ScrollbarStyle::default(),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_scenario_detail(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        index: usize,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let palette = frame.palette();
        let key = self.keys.key(index);
        let (column_x, column_width) = column(x, width);
        let mut row_y = y + PAD;
        let delete_options = ButtonOptions::variant(ButtonVariant::Text).icon("delete_outline").id("scen:del");
        let delete_width = frame.button_width("Delete Scenario", &delete_options);
        let scenario = &mut model.game.scenarios[index];
        frame.text_field(
            "scen:name",
            column_x,
            row_y,
            column_width - delete_width - GAP,
            &mut scenario.name,
            &TextFieldOptions::label("Scenario Name"),
        );
        if frame.button(
            "Delete Scenario",
            column_x + column_width - delete_width,
            row_y,
            Some(delete_width),
            Some(FIELD_HEIGHT),
            &delete_options,
        ) {
            self.pending_remove = Some(index);
        }
        row_y += FIELD_HEIGHT + 16.0;

        let choose_label = if scenario.guide_path.is_some() { "Change Guide..." } else { "Choose Guide..." };
        let choose_options = ButtonOptions::variant(ButtonVariant::Outlined).id("scen:guide:choose");
        let choose_width = frame.button_width(choose_label, &choose_options);
        let clear_width = if scenario.guide_path.is_some() { 40.0 } else { 0.0 };
        frame.icon("insert_drive_file", column_x, row_y + 10.0, 20.0, palette.on_surface_variant);
        let color = if scenario.guide_path.is_some() { palette.on_surface } else { palette.on_surface_variant };
        let options = TextOptions::new()
            .width(column_width - 28.0 - choose_width - clear_width - 16.0)
            .middle_of(FIELD_HEIGHT)
            .ellipsis()
            .color(color);
        frame.text(
            scenario.guide_path.as_deref().unwrap_or("No Scenario Guide assigned"),
            column_x + 28.0,
            row_y,
            options,
        );
        if scenario.guide_path.is_some()
            && frame.icon_button(
                "close",
                column_x + column_width - choose_width - clear_width,
                row_y + 4.0,
                &IconButtonOptions::tooltip("Clear Scenario Guide").id("scen:guide:clear"),
            )
        {
            scenario.guide_path = None;
        }
        if frame.button(
            choose_label,
            column_x + column_width - choose_width,
            row_y,
            Some(choose_width),
            Some(FIELD_HEIGHT),
            &choose_options,
        ) {
            let destination = join_path(&[&model.folder_path, "_scenarios"]);
            self.guide_import = Some((FileImport::document(frame, destination), key));
        }
        row_y += FIELD_HEIGHT + 16.0;
        self.view = frame.segmented(
            "scen:view",
            column_x,
            row_y,
            column_width.min(480.0),
            &view_choices(),
            self.view,
            Some(40.0),
            false,
        );
        row_y += 40.0 + 16.0;

        frame.ui.push_clip(x, row_y, width, y + height - row_y);
        let body_height = y + height - row_y;
        match self.view {
            ScenarioView::Placement => self.draw_placement_view(frame, model, index, x, row_y, width, body_height),
            ScenarioView::Deck => self.draw_deck_builder_view(frame, model, index, x, row_y, width, body_height),
            ScenarioView::Zones => self.draw_zones_view(frame, model, index, x, row_y, width, body_height),
        }
        frame.ui.pop_clip();
    }

    // -----------------------------------------------------------------------
    // Zones sub-view
    // -----------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn draw_zones_view(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        index: usize,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let scenario_key = self.keys.key(index);
        let zone_count = model.game.scenarios[index].zones.len();
        let view = self.zones_views.entry(scenario_key).or_default();
        view.keys.sync(zone_count);
        let keys: Vec<u64> = (0..zone_count).map(|zone| view.keys.key(zone)).collect();
        clamp_selection(&mut view.selected, &keys);
        let selected_zone = view.selected.and_then(|key| view.keys.index_of(key));
        frame.list_detail(
            x,
            y,
            width,
            height,
            260.0,
            selected_zone.is_some(),
            "Add a zone to get started",
            |frame, pane| match pane {
                Pane::List(rect) => {
                    self.draw_scenario_zone_list(frame, model, index, rect.x, rect.y, rect.width, rect.height)
                }
                Pane::Detail(rect) => {
                    if let Some(zone_index) = selected_zone {
                        self.draw_scenario_zone_detail(
                            frame,
                            model,
                            index,
                            zone_index,
                            rect.x,
                            rect.y,
                            rect.width,
                            rect.height,
                        );
                    }
                }
            },
        );
        if let Some((key, zone_index)) = self.pending_zone_remove.take()
            && key == scenario_key
        {
            let scenario = &mut model.game.scenarios[index];
            let removed_zone = scenario.zones.remove(zone_index);
            // A removed zone is off the table too.
            scenario.zone_placements.retain(|placement| placement.zone_id != removed_zone.id);
            let zones = &scenario.zones;
            let view = self.zones_views.entry(scenario_key).or_default();
            view.keys.removed(zone_index);
            let count = zones.len();
            view.selected = (count > 0).then(|| view.keys.key(zone_index.min(count - 1)));
        }
    }

    fn scenario_zone_badge(zone: &ZoneDefinition) -> String {
        if zone.kind == ZoneKind::Widget {
            if zone.widget_kind == ZoneWidgetKind::TokenBag {
                let total: i64 = zone.bag_entries.iter().map(|entry| entry.quantity).sum();
                return format!("{total} token{}", if total == 1 { "" } else { "s" });
            }
            return "Counter".to_string();
        }
        if zone.deals_built_deck {
            format!("Deck: {}", zone.deck_type)
        } else {
            format!("{} entries", zone.entries.len())
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_scenario_zone_list(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        index: usize,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let palette = frame.palette();
        let scenario_key = self.keys.key(index);
        frame.fill_rect(x, y, width, height, 0.0, palette.surface_container);
        let scenario = &mut model.game.scenarios[index];
        let action = SectionAction { label: "Add Zone", icon: Some("add"), id: "scen:zadd", disabled: false };
        let (header_height, add) = frame.section_header(
            "scen:zlist",
            "Zones",
            x + 16.0,
            y + 12.0,
            width - 32.0,
            Some(scenario.zones.len()),
            None,
            Some(action),
        );
        let view = self.zones_views.entry(scenario_key).or_default();
        if add {
            scenario.zones.push(ZoneDefinition { shared: true, ..ZoneDefinition::new(new_v4_uuid(), "New Zone") });
            view.keys.sync(scenario.zones.len());
            view.selected = Some(view.keys.key(scenario.zones.len() - 1));
        }
        let list_y = y + 12.0 + header_height + 8.0;
        let scroll_id = format!("scen:zlist:scroll:{scenario_key}");
        let offset = frame.begin_scroll(&scroll_id, x, list_y, width, y + height - list_y);
        let reorder = Reorder::begin(frame, &format!("scen:zones:{scenario_key}"));
        let mut row_y = list_y - offset;
        for zone_index in 0..scenario.zones.len() {
            let key = view.keys.key(zone_index);
            let zone = &scenario.zones[zone_index];
            let (row_x, row_width) = (x + 8.0, width - 16.0);
            let is_selected = view.selected == Some(key);
            let region = frame.ui.region(
                &format!("scen:zrow:{key}"),
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
            reorder.handle(frame, zone_index, row_x, row_y + (ROW_HEIGHT - 40.0) / 2.0, row_y, ROW_HEIGHT);
            let foreground = if is_selected { palette.on_secondary_container } else { palette.on_surface };
            let icon = if zone.kind == ZoneKind::Card { "layers" } else { "widgets" };
            let icon_color = if is_selected { foreground } else { palette.on_surface_variant };
            frame.icon(icon, row_x + 42.0, row_y + (ROW_HEIGHT - 20.0) / 2.0, 20.0, icon_color);
            let text_x = row_x + 72.0;
            let name = if zone.name.is_empty() { "(unnamed zone)" } else { zone.name.as_str() };
            let mut name_options =
                TextOptions::new().width(row_x + row_width - 8.0 - text_x).ellipsis().color(foreground);
            if is_selected {
                name_options = name_options.medium();
            }
            frame.text(name, text_x, row_y + 9.0, name_options);
            let options = TextOptions::new()
                .width(row_x + row_width - 8.0 - text_x)
                .ellipsis()
                .size(12.0)
                .color(palette.on_surface_variant);
            frame.text(&ScenarioTab::scenario_zone_badge(zone), text_x, row_y + 30.0, options);
            if region.clicked {
                view.selected = Some(key);
                frame.ui.blur();
            }
            row_y += ROW_HEIGHT + 2.0;
        }
        if let Some((from, to)) = reorder.finish(frame, x + 8.0, width - 16.0) {
            move_item(&mut scenario.zones, from, to);
            view.keys.moved(from, to);
        }
        frame.end_scroll(
            &scroll_id,
            scenario.zones.len() as f32 * (ROW_HEIGHT + 2.0) + 12.0,
            &ScrollbarStyle::default(),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_scenario_zone_detail(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        index: usize,
        zone_index: usize,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let palette = frame.palette();
        let scenario_key = self.keys.key(index);
        let zone_key = self.zones_views.entry(scenario_key).or_default().keys.key(zone_index);
        let id = format!("scen:zone:{zone_key}");
        let offset = frame.begin_scroll("scen:zdetail", x, y, width, height);
        let (column_x, column_width) = column(x, width);
        let mut row_y = y + PAD - offset;
        let card_choices: Vec<Choice<String>> =
            model.game.cards().iter().map(|card| Choice::new(card.id.clone(), card.card_title.clone())).collect();
        let bag_images = bag_token_images(&model.game, &model.folder_path);
        let folder = model.folder_path.clone();
        let zone = &mut model.game.scenarios[index].zones[zone_index];
        // Scenario zones are always shared.
        zone.shared = true;

        let name = if zone.name.is_empty() { "(unnamed zone)".to_string() } else { zone.name.clone() };
        let options = TextOptions::new().size(20.0).medium().middle_of(40.0).width(column_width - 160.0).ellipsis();
        frame.text(&name, column_x, row_y, options);
        let delete_id = format!("{id}:del");
        let delete_options = ButtonOptions::variant(ButtonVariant::Text).icon("delete_outline").id(&delete_id);
        let delete_width = frame.button_width("Delete Zone", &delete_options);
        if frame.button(
            "Delete Zone",
            column_x + column_width - delete_width,
            row_y,
            Some(delete_width),
            Some(40.0),
            &delete_options,
        ) {
            self.pending_zone_remove = Some((scenario_key, zone_index));
        }
        row_y += 40.0 + 16.0;
        let half_width = (column_width - GAP) / 2.0;
        frame.text_field(
            &format!("{id}:name"),
            column_x,
            row_y,
            half_width,
            &mut zone.name,
            &TextFieldOptions::label("Zone Name"),
        );
        frame.text_field(
            &format!("{id}:id"),
            column_x + half_width + GAP,
            row_y,
            half_width,
            &mut zone.id,
            &TextFieldOptions::label("Zone ID"),
        );
        row_y += FIELD_HEIGHT + 20.0;
        zone.kind = frame.segmented(
            &format!("{id}:kind"),
            column_x,
            row_y,
            column_width.min(320.0),
            &kind_choices(),
            zone.kind,
            None,
            false,
        );
        row_y += 40.0 + 20.0;

        if zone.kind == ZoneKind::Widget {
            let options = DropdownOptions { label: Some("Widget Type"), ..DropdownOptions::default() };
            zone.widget_kind = frame.dropdown(
                &format!("{id}:wkind"),
                column_x,
                row_y,
                half_width,
                &widget_kind_choices(),
                zone.widget_kind,
                &options,
            );
            let side_x = column_x + half_width + GAP;
            frame.text(
                "Side",
                side_x,
                row_y + 4.0,
                TextOptions::new().middle_of(40.0).color(palette.on_surface_variant),
            );
            zone.side = frame.segmented(
                &format!("{id}:side"),
                side_x + 40.0,
                row_y + 4.0,
                half_width - 40.0,
                &side_choices(),
                zone.side,
                None,
                false,
            );
            row_y += FIELD_HEIGHT + GAP;
            let mut pick = None;
            if zone.widget_kind == ZoneWidgetKind::TokenBag {
                row_y += draw_token_bag_fields(
                    frame,
                    &folder,
                    &bag_images,
                    zone,
                    &id,
                    column_x,
                    row_y,
                    column_width,
                    &mut pick,
                );
            } else {
                row_y += draw_counter_fields(frame, &folder, zone, &id, column_x, row_y, column_width, &mut pick);
            }
            if let Some(pick) = pick {
                let chooser = pick.chooser(&model.game, &folder);
                self.chooser = Some((chooser, ChooserTarget::Zone { scenario: scenario_key, zone: zone_key, pick }));
            }
            row_y += PAD;
            frame.end_scroll("scen:zdetail", row_y + offset - y, &ScrollbarStyle::default());
            return;
        }

        row_y += frame.spaced_group_label("Display", column_x, row_y, column_width);
        frame.text("Side", column_x, row_y + 4.0, TextOptions::new().middle_of(40.0).color(palette.on_surface_variant));
        zone.side = frame.segmented(
            &format!("{id}:side"),
            column_x + 40.0,
            row_y + 4.0,
            half_width - 40.0,
            &side_choices(),
            zone.side,
            None,
            false,
        );
        let options = DropdownOptions { label: Some("Zone Orientation"), ..DropdownOptions::default() };
        zone.orientation = frame.dropdown(
            &format!("{id}:orient"),
            column_x + half_width + GAP,
            row_y,
            half_width,
            &orientation_choices(),
            zone.orientation,
            &options,
        );
        row_y += FIELD_HEIGHT + 8.0;

        row_y += frame.spaced_group_label("Behavior", column_x, row_y, column_width);
        let (mut face_up, mut shuffleable, mut auto_shuffle, mut discard) =
            (zone.face_up, zone.shuffleable, zone.auto_shuffle, zone.is_discard_pile);
        let mut rows = vec![
            ToggleRow {
                id: format!("{id}:faceup"),
                label: "Face Up",
                help: Some("Cards in this zone show their real face"),
                value: &mut face_up,
                disabled: false,
            },
            ToggleRow {
                id: format!("{id}:shuffleable"),
                label: "Shuffleable",
                help: Some("Show a shuffle button for this zone"),
                value: &mut shuffleable,
                disabled: false,
            },
            ToggleRow {
                id: format!("{id}:autoshuffle"),
                label: "Auto Shuffle",
                help: Some("Randomize this zone's order when the scenario loads"),
                value: &mut auto_shuffle,
                disabled: false,
            },
            ToggleRow {
                id: format!("{id}:discard"),
                label: "Is Discard Pile",
                help: Some("Pressing D sends a hovered table card here"),
                value: &mut discard,
                disabled: false,
            },
        ];
        row_y += frame.toggle_grid(&mut rows, column_x, row_y, column_width);
        drop(rows);
        (zone.face_up, zone.shuffleable, zone.auto_shuffle, zone.is_discard_pile) =
            (face_up, shuffleable, auto_shuffle, discard);

        row_y += frame.spaced_group_label("Contents", column_x, row_y, column_width);
        let (deals, toggle_height) = frame.toggle_row(
            &format!("{id}:deals"),
            "Loads Built Deck",
            Some("Deals from a named subdeck built in this scenario's Deck Builder sub-view, instead of the entries below"),
            column_x,
            row_y,
            column_width,
            zone.deals_built_deck,
            false,
        );
        zone.deals_built_deck = deals;
        row_y += toggle_height + 8.0;
        if zone.deals_built_deck {
            deck_type_field(
                frame,
                &format!("{id}:decktypetext"),
                &format!("{id}:decktype"),
                "Deck Name",
                None,
                &mut zone.deck_type,
                column_x,
                row_y,
                column_width.min(360.0),
            );
            row_y += FIELD_HEIGHT + 4.0;
            let message = format!("Cards come from the \"{}\" deck in the Deck Builder sub-view.", zone.deck_type);
            frame.text(
                &message,
                column_x,
                row_y,
                TextOptions::new().width(column_width).color(palette.on_surface_variant),
            );
            row_y += 24.0;
        } else {
            row_y += draw_card_entries(frame, &card_choices, &mut zone.entries, &id, column_x, row_y, column_width);
        }
        row_y += PAD;
        frame.end_scroll("scen:zdetail", row_y + offset - y, &ScrollbarStyle::default());
    }

    // -----------------------------------------------------------------------
    // Deck Builder sub-view
    // -----------------------------------------------------------------------

    /// One slot per distinct deck type of the scenario's built-deck zones: (deck type, label).
    fn deck_slots(scenario: &Scenario) -> Vec<(String, String)> {
        let mut slots: Vec<(String, String)> = Vec::new();
        for zone in scenario.zones.iter().filter(|zone| zone.deals_built_deck) {
            if !slots.iter().any(|(deck_type, _)| *deck_type == zone.deck_type) {
                slots.push((zone.deck_type.clone(), zone.name.clone()));
            }
        }
        slots
    }

    fn add_deck_copy(frame: &mut Frame, model: &mut EditorModel, index: usize, deck_type: &str, definition_id: &str) {
        let definition = model.game.card_by_id(definition_id).cloned();
        let scenario = &mut model.game.scenarios[index];
        let existing = scenario
            .subdeck(deck_type)
            .and_then(|deck| deck.entries.iter().find(|entry| entry.definition_id == definition_id))
            .map(|entry| entry.quantity);
        if let Some(limit) = definition.as_ref().and_then(owned_quantity)
            && existing.unwrap_or(0) as f64 >= limit
        {
            let title = definition.as_ref().map_or("this card", |card| card.card_title.as_str());
            let copies = if limit == 1.0 { "y" } else { "ies" };
            frame.ui.toast(&format!("Only {} cop{copies} of \"{title}\" available.", format_number(limit)), None);
            return;
        }
        let deck_position = match scenario.decks.iter().position(|deck| deck.name == deck_type) {
            Some(position) => position,
            None => {
                scenario.decks.push(SubDeck::new(deck_type.to_string(), Vec::new()));
                scenario.decks.len() - 1
            }
        };
        let entries = &mut scenario.decks[deck_position].entries;
        match entries.iter_mut().find(|entry| entry.definition_id == definition_id) {
            Some(entry) => entry.quantity += 1,
            None => entries.push(DeckEntry::new(definition_id.to_string(), 1)),
        }
    }

    fn remove_deck_copy(scenario: &mut Scenario, deck_type: &str, definition_id: &str) {
        let Some(deck) = scenario.decks.iter_mut().find(|deck| deck.name == deck_type) else { return };
        if let Some(position) = deck.entries.iter().position(|entry| entry.definition_id == definition_id) {
            if deck.entries[position].quantity <= 1 {
                deck.entries.remove(position);
            } else {
                deck.entries[position].quantity -= 1;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_deck_pool(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        index: usize,
        deck_type: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let palette = frame.palette();
        let game = model.game.clone();
        let mut row_y = y;
        let has_scenario_cards = game.cards().iter().any(|card| card.is_scenario_card);
        if has_scenario_cards {
            self.deck_pool_scenario = frame.segmented(
                "scen:deckmode",
                x + 12.0,
                row_y + 12.0,
                width - 24.0,
                &scenario_mode_choices(),
                self.deck_pool_scenario,
                Some(40.0),
                false,
            );
            row_y += 12.0 + 40.0 + 12.0;
        }
        let filters = self.deck_filters.get_or_insert_with(|| CardFilterState::new(&game));
        let (filters_bottom, _) =
            frame.card_filters("scendeck", filters, &game, x + 12.0, row_y, width - 24.0, false, &[]);
        row_y = frame.card_search(
            "scendeck",
            filters,
            &game,
            x + 12.0,
            filters_bottom + 12.0,
            width - 24.0,
            Some(palette.surface),
            None,
        ) + 4.0;
        let mut visible: Vec<&CardDefinition> = filters.visible(&game);
        if has_scenario_cards {
            visible.retain(|card| card.is_scenario_card == self.deck_pool_scenario);
        }
        let entries: Vec<DeckEntry> = model.game.scenarios[index].entries_for(deck_type).to_vec();
        let name_height = frame.card_pool_name_height();
        let display_backs = display_card_backs(&model.folder_path, &game.card_backs);
        let mut hovered = None;
        let mut clicked = None;
        let has_turned_cards = any_card_turned(visible.iter().copied());
        frame.card_pool_grid(
            "scendeckpool",
            visible.len(),
            x,
            row_y,
            width,
            y + height - row_y,
            12.0,
            1.0,
            has_turned_cards,
            |frame, tile| {
                let card = visible[tile.index];
                let region = frame.ui.region(
                    &format!("scendeck:card:{}", card.id),
                    tile.center_x - TILE_CARD_WIDTH / 2.0,
                    tile.y,
                    TILE_CARD_WIDTH,
                    TILE_CARD_HEIGHT + 6.0 + name_height,
                    RegionOptions::hand(),
                );
                let display = display_card(&model.folder_path, card);
                let draw = CardDraw::new(
                    Some(&display),
                    true,
                    tile.center_x,
                    tile.y + TILE_CARD_HEIGHT / 2.0,
                    &display_backs,
                    game.scenario_card_back_id.as_deref(),
                )
                .scale(TILE_SCALE)
                .angle(orientation_turns(card.orientation) * 2.0 * PI);
                frame.draw_card(&draw);
                let count =
                    entries.iter().find(|entry| entry.definition_id == card.id).map_or(0, |entry| entry.quantity);
                if count > 0 {
                    frame.draw_count_badge(
                        tile.center_x + TILE_CARD_WIDTH / 2.0 - 12.0,
                        tile.y + 12.0,
                        count as usize,
                        1.0,
                    );
                }
                if let Some(limit) = owned_quantity(card) {
                    frame.draw_count_badge(
                        tile.center_x - TILE_CARD_WIDTH / 2.0 + 12.0,
                        tile.y + 12.0,
                        limit as usize,
                        1.0,
                    );
                }
                let options = TextOptions::new().size(11.0).width(tile.width).centered().max_lines(NAME_LINES as usize);
                frame.text(&card.card_title, tile.x, tile.y + TILE_CARD_HEIGHT + 6.0, options);
                if region.hovered {
                    hovered = Some(card.id.clone());
                }
                if region.clicked {
                    clicked = Some(card.id.clone());
                }
            },
        );
        if hovered.is_some() {
            self.hovered_id = hovered;
        }
        if let Some(definition_id) = clicked {
            ScenarioTab::add_deck_copy(frame, model, index, deck_type, &definition_id);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_deck_list(
        frame: &mut Frame,
        model: &mut EditorModel,
        index: usize,
        deck_type: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let palette = frame.palette();
        let entries: Vec<DeckEntry> = model.game.scenarios[index].entries_for(deck_type).to_vec();
        if entries.is_empty() {
            let options =
                TextOptions::new().width(width).middle_of(height).centered().color(palette.on_surface_variant);
            frame.text("Click a card to add it", x, y, options);
            return;
        }
        let row_height = 48.0;
        let offset = frame.begin_scroll("scendecklist", x, y, width, height);
        let mut remove = None;
        for (row, entry) in entries.iter().enumerate() {
            let row_y = y + row as f32 * row_height - offset;
            if row_y + row_height > y && row_y < y + height {
                let title = model
                    .game
                    .card_by_id(&entry.definition_id)
                    .map_or(entry.definition_id.as_str(), |card| card.card_title.as_str());
                let region = frame.ui.region(
                    &format!("scendeckrow:{}", entry.definition_id),
                    x,
                    row_y,
                    width,
                    row_height,
                    RegionOptions::hand(),
                );
                if region.hovered {
                    frame.fill_rect(x, row_y, width, row_height, 0.0, with_alpha(palette.on_surface, 0.08));
                }
                let trailing = format!("×{}", entry.quantity);
                let trailing_width = frame.text_width(&trailing, 14.0, Weight::Regular);
                let options = TextOptions::new().middle_of(row_height).width(width - 48.0 - trailing_width).ellipsis();
                frame.text(title, x + 16.0, row_y, options);
                let options = TextOptions::new()
                    .width(width - 16.0)
                    .middle_of(row_height)
                    .right()
                    .color(palette.on_surface_variant);
                frame.text(&trailing, x, row_y, options);
                if region.clicked {
                    remove = Some(entry.definition_id.clone());
                }
            }
        }
        frame.end_scroll("scendecklist", entries.len() as f32 * row_height, &ScrollbarStyle::default());
        if let Some(definition_id) = remove {
            ScenarioTab::remove_deck_copy(&mut model.game.scenarios[index], deck_type, &definition_id);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_deck_builder_view(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        index: usize,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let palette = frame.palette();
        let slots = ScenarioTab::deck_slots(&model.game.scenarios[index]);
        if slots.is_empty() {
            let options =
                TextOptions::new().width(width).middle_of(height).centered().color(palette.on_surface_variant);
            frame.text(
                "Mark a zone's \"Loads Built Deck\" on in the Zones sub-view to build a deck for it here.",
                x,
                y,
                options,
            );
            return;
        }
        let scenario_key = self.keys.key(index);
        let current = self.active_deck_type.get(&scenario_key).cloned();
        let mut active = match current {
            Some(deck_type) if slots.iter().any(|(known, _)| *known == deck_type) => deck_type,
            _ => slots[0].0.clone(),
        };
        let scenario = &model.game.scenarios[index];
        let slot_label = |deck_type: &str, label: &str| {
            let count: i64 = scenario.entries_for(deck_type).iter().map(|entry| entry.quantity).sum();
            format!("{label} -- {count}{}", if count == 1 { " card" } else { " cards" })
        };
        let mut row_y = y;
        if slots.len() > 1 {
            let choices: Vec<Choice<String>> = slots
                .iter()
                .map(|(deck_type, label)| Choice::new(deck_type.clone(), slot_label(deck_type, label)))
                .collect();
            let options = DropdownOptions { height: Some(40.0), size: Some(14.0), ..DropdownOptions::default() };
            active = frame.dropdown("scen:deckslots", x + 12.0, row_y + 12.0, width - 24.0, &choices, active, &options);
            row_y += 12.0 + 40.0 + 12.0;
        } else {
            let label = slot_label(&slots[0].0, &slots[0].1);
            frame.text(&label, x + 12.0, row_y + 12.0, TextOptions::new().bold().width(width - 24.0).ellipsis());
            row_y += 12.0 + 20.0 + 12.0;
        }
        self.active_deck_type.insert(scenario_key, active.clone());
        frame.divider(x, row_y, width, None);
        row_y += 1.0;
        let deck_width = 240.0;
        let pool_width = width - deck_width - 1.0;
        frame.fill_rect(x + pool_width, row_y, 1.0, y + height - row_y, 0.0, palette.outline_variant);
        self.draw_deck_pool(frame, model, index, &active, x, row_y, pool_width, y + height - row_y);
        ScenarioTab::draw_deck_list(
            frame,
            model,
            index,
            &active,
            x + pool_width + 1.0,
            row_y,
            deck_width,
            y + height - row_y,
        );
    }

    // -----------------------------------------------------------------------
    // Card Placement sub-view
    // -----------------------------------------------------------------------

    /// A token placement's size on the table (`V.widgetInstanceSize` for an image token).
    fn token_size(frame: &mut Frame, folder: &str, placement: &ScenarioTokenPlacement) -> f32 {
        match placement.size_override {
            Some(WidgetSizeOverride::Multiplier(size)) => size as f32,
            Some(WidgetSizeOverride::NativeImageSize) => frame
                .images
                .get(&join_path(&[folder, "_tokens", &placement.image_path]))
                .map_or(IMAGE_TOKEN_SIZE, |texture| texture.width.max(texture.height)),
            None => IMAGE_TOKEN_SIZE,
        }
    }

    /// `V.tokenFace` for an image token: contain-fit and turned, or a faint circle without art.
    fn token_face(
        frame: &mut Frame,
        folder: &str,
        placement: &ScenarioTokenPlacement,
        center_x: f32,
        center_y: f32,
        size: f32,
    ) {
        let texture = frame.images.get(&join_path(&[folder, "_tokens", &placement.image_path]));
        let list = frame.ui.list();
        match texture {
            Some(texture) => {
                let scale = (size / texture.width).min(size / texture.height);
                list.set_color(Color::rgba(1.0, 1.0, 1.0, 1.0));
                list.draw_texture(
                    texture,
                    None,
                    center_x,
                    center_y,
                    placement.rotation_turns as f32 * PI / 2.0,
                    scale,
                    scale,
                    texture.width / 2.0,
                    texture.height / 2.0,
                );
            }
            None => {
                list.set_color(Color::rgba(1.0, 1.0, 1.0, 0.3));
                list.circle(DrawMode::Line, center_x, center_y, size / 2.0, None);
            }
        }
    }

    fn draw_placement_library(&mut self, frame: &mut Frame, model: &EditorModel) {
        let game = &model.game;
        let (screen_width, screen_height) = frame.ui.screen_size();
        let (panel_width, panel_height) =
            (1280f32.min(screen_width - 40.0), 720f32.min(screen_height - frame.ui.content_top - 40.0));
        frame.theme.force_dark = true;
        let response = frame.panel("scenplacelib", panel_width, panel_height, "Card Library", None, None);
        if response.close_requested {
            self.placement_library_open = false;
        }
        let content = response.content;
        let (x, y, width, height) = (content.x, content.y, content.width, content.height);
        let mut row_y = y;
        let has_scenario_cards = game.cards().iter().any(|card| card.is_scenario_card);
        if has_scenario_cards {
            self.placement_library_scenario = frame.segmented(
                "scenplacelib:mode",
                x + 16.0,
                row_y,
                width - 32.0,
                &scenario_mode_choices(),
                self.placement_library_scenario,
                Some(40.0),
                false,
            );
            row_y += 52.0;
        }
        let filters = self.placement_filters.get_or_insert_with(|| CardFilterState::new(game));
        let (filters_bottom, _) =
            frame.card_filters("scenplacelib", filters, game, x + 16.0, row_y, width - 32.0, false, &[]);
        row_y = frame.card_search(
            "scenplacelib",
            filters,
            game,
            x + 16.0,
            filters_bottom + 12.0,
            width - 32.0,
            Some(fixed::OVERLAY_PANEL),
            Some(fixed::WHITE),
        ) + 4.0;
        let mut visible: Vec<&CardDefinition> = filters.visible(game);
        if has_scenario_cards {
            visible.retain(|card| card.is_scenario_card == self.placement_library_scenario);
        }
        let name_height = frame.card_pool_name_height();
        let display_backs = display_card_backs(&model.folder_path, &game.card_backs);
        let mut hovered = None;
        let mut pressed = None;
        let has_turned_cards = any_card_turned(visible.iter().copied());
        frame.card_pool_grid(
            "scenplacelib:grid",
            visible.len(),
            x,
            row_y,
            width,
            y + height - row_y,
            12.0,
            1.0,
            has_turned_cards,
            |frame, tile| {
                let card = visible[tile.index];
                let region = frame.ui.region(
                    &format!("scenplacelib:card:{}", card.id),
                    tile.center_x - TILE_CARD_WIDTH / 2.0,
                    tile.y,
                    TILE_CARD_WIDTH,
                    TILE_CARD_HEIGHT + 6.0 + name_height,
                    RegionOptions::hand(),
                );
                let display = display_card(&model.folder_path, card);
                let draw = CardDraw::new(
                    Some(&display),
                    true,
                    tile.center_x,
                    tile.y + TILE_CARD_HEIGHT / 2.0,
                    &display_backs,
                    game.scenario_card_back_id.as_deref(),
                )
                .scale(TILE_SCALE)
                .angle(orientation_turns(card.orientation) * 2.0 * PI);
                frame.draw_card(&draw);
                let options = TextOptions::new()
                    .size(11.0)
                    .width(tile.width)
                    .centered()
                    .max_lines(NAME_LINES as usize)
                    .color(fixed::WHITE);
                frame.text(&card.card_title, tile.x, tile.y + TILE_CARD_HEIGHT + 6.0, options);
                if region.hovered {
                    hovered = Some(card.id.clone());
                }
                if region.pressed {
                    pressed = Some(card.id.clone());
                }
            },
        );
        frame.theme.force_dark = false;
        if hovered.is_some() {
            self.hovered_id = hovered;
        }
        if let Some(definition_id) = pressed {
            self.placing_draft = Some(definition_id);
            self.placement_library_open = false;
        }
    }

    /// Moves a held placement (or its Ctrl-drag clone) to the mouse, kept inside the canvas.
    fn drag_target(&self, tokens: bool, index: usize) -> usize {
        match self.clone_drag {
            Some(drag) if drag.tokens == tokens && drag.original == index => drag.clone,
            _ => index,
        }
    }

    /// Draws the scenario's zones set on the table (a card back for card zones, the widget for
    /// counters and token bags, with the name above) and lets them be selected and dragged
    /// anywhere on the table. Returns whether one was pressed this frame.
    fn draw_zone_placements(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        index: usize,
        (canvas_x, canvas_y): (f32, f32),
        scale: f32,
        display_backs: &[CardBackDefinition],
    ) -> bool {
        let (canvas_width, canvas_height) = (WORLD_WIDTH as f32 * scale, WORLD_HEIGHT as f32 * scale);
        let scenario_key = self.keys.key(index);
        let (mouse_x, mouse_y) = frame.ui.mouse();
        let scenario_card_back_id = model.game.scenario_card_back_id.clone();
        let mut pressed = false;
        let placements = model.game.scenarios[index].zone_placements.clone();
        for placement in &placements {
            let Some(zone) =
                model.game.scenarios[index].zones.iter().find(|zone| zone.id == placement.zone_id).cloned()
            else {
                continue;
            };
            let (world_width, world_height) = zone_world_size(&zone);
            let (world_width, world_height) =
                (world_width * placement.scale as f32, world_height * placement.scale as f32);
            let (box_width, box_height) = (world_width * scale, world_height * scale);
            let mut center =
                (canvas_x + placement.x as f32 * canvas_width, canvas_y + placement.y as f32 * canvas_height);
            let region = frame.ui.region(
                &format!("scen:zplace:{scenario_key}:{}", placement.zone_id),
                center.0 - box_width / 2.0,
                center.1 - box_height / 2.0,
                box_width,
                box_height,
                RegionOptions::hand(),
            );
            if region.pressed {
                pressed = true;
                self.selected_zone_placement = Some(placement.zone_id.clone());
            }
            if region.held {
                // Kept wholly on the reachable table.
                let (fraction_x, fraction_y) = reachable_table_fraction(
                    (mouse_x, mouse_y),
                    (canvas_x, canvas_y),
                    scale,
                    (world_width / 2.0, world_height / 2.0),
                );
                center = (canvas_x + fraction_x as f32 * canvas_width, canvas_y + fraction_y as f32 * canvas_height);
                let zone_placements = &mut model.game.scenarios[index].zone_placements;
                if let Some(stored) = zone_placements.iter_mut().find(|stored| stored.zone_id == placement.zone_id) {
                    (stored.x, stored.y) = (fraction_x, fraction_y);
                }
            }
            draw_zone_stand_in(
                frame,
                &model.folder_path,
                &zone,
                true,
                center,
                scale * placement.scale as f32,
                1.0,
                display_backs,
                scenario_card_back_id.as_deref(),
            );
            let label = placed_zone_label(&zone.name, placement.scale);
            draw_zone_label(frame, &label, center.0, center.1 - box_height / 2.0, 1.0);
            if self.selected_zone_placement.as_deref() == Some(placement.zone_id.as_str()) {
                frame.outline_rect(
                    center.0 - box_width / 2.0 - 2.0,
                    center.1 - box_height / 2.0 - 2.0,
                    box_width + 4.0,
                    box_height + 4.0,
                    4.0,
                    fixed::BLUE,
                    1.0,
                );
            }
        }
        pressed
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_placement_view(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        index: usize,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let palette = frame.palette();
        let folder = model.folder_path.clone();
        let game = model.game.clone();
        let scenario_key = self.keys.key(index);
        let placement_count = model.game.scenarios[index].placements.len();
        let token_count = model.game.scenarios[index].token_placements.len();
        if self.selected_placement.is_some_and(|placement| placement >= placement_count) {
            self.selected_placement = None;
        }
        if self.selected_token_placement.is_some_and(|token| token >= token_count) {
            self.selected_token_placement = None;
        }
        if let Some(zone_id) = &self.selected_zone_placement
            && model.game.scenarios[index].zone_placement_for(zone_id).is_none()
        {
            self.selected_zone_placement = None;
        }

        let toolbar_height = 40.0;
        let flip_options = ButtonOptions::variant(ButtonVariant::Outlined)
            .id("scen:place:flip")
            .disabled(self.selected_placement.is_none());
        let selected_widget_zone = self.selected_zone_placement.clone().filter(|zone_id| {
            model.game.scenarios[index].zones.iter().any(|zone| &zone.id == zone_id && zone.kind == ZoneKind::Widget)
        });
        let size_options = ButtonOptions::variant(ButtonVariant::Outlined)
            .id("scen:place:size")
            .disabled(self.selected_token_placement.is_none() && selected_widget_zone.is_none());
        let delete_options =
            ButtonOptions::variant(ButtonVariant::Outlined).icon("delete_outline").id("scen:place:delete").disabled(
                self.selected_placement.is_none()
                    && self.selected_token_placement.is_none()
                    && self.selected_zone_placement.is_none(),
            );
        let flip_width = frame.button_width("Flip", &flip_options);
        let size_width = frame.button_width("Size", &size_options);
        let delete_width = frame.button_width("Delete", &delete_options);
        let options = TextOptions::new()
            .middle_of(toolbar_height)
            .size(12.0)
            .color(palette.on_surface_variant)
            .width(width - 24.0 - flip_width - size_width - delete_width - 24.0);
        frame.text(
            "Right-click to add a card, token or scenario zone. Drag to move (Ctrl+drag to clone); Q/E rotates (Ctrl for 45°).",
            x + 12.0,
            y,
            options,
        );
        let flip_x = x + width - 12.0 - delete_width - 8.0 - size_width - 8.0 - flip_width;
        if frame.button("Flip", flip_x, y, Some(flip_width), Some(toolbar_height), &flip_options)
            && let Some(placement) = self.selected_placement
        {
            let placement = &mut model.game.scenarios[index].placements[placement];
            placement.face_up = !placement.face_up;
        }
        if frame.button(
            "Size",
            x + width - 12.0 - delete_width - 8.0 - size_width,
            y,
            Some(size_width),
            Some(toolbar_height),
            &size_options,
        ) {
            if let Some(token) = self.selected_token_placement {
                let current = model.game.scenarios[index].token_placements[token].size_override;
                let options = [
                    ("Default", None),
                    ("Medium", Some(IMAGE_TOKEN_SIZE as f64 * 2.0)),
                    ("Large", Some(IMAGE_TOKEN_SIZE as f64 * 4.0)),
                ];
                let mut items: Vec<MenuItem<TokenSizeChoice>> = options
                    .into_iter()
                    .map(|(label, size)| {
                        let checked = current == size.map(WidgetSizeOverride::Multiplier);
                        MenuItem::new(label, TokenSizeChoice::Preset(size)).checked(checked)
                    })
                    .collect();
                items.push(MenuItem::new("Set Scale...", TokenSizeChoice::SetScale));
                let (mouse_x, mouse_y) = frame.ui.mouse();
                self.size_menu = Some(ContextMenu::new("scen:place:sizemenu", mouse_x, mouse_y, items));
            } else if let Some(zone_id) = selected_widget_zone {
                let scenario = &model.game.scenarios[index];
                if let Some(placement) = scenario.zone_placement_for(&zone_id) {
                    let name = scenario.zones.iter().find(|zone| zone.id == zone_id).map_or("", |zone| &zone.name);
                    let dialog = WidgetScaleDialog::new("scen:place:scale", name, placement.scale);
                    self.scaling = Some((ScaledWidget::Zone(zone_id), dialog));
                }
            }
        }
        if frame.button(
            "Delete",
            x + width - 12.0 - delete_width,
            y,
            Some(delete_width),
            Some(toolbar_height),
            &delete_options,
        ) {
            let scenario = &mut model.game.scenarios[index];
            if let Some(placement) = self.selected_placement.take() {
                scenario.placements.remove(placement);
            } else if let Some(token) = self.selected_token_placement.take() {
                scenario.token_placements.remove(token);
            } else if let Some(zone_id) = self.selected_zone_placement.take() {
                scenario.zone_placements.retain(|placement| placement.zone_id != zone_id);
            }
        }

        // Second row: how many seats the game boards are laid out for, and the zoom controls.
        let view_row_y = y + toolbar_height + 8.0;
        let view_row_height = 32.0;
        if has_placed_boards(&game) {
            let label =
                TextOptions::new().size(12.0).medium().middle_of(view_row_height).color(palette.on_surface_variant);
            frame.text("Players", x + 12.0, view_row_y, label);
            let choices: Vec<Choice<usize>> =
                (1..=MAXIMUM_PREVIEW_PLAYERS).map(|count| Choice::new(count, count.to_string())).collect();
            self.board_preview_players = frame.segmented(
                "scen:place:players",
                x + 68.0,
                view_row_y,
                228.0,
                &choices,
                self.board_preview_players,
                Some(view_row_height),
                false,
            );
        }
        let zoom_controls_width =
            draw_zoom_controls(frame, &mut self.placement_camera, "scen:place", x + width - 12.0, view_row_y);
        let hint_x = x + 68.0 + 228.0 + 24.0;
        let hint = TextOptions::new()
            .size(12.0)
            .width((x + width - 12.0 - zoom_controls_width - 16.0 - hint_x).max(0.0))
            .ellipsis()
            .middle_of(view_row_height)
            .color(palette.on_surface_variant);
        frame.text("Wheel zooms · drag the felt to pan", hint_x, view_row_y, hint);

        // The canvas: the whole table under the camera, game boards as the table places them.
        let area_y = view_row_y + view_row_height + 8.0;
        let (area_x, area_width, area_height) = (x, width, (y + height - area_y).max(100.0));
        let (mouse_x, mouse_y) = frame.ui.mouse();
        let mut view = self.placement_camera.view(area_x, area_y, area_width, area_height);
        let wheel = frame.ui.input.wheel_y;
        if wheel != 0.0 && view.contains(mouse_x, mouse_y) && !frame.ui.modal_open() && !self.placement_library_open {
            self.placement_camera.zoom_at(&view, mouse_x, mouse_y, ZOOM_STEP.powf(wheel));
            frame.ui.consume_wheel();
            view = self.placement_camera.view(area_x, area_y, area_width, area_height);
        }
        if let Some((last_x, last_y)) = self.placement_pan {
            if frame.ui.input.is_down(1) || frame.ui.input.is_down(3) {
                self.placement_camera.pan_by(&view, mouse_x - last_x, mouse_y - last_y);
                self.placement_pan = Some((mouse_x, mouse_y));
                view = self.placement_camera.view(area_x, area_y, area_width, area_height);
            } else {
                self.placement_pan = None;
            }
        }
        let (canvas_x, canvas_y) = view.to_screen(0.0, 0.0);
        let scale = view.scale;
        let (canvas_width, canvas_height) = (WORLD_WIDTH as f32 * scale, WORLD_HEIGHT as f32 * scale);
        frame.fill_rect(area_x, area_y, area_width, area_height, 8.0, palette.surface_container_highest);
        frame.ui.push_clip(area_x, area_y, area_width, area_height);
        let felt = argb_color(TableColor::Felt.resolve(frame.settings, game.table_color.as_deref()));
        draw_preview_felt(frame, &view, felt, argb_color(SHARED_BOARD_BORDER_COLOR));
        draw_placed_boards(frame, &folder, &game, &view, self.board_preview_players);
        let canvas =
            frame.ui.region("scen:place:canvas", area_x, area_y, area_width, area_height, RegionOptions::default());
        let mut placement_pressed = false;
        if canvas.right_clicked {
            let (mouse_x, mouse_y) = frame.ui.mouse();
            self.placement_click =
                reachable_table_fraction((mouse_x, mouse_y), (canvas_x, canvas_y), scale, (0.0, 0.0));
            let scenario = &model.game.scenarios[index];
            let zones_off_the_table: Vec<MenuItem<AddToCanvas>> = scenario
                .zones
                .iter()
                .filter(|zone| scenario.zone_placement_for(&zone.id).is_none())
                .map(|zone| MenuItem::new(zone.name.clone(), AddToCanvas::Zone(zone.id.clone())))
                .collect();
            let mut items = vec![
                MenuItem::new("Add Card...", AddToCanvas::Card),
                MenuItem::new("Add Token...", AddToCanvas::Token),
            ];
            if zones_off_the_table.is_empty() {
                let label = if scenario.zones.is_empty() { "Add Zone (none yet)" } else { "Add Zone (all placed)" };
                items.push(MenuItem::new(label, AddToCanvas::Card).disabled(true));
            } else {
                items.push(MenuItem::submenu("Add Zone", zones_off_the_table));
            }
            self.placement_menu = Some(ContextMenu::new("scen:place:menu", mouse_x, mouse_y, items));
        }

        let display_backs = display_card_backs(&folder, &game.card_backs);
        // The scenario's zones set on the table, under the cards as on the table.
        let zone_pressed = self.draw_zone_placements(frame, model, index, (canvas_x, canvas_y), scale, &display_backs);
        let ctrl = frame.keyboard.ctrl();
        let mut card_clone = None;
        for placement_index in 0..placement_count {
            let placement = model.game.scenarios[index].placements[placement_index].clone();
            let Some(definition) = game.card_by_id(&placement.definition_id) else { continue };
            let (screen_x, screen_y) =
                (canvas_x + placement.x as f32 * canvas_width, canvas_y + placement.y as f32 * canvas_height);
            let total_turns = orientation_turns(definition.orientation) + placement.rotation_turns as f32 / 4.0;
            let (half_width, half_height) = half_extents(CARD_WIDTH, CARD_HEIGHT, total_turns);
            let (box_width, box_height) = (half_width * 2.0 * scale, half_height * 2.0 * scale);
            let region = frame.ui.region(
                &format!("scen:place:{scenario_key}:{placement_index}"),
                screen_x - box_width / 2.0,
                screen_y - box_height / 2.0,
                box_width,
                box_height,
                RegionOptions::hand(),
            );
            if region.hovered {
                self.hovered_id = Some(definition.id.clone());
                let orientation = shown_orientation(
                    Some(definition),
                    !placement.face_up,
                    &game.card_backs,
                    game.scenario_card_back_id.as_deref(),
                );
                self.hovered_placement_face = Some((definition.id.clone(), placement.face_up, orientation));
            }
            if region.pressed {
                placement_pressed = true;
                if ctrl {
                    // Ctrl+drag clones: the clone follows the drag, added after this loop.
                    let clone_index = placement_count + usize::from(card_clone.is_some());
                    card_clone = Some(placement.clone());
                    self.clone_drag = Some(CloneDrag { tokens: false, original: placement_index, clone: clone_index });
                    self.selected_placement = Some(clone_index);
                } else {
                    self.clone_drag = None;
                    self.selected_placement = Some(placement_index);
                }
                self.selected_token_placement = None;
            }
            if region.held {
                let target = self.drag_target(false, placement_index);
                let (fraction_x, fraction_y) = reachable_table_fraction(
                    (mouse_x, mouse_y),
                    (canvas_x, canvas_y),
                    scale,
                    (half_width, half_height),
                );
                let placements = &mut model.game.scenarios[index].placements;
                if let Some(target) = placements.get_mut(target) {
                    target.x = fraction_x;
                    target.y = fraction_y;
                } else if let Some(clone) = &mut card_clone {
                    clone.x = fraction_x;
                    clone.y = fraction_y;
                }
            }
            let display = display_card(&folder, definition);
            let draw = CardDraw::new(
                Some(&display),
                placement.face_up,
                screen_x,
                screen_y,
                &display_backs,
                game.scenario_card_back_id.as_deref(),
            )
            .scale(scale)
            .angle(total_turns * 2.0 * PI);
            frame.draw_card(&draw);
            if self.selected_placement == Some(placement_index) {
                frame.outline_rect(
                    screen_x - box_width / 2.0 - 2.0,
                    screen_y - box_height / 2.0 - 2.0,
                    box_width + 4.0,
                    box_height + 4.0,
                    4.0,
                    fixed::BLUE,
                    1.0,
                );
            }
            let options = TextOptions::new().width(box_width).centered().size(11.0).color(fixed::WHITE);
            frame.text(&definition.card_title, screen_x - box_width / 2.0, screen_y + box_height / 2.0 + 4.0, options);
        }
        if let Some(clone) = card_clone {
            model.game.scenarios[index].placements.push(clone);
        }

        let mut token_clone = None;
        for token_index in 0..token_count {
            let placement = model.game.scenarios[index].token_placements[token_index].clone();
            let size = ScenarioTab::token_size(frame, &folder, &placement);
            let (box_width, box_height) = (size * scale, size * scale);
            let (screen_x, screen_y) =
                (canvas_x + placement.x as f32 * canvas_width, canvas_y + placement.y as f32 * canvas_height);
            let region = frame.ui.region(
                &format!("scen:tplace:{scenario_key}:{token_index}"),
                screen_x - box_width / 2.0,
                screen_y - box_height / 2.0,
                box_width,
                box_height,
                RegionOptions::hand(),
            );
            if region.pressed {
                placement_pressed = true;
                if ctrl {
                    let clone_index = token_count;
                    token_clone = Some(placement.clone());
                    self.clone_drag = Some(CloneDrag { tokens: true, original: token_index, clone: clone_index });
                    self.selected_token_placement = Some(clone_index);
                } else {
                    self.clone_drag = None;
                    self.selected_token_placement = Some(token_index);
                }
                self.selected_placement = None;
            }
            if region.held {
                let target = self.drag_target(true, token_index);
                let (fraction_x, fraction_y) =
                    reachable_table_fraction((mouse_x, mouse_y), (canvas_x, canvas_y), scale, (size / 2.0, size / 2.0));
                let tokens = &mut model.game.scenarios[index].token_placements;
                if let Some(target) = tokens.get_mut(target) {
                    target.x = fraction_x;
                    target.y = fraction_y;
                } else if let Some(clone) = &mut token_clone {
                    clone.x = fraction_x;
                    clone.y = fraction_y;
                }
            }
            ScenarioTab::token_face(frame, &folder, &placement, screen_x, screen_y, box_width);
            if self.selected_token_placement == Some(token_index) {
                frame.outline_rect(
                    screen_x - box_width / 2.0 - 2.0,
                    screen_y - box_height / 2.0 - 2.0,
                    box_width + 4.0,
                    box_height + 4.0,
                    4.0,
                    fixed::BLUE,
                    1.0,
                );
            }
        }
        if let Some(clone) = token_clone {
            model.game.scenarios[index].token_placements.push(clone);
        }
        frame.ui.pop_clip();
        if placement_pressed {
            self.selected_zone_placement = None;
        } else if zone_pressed {
            self.selected_placement = None;
            self.selected_token_placement = None;
        }
        // A press on bare felt (or the middle button anywhere) starts panning.
        if (canvas.pressed && !placement_pressed && !zone_pressed) || (canvas.hovered && frame.ui.input.was_pressed(3))
        {
            self.placement_pan = Some((mouse_x, mouse_y));
        }

        if let Some(definition_id) = self.placing_draft.clone() {
            if let Some(definition) = game.card_by_id(&definition_id) {
                let display = display_card(&folder, definition);
                let draw = CardDraw::new(
                    Some(&display),
                    true,
                    mouse_x,
                    mouse_y,
                    &display_backs,
                    game.scenario_card_back_id.as_deref(),
                )
                .scale(scale)
                .angle(orientation_turns(definition.orientation) * 2.0 * PI)
                .alpha(0.85);
                frame.draw_card(&draw);
                if frame.ui.mouse_released(1) {
                    let (world_x, world_y) = view.to_world(mouse_x, mouse_y);
                    let inside = view.contains(mouse_x, mouse_y) && is_on_reachable_table(world_x, world_y);
                    if inside {
                        let turned = definition.orientation != CardOrientation::Portrait;
                        let (card_width, card_height) =
                            if turned { (CARD_HEIGHT, CARD_WIDTH) } else { (CARD_WIDTH, CARD_HEIGHT) };
                        let (fraction_x, fraction_y) = reachable_table_fraction(
                            (mouse_x, mouse_y),
                            (canvas_x, canvas_y),
                            scale,
                            (card_width / 2.0, card_height / 2.0),
                        );
                        let placements = &mut model.game.scenarios[index].placements;
                        placements.push(ScenarioPlacement::new(definition_id.clone(), fraction_x, fraction_y));
                        self.selected_placement = Some(placements.len() - 1);
                        self.selected_token_placement = None;
                    }
                    self.placing_draft = None;
                }
            } else {
                self.placing_draft = None;
            }
        }

        if self.placement_library_open {
            frame.overlay(|frame| self.draw_placement_library(frame, model));
        }
        if let Some(menu) = &mut self.placement_menu {
            match menu.show(frame) {
                MenuResponse::Open => {}
                MenuResponse::Closed => self.placement_menu = None,
                MenuResponse::Selected(action) => {
                    self.placement_menu = None;
                    match action {
                        AddToCanvas::Card => self.placement_library_open = true,
                        AddToCanvas::Zone(zone_id) => {
                            let (fraction_x, fraction_y) = self.placement_click;
                            let scenario = &mut model.game.scenarios[index];
                            if scenario.zone_placement_for(&zone_id).is_none() {
                                scenario.zone_placements.push(ScenarioZonePlacement::new(
                                    zone_id.clone(),
                                    fraction_x,
                                    fraction_y,
                                ));
                                self.selected_zone_placement = Some(zone_id);
                                self.selected_placement = None;
                                self.selected_token_placement = None;
                            }
                        }
                        AddToCanvas::Token => {
                            let (fraction_x, fraction_y) = self.placement_click;
                            let mut chooser_game = model.game.clone();
                            chooser_game.folder_path = Some(folder.clone());
                            self.chooser = Some((
                                TokenChooser::for_game(&chooser_game),
                                ChooserTarget::TokenPlacement { scenario: scenario_key, x: fraction_x, y: fraction_y },
                            ));
                        }
                    }
                }
            }
        }
        if let Some(menu) = &mut self.size_menu {
            match menu.show(frame) {
                MenuResponse::Open => {}
                MenuResponse::Closed => self.size_menu = None,
                MenuResponse::Selected(choice) => {
                    self.size_menu = None;
                    if let Some(token) = self.selected_token_placement
                        && let Some(placement) = model.game.scenarios[index].token_placements.get_mut(token)
                    {
                        match choice {
                            TokenSizeChoice::Preset(size) => {
                                placement.size_override = size.map(WidgetSizeOverride::Multiplier);
                            }
                            TokenSizeChoice::SetScale => {
                                let name = token_display_name(&placement.image_path);
                                let scale = token_scale(placement.size_override);
                                let dialog = WidgetScaleDialog::new("scen:place:scale", &name, scale);
                                self.scaling = Some((ScaledWidget::Token(token), dialog));
                            }
                        }
                    }
                }
            }
        }
        if let Some((widget, dialog)) = &mut self.scaling {
            let (scale, closed) = match dialog.show(frame) {
                WidgetScaleResponse::Open(scale) => (scale, false),
                WidgetScaleResponse::Closed(scale) => (scale, true),
            };
            let scenario = &mut model.game.scenarios[index];
            let still_there = match widget {
                ScaledWidget::Zone(zone_id) => {
                    let placement = scenario.zone_placements.iter_mut().find(|placement| &placement.zone_id == zone_id);
                    placement.map(|placement| placement.scale = scale).is_some()
                }
                ScaledWidget::Token(token) => scenario
                    .token_placements
                    .get_mut(*token)
                    .map(|placement| placement.size_override = token_size_override_for_scale(scale))
                    .is_some(),
            };
            if closed || !still_there {
                self.scaling = None;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Preview and tab
    // -----------------------------------------------------------------------

    /// `T:drawPreview`: Space shows the hovered placement exactly as on the canvas, or a hovered
    /// pool/library card's front and back.
    fn draw_preview(&self, frame: &mut Frame, model: &EditorModel) {
        if frame.ui.any_overlay_open() || frame.ui.focus.is_some() || !frame.keyboard.is_down(&["space"]) {
            return;
        }
        let game = &model.game;
        let display_backs = display_card_backs(&model.folder_path, &game.card_backs);
        let (screen_width, screen_height) = frame.ui.screen_size();
        let (mouse_x, _) = frame.ui.mouse();
        let scenario_back = game.scenario_card_back_id.as_deref();
        if let Some((definition_id, face_up, orientation)) = &self.hovered_placement_face
            && let Some(definition) = game.card_by_id(definition_id)
        {
            let display = display_card(&model.folder_path, definition);
            let faces = [PreviewFace { definition: Some(&display), face_up: *face_up, orientation: *orientation }];
            frame.draw_card_preview(&faces, &display_backs, screen_width, screen_height, mouse_x, scenario_back);
            return;
        }
        let Some(definition) = self.hovered_id.as_deref().and_then(|id| game.card_by_id(id)) else { return };
        let display = display_card(&model.folder_path, definition);
        let faces = definition_faces(&display, &display_backs, scenario_back);
        frame.draw_card_preview(&faces, &display_backs, screen_width, screen_height, mouse_x, scenario_back);
    }

    pub fn draw(&mut self, frame: &mut Frame, model: &mut EditorModel, x: f32, y: f32, width: f32, height: f32) {
        self.keys.sync(model.game.scenarios.len());
        self.pending_remove = None;
        self.hovered_id = None;
        self.hovered_placement_face = None;
        let keys: Vec<u64> = (0..model.game.scenarios.len()).map(|index| self.keys.key(index)).collect();
        clamp_selection(&mut self.selected, &keys);
        let selected_index = self.selected_index();
        frame.list_detail(
            x,
            y,
            width,
            height,
            300.0,
            selected_index.is_some(),
            "Add a scenario to get started",
            |frame, pane| match pane {
                Pane::List(rect) => self.draw_scenario_list(frame, model, rect.x, rect.y, rect.width, rect.height),
                Pane::Detail(rect) => {
                    if let Some(index) = selected_index {
                        self.draw_scenario_detail(frame, model, index, rect.x, rect.y, rect.width, rect.height);
                    }
                }
            },
        );
        if let Some(index) = self.pending_remove.take() {
            model.game.scenarios.remove(index);
            self.keys.removed(index);
            let count = model.game.scenarios.len();
            self.selected = (count > 0).then(|| self.keys.key(index.min(count - 1)));
        }
        frame.overlay(|frame| self.draw_preview(frame, model));
        self.show_dialogs(frame, model);
    }

    fn show_dialogs(&mut self, frame: &mut Frame, model: &mut EditorModel) {
        if let Some(menu) = &mut self.duplicate_menu {
            match menu.show(frame) {
                MenuResponse::Open => {}
                MenuResponse::Closed => self.duplicate_menu = None,
                MenuResponse::Selected(index) => {
                    self.duplicate_menu = None;
                    if index < model.game.scenarios.len() {
                        self.duplicate_scenario(model, index);
                    }
                }
            }
        }
        if let Some((import, key)) = &mut self.guide_import {
            let key = *key;
            match import.show(frame) {
                ImportResponse::Open => {}
                ImportResponse::Cancelled => self.guide_import = None,
                ImportResponse::Imported(file) => {
                    self.guide_import = None;
                    if let Some(index) = self.keys.index_of(key) {
                        model.game.scenarios[index].guide_path = Some(file);
                    }
                }
            }
        }
        if let Some((chooser, _)) = &mut self.chooser {
            match chooser.show(frame) {
                ChooserResponse::Open => {}
                ChooserResponse::Cancelled => self.chooser = None,
                ChooserResponse::Picked(file) => {
                    let Some((_, target)) = self.chooser.take() else { return };
                    match target {
                        ChooserTarget::Zone { scenario, zone, pick } => {
                            let zone_index = self.zones_views.get(&scenario).and_then(|view| view.keys.index_of(zone));
                            if let (Some(index), Some(zone_index)) = (self.keys.index_of(scenario), zone_index) {
                                pick.apply(&mut model.game.scenarios[index].zones[zone_index], file);
                            }
                        }
                        ChooserTarget::TokenPlacement { scenario, x, y } => {
                            if let Some(index) = self.keys.index_of(scenario) {
                                let tokens = &mut model.game.scenarios[index].token_placements;
                                tokens.push(ScenarioTokenPlacement::new(file, x, y));
                                self.selected_token_placement = Some(tokens.len() - 1);
                                self.selected_placement = None;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A number as Lua's `tostring` writes it.
/// The table position (fractions of the table, which may go past 0..1 into the margin players
/// can reach) under a screen point on the placement canvas, kept so something with these half
/// extents (world units) stays wholly on the reachable table. `canvas` is where the table's top
/// left corner is on screen and `scale` is screen pixels per world unit.
fn reachable_table_fraction(
    (screen_x, screen_y): (f32, f32),
    (canvas_x, canvas_y): (f32, f32),
    scale: f32,
    (half_width, half_height): (f32, f32),
) -> (f64, f64) {
    let (world_x, world_y) = ((screen_x - canvas_x) / scale, (screen_y - canvas_y) / scale);
    let (center_x, center_y) = clamp_to_reachable_table(world_x, world_y, half_width, half_height);
    (f64::from(center_x / WORLD_WIDTH as f32), f64::from(center_y / WORLD_HEIGHT as f32))
}

/// An image token's name for the Set Scale title: its file name without folders or extension.
fn token_display_name(image_path: &str) -> String {
    let file_name = image_path.rsplit(['/', '\\']).next().unwrap_or(image_path);
    file_name.rsplit_once('.').map_or(file_name, |(stem, _)| stem).to_string()
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 { format!("{}", value as i64) } else { value.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::harness::UiHarness;
    use crate::screens::gamedef::common::StableKeys;
    use gametable_core::models::{BoardTablePlacement, GameBoardDefinition, GameDefinition, ZoneDefinition};

    fn model_with_a_placed_card_and_a_board() -> EditorModel {
        let mut game = GameDefinition::new("game", "Game");
        game.cards_mut().push(CardDefinition::new("card", "Card"));
        let mut board = GameBoardDefinition::new("missing.png");
        board.table_placement = Some(BoardTablePlacement::new(1280.0, 720.0));
        game.game_boards.push(board);
        let mut scenario = Scenario::new("scenario", "Scenario");
        scenario.placements.push(ScenarioPlacement::new("card", 0.5, 0.5));
        game.scenarios.push(scenario);
        EditorModel { folder_path: String::new(), game, card_keys: StableKeys::default() }
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
    fn a_scenario_zone_is_added_from_the_right_click_menu_and_dragged_anywhere() {
        let mut harness = UiHarness::new();
        let mut model = model_with_a_placed_card_and_a_board();
        let boss_deck = ZoneDefinition { shared: true, ..ZoneDefinition::new("boss", "Boss Deck") };
        model.game.scenarios[0].zones.push(boss_deck);
        let mut tab = ScenarioTab::new();
        tab.set_view_for_development("placement");
        tab.keys.sync(1);
        let zone_region = format!("scen:zplace:{}:boss", tab.keys.key(0));
        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 0.0, 1280.0, 800.0);
        harness.frame(|frame| draw(frame));
        harness.frame(|frame| draw(frame));

        // Right-click an empty spot of the table, then Add Zone > Boss Deck.
        let (canvas_x, canvas_y) = harness.region_center("scen:place:canvas").unwrap();
        let spot = (canvas_x - 250.0, canvas_y - 120.0);
        harness.move_mouse(spot.0, spot.1);
        harness.frame(|frame| draw(frame));
        harness.press(spot.0, spot.1, 2, 1);
        harness.frame(|frame| draw(frame));
        harness.release(spot.0, spot.1, 2);
        harness.frame(|frame| draw(frame));
        harness.click_region("scen:place:menu:menu1:3:Add Zone", &mut draw);
        harness.click_region("scen:place:menu:menu2:1:Boss Deck", &mut draw);
        harness.frame(|frame| draw(frame));
        let placed = harness.region_center(&zone_region).expect("the zone is on the canvas");
        assert!((placed.0 - spot.0).abs() < 1.0 && (placed.1 - spot.1).abs() < 1.0, "{spot:?} -> {placed:?}");

        // It drags anywhere on the table.
        drag(&mut harness, &mut draw, placed, (placed.0 + 300.0, placed.1 + 200.0));
        let dragged = harness.region_center(&zone_region).unwrap();
        assert!((dragged.0 - (placed.0 + 300.0)).abs() < 1.0 && (dragged.1 - (placed.1 + 200.0)).abs() < 1.0);

        // Selected by the drag, Delete takes it off the table again (the zone itself stays).
        harness.click_region("scen:place:delete", &mut draw);
        harness.frame(|frame| draw(frame));
        let scenario = &model.game.scenarios[0];
        assert!(scenario.zone_placements.is_empty());
        assert_eq!(scenario.zones.len(), 1);
    }

    #[test]
    fn the_placement_canvas_zooms_and_pans_like_the_table_preview() {
        let mut harness = UiHarness::new();
        let mut model = model_with_a_placed_card_and_a_board();
        let mut tab = ScenarioTab::new();
        tab.set_view_for_development("placement");
        tab.keys.sync(1);
        let card_id = format!("scen:place:{}:0", tab.keys.key(0));
        let mut draw = |frame: &mut Frame| tab.draw(frame, &mut model, 0.0, 0.0, 1280.0, 800.0);
        harness.frame(|frame| draw(frame));
        harness.frame(|frame| draw(frame));
        let card = harness.region_center(&card_id).expect("the placed card");
        assert!(harness.region_center("scen:place:players:3").is_some(), "a board is placed, so seats can be picked");

        // The wheel zooms around the mouse: the card under it stays put.
        harness.move_mouse(card.0, card.1);
        harness.frame(|frame| draw(frame));
        harness.wheel(2.0);
        harness.frame(|frame| draw(frame));
        harness.frame(|frame| draw(frame));
        let zoomed = harness.region_center(&card_id).unwrap();
        assert!((zoomed.0 - card.0).abs() < 1.0 && (zoomed.1 - card.1).abs() < 1.0, "{card:?} -> {zoomed:?}");

        // Dragging the felt pans the view, not the card.
        let felt = (card.0 + 300.0, card.1);
        drag(&mut harness, &mut draw, felt, (felt.0 - 100.0, felt.1));
        let panned = harness.region_center(&card_id).unwrap();
        assert!((panned.0 - (card.0 - 100.0)).abs() < 1.0, "{card:?} -> {panned:?}");

        // Dragging the card still drops it where the mouse lets go.
        drag(&mut harness, &mut draw, panned, (panned.0 + 60.0, panned.1));
        let dropped = harness.region_center(&card_id).unwrap();
        assert!((dropped.0 - (panned.0 + 60.0)).abs() < 1.0, "{panned:?} -> {dropped:?}");

        // The percentage fits the whole table again.
        harness.click_region("scen:place:zoom:fit", &mut draw);
        assert_eq!(tab.placement_camera, PreviewCamera::default());
        let placed = &model.game.scenarios[0].placements[0];
        assert!(placed.x > 0.5 && (placed.y - 0.5).abs() < 1e-6, "only the card drag moved it: {placed:?}");
    }

    #[test]
    fn a_token_scale_is_its_size_against_the_default_and_back() {
        assert_eq!(token_scale(None), 1.0);
        assert_eq!(token_size_override_for_scale(1.0), None);
        let doubled = token_size_override_for_scale(2.0);
        assert_eq!(doubled, Some(WidgetSizeOverride::Multiplier(f64::from(IMAGE_TOKEN_SIZE) * 2.0)));
        assert_eq!(token_scale(doubled), 2.0);
        assert_eq!(token_display_name("bags/Clue Token.png"), "Clue Token");
    }
}
