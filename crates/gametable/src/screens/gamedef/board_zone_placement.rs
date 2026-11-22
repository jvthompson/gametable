//! Zones placed on a game board, edited in the Game Boards tab's Board Preview (a GameTable
//! addition). Right-clicking the preview offers "Add Zone...", a list of the zones that can still
//! go on the selected board; the picked zone lands where the preview was right-clicked (or in the
//! board's centre). Placed zones are drawn on the preview (a card back for card
//! zones, the widget for counters and token bags) with their name as a small label above, and can
//! be dragged around the board, dragged off it to remove them, or removed with their × button.
//! Right-clicking a placed zone offers "Set Scale..." for widgets (counters and token bags), so
//! each can be drawn bigger or smaller than usual, and "Take Off the Board".

use std::f32::consts::PI;

use gametable_core::models::{
    BoardKind, BoardZonePlacement, CardDefinition, GameDefinition, ZoneDefinition, ZoneKind, ZoneWidgetKind,
    zone_fits_board_kind,
};

use crate::app::Frame;
use crate::data::file_system::join_path;
use crate::graphics::draw_list::Color;
use crate::graphics::tessellation::DrawMode;
use crate::screens::gamedef::EditorModel;
use crate::screens::gamedef::common::display_card_backs;
use crate::screens::gamedef::list_picker_dialog::{ListPickerDialog, PickerEntry, PickerResponse};
use crate::screens::gamedef::widget_scale_dialog::{WidgetScaleDialog, WidgetScaleResponse, scale_percent_text};
use crate::table::card_render::{CARD_HEIGHT, CARD_WIDTH, CardDraw, orientation_turns};
use crate::table::panels::lua_number_text;
use crate::table::view::COUNTER_SIZE;
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::overlays::{ContextMenu, MenuItem, MenuResponse};
use crate::ui::theme::with_alpha;
use crate::ui::widgets::IconButtonOptions;

/// The board's rectangle on screen and the preview's world-to-screen scale.
#[derive(Debug, Clone, Copy)]
pub struct BoardScreenRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub preview_scale: f32,
}

impl BoardScreenRect {
    fn point_at(&self, fraction_x: f64, fraction_y: f64) -> (f32, f32) {
        (self.x + fraction_x as f32 * self.width, self.y + fraction_y as f32 * self.height)
    }

    pub fn fraction_at(&self, screen_x: f32, screen_y: f32) -> (f64, f64) {
        (f64::from((screen_x - self.x) / self.width), f64::from((screen_y - self.y) / self.height))
    }
}

/// A zone that can be placed on the selected kind of board.
#[derive(Debug, Clone)]
pub struct PlaceableZone {
    pub zone: ZoneDefinition,
    /// Shown on the chip: the zone name, or "Scenario: <scenario> — <zone>".
    pub chip_label: String,
    pub is_scenario_zone: bool,
}

/// Every zone that fits this kind of board: the game's own zones, and for a shared board also
/// every scenario's zones (scenario zones are always shared).
pub fn placeable_zones(game: &GameDefinition, kind: BoardKind) -> Vec<PlaceableZone> {
    let mut zones: Vec<PlaceableZone> = game
        .zones
        .iter()
        .filter(|zone| zone_fits_board_kind(zone, kind))
        .map(|zone| PlaceableZone { zone: zone.clone(), chip_label: zone.name.clone(), is_scenario_zone: false })
        .collect();
    if kind == BoardKind::Shared {
        for scenario in &game.scenarios {
            for zone in &scenario.zones {
                if zones.iter().any(|placeable| placeable.zone.id == zone.id) {
                    continue;
                }
                zones.push(PlaceableZone {
                    zone: zone.clone(),
                    chip_label: format!("Scenario: {} — {}", scenario.name, zone.name),
                    is_scenario_zone: true,
                });
            }
        }
    }
    zones
}

/// The zone's size in world units, turned for landscape orientations.
pub(super) fn zone_world_size(zone: &ZoneDefinition) -> (f32, f32) {
    if zone.kind == ZoneKind::Widget {
        return (COUNTER_SIZE, COUNTER_SIZE);
    }
    if orientation_turns(zone.orientation) == 0.0 { (CARD_WIDTH, CARD_HEIGHT) } else { (CARD_HEIGHT, CARD_WIDTH) }
}

/// A zone being dragged around the board: which placement, and where on the zone it was grabbed
/// (as an offset from its centre, in screen pixels).
struct DraggedZone {
    zone_id: String,
    grab_offset: (f32, f32),
}

/// The Set Scale dialog open for one placed zone (by zone id).
struct ScalingZone {
    zone_id: String,
    dialog: WidgetScaleDialog,
}

#[derive(Default)]
pub struct ZonePlacementEditor {
    dragged: Option<DraggedZone>,
    menu: Option<ContextMenu<ZoneMenuAction>>,
    picker: Option<ListPickerDialog>,
    /// Where on the board (fractions) a zone picked from the open picker goes.
    add_at: (f64, f64),
    scaling: Option<ScalingZone>,
}

/// The zones that fit the board and aren't on it yet.
fn unplaced_zones(model: &EditorModel, board_index: usize) -> Vec<PlaceableZone> {
    let Some(board) = model.game.game_boards.get(board_index) else { return Vec::new() };
    placeable_zones(&model.game, board.kind)
        .into_iter()
        .filter(|candidate| board.placement_for(&candidate.zone.id).is_none())
        .collect()
}

/// The picker's second line for a zone: what kind it is, and its scenario.
fn zone_description(candidate: &PlaceableZone) -> String {
    let kind = match (candidate.zone.kind, candidate.zone.widget_kind) {
        (ZoneKind::Card, _) => "Card zone",
        (ZoneKind::Widget, ZoneWidgetKind::Counter) => "Counter",
        (ZoneKind::Widget, ZoneWidgetKind::TokenBag) => "Token bag",
    };
    match candidate.chip_label.strip_prefix("Scenario: ") {
        Some(scenario) => format!("{kind} · Scenario: {}", scenario.split(" — ").next().unwrap_or(scenario)),
        None => kind.to_string(),
    }
}

/// What the Board Preview's right-click menus do: the board's own menu adds a zone, a placed
/// zone's menu scales it or takes it off the board.
#[derive(Debug, Clone, PartialEq)]
enum ZoneMenuAction {
    AddZone,
    SetScale(String),
    TakeOff(String),
}

/// A placed zone's right-click menu: Set Scale for widgets, then Take Off the Board.
fn placed_zone_menu_items(zone_id: &str, is_widget: bool) -> Vec<MenuItem<ZoneMenuAction>> {
    let mut items = Vec::new();
    if is_widget {
        items.push(MenuItem::new("Set Scale...", ZoneMenuAction::SetScale(zone_id.to_string())));
    }
    items.push(MenuItem::new("Take Off the Board", ZoneMenuAction::TakeOff(zone_id.to_string())));
    items
}

/// The name label of a placed zone, with its scale when that isn't the usual size.
pub(super) fn placed_zone_label(name: &str, scale: f64) -> String {
    if (scale - 1.0).abs() < 1e-6 { name.to_string() } else { format!("{name} · {}", scale_percent_text(scale)) }
}

pub const ADD_ZONE_DIALOG: &str = "gb:zones:add";

impl ZonePlacementEditor {
    /// The Board Preview was right-clicked at `(screen_x, screen_y)`: offers "Add Zone...", which
    /// places the picked zone there when that is on the board (in the board's middle otherwise).
    pub fn open_menu(
        &mut self,
        model: &EditorModel,
        board_index: usize,
        board_rect: BoardScreenRect,
        screen_x: f32,
        screen_y: f32,
    ) {
        // A placed zone's own menu was opened by this right-click.
        if self.menu.is_some() {
            return;
        }
        let (fraction_x, fraction_y) = board_rect.fraction_at(screen_x, screen_y);
        let on_board = (0.0..=1.0).contains(&fraction_x) && (0.0..=1.0).contains(&fraction_y);
        self.add_at = if on_board { (fraction_x, fraction_y) } else { (0.5, 0.5) };
        let nothing_to_add = unplaced_zones(model, board_index).is_empty();
        let items = vec![MenuItem::new("Add Zone...", ZoneMenuAction::AddZone).disabled(nothing_to_add)];
        self.menu = Some(ContextMenu::new("gb:zones:menu", screen_x, screen_y, items));
    }

    /// Shows the open menu and zone picker, and places a picked zone.
    pub fn show_menu_and_picker(&mut self, frame: &mut Frame, model: &mut EditorModel, board_index: usize) {
        if let Some(menu) = &mut self.menu {
            match menu.show(frame) {
                MenuResponse::Open => {}
                MenuResponse::Closed => self.menu = None,
                MenuResponse::Selected(ZoneMenuAction::AddZone) => {
                    self.menu = None;
                    let entries: Vec<PickerEntry> = unplaced_zones(model, board_index)
                        .into_iter()
                        .map(|candidate| {
                            let entry = PickerEntry::new(candidate.zone.id.clone(), candidate.zone.name.clone());
                            entry.detail(zone_description(&candidate))
                        })
                        .collect();
                    let empty = match model.game.game_boards.get(board_index).map(|board| board.kind) {
                        Some(BoardKind::Shared) => {
                            "Add shared zones or scenario zones to place them on a shared board."
                        }
                        _ => "Add player zones (not shared) on the Zones tab to place them here.",
                    };
                    self.picker = Some(ListPickerDialog::new(ADD_ZONE_DIALOG, "Add Zone", empty, entries));
                }
                MenuResponse::Selected(ZoneMenuAction::SetScale(zone_id)) => {
                    self.menu = None;
                    let board = &model.game.game_boards[board_index];
                    if let Some(placement) = board.placement_for(&zone_id) {
                        let name = zone_name(&model.game, &zone_id);
                        let dialog = WidgetScaleDialog::new("gb:zones:scale", &name, placement.scale);
                        self.scaling = Some(ScalingZone { zone_id, dialog });
                    }
                }
                MenuResponse::Selected(ZoneMenuAction::TakeOff(zone_id)) => {
                    self.menu = None;
                    model.game.game_boards[board_index]
                        .zone_placements
                        .retain(|placement| placement.zone_id != zone_id);
                }
            }
        }
        if let Some(scaling) = &mut self.scaling {
            let (scale, closed) = match scaling.dialog.show(frame) {
                WidgetScaleResponse::Open(scale) => (scale, false),
                WidgetScaleResponse::Closed(scale) => (scale, true),
            };
            let placements = &mut model.game.game_boards[board_index].zone_placements;
            match placements.iter_mut().find(|placement| placement.zone_id == scaling.zone_id) {
                Some(placement) => placement.scale = scale,
                None => self.scaling = None,
            }
            if closed {
                self.scaling = None;
            }
        }
        if let Some(picker) = &mut self.picker {
            match picker.show(frame) {
                PickerResponse::Open => {}
                PickerResponse::Cancelled => self.picker = None,
                PickerResponse::Picked(zone_id) => {
                    self.picker = None;
                    let (x, y) = self.add_at;
                    if let Some(board) = model.game.game_boards.get_mut(board_index)
                        && board.placement_for(&zone_id).is_none()
                    {
                        board.zone_placements.push(BoardZonePlacement::new(zone_id, x, y));
                    }
                }
            }
        }
    }

    /// Draws the placed zones on the previewed board and handles dragging and removing them.
    pub fn draw_placed_zones(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        board_index: usize,
        mat: BoardScreenRect,
    ) {
        let placeable = placeable_zones(&model.game, model.game.game_boards[board_index].kind);
        let display_backs = display_card_backs(&model.folder_path, &model.game.card_backs);
        let scenario_card_back_id = model.game.scenario_card_back_id.clone();
        let mut removed_zone_id = None;
        let placements = model.game.game_boards[board_index].zone_placements.clone();
        for placement in &placements {
            let fitting = placeable.iter().find(|candidate| candidate.zone.id == placement.zone_id);
            let known_zone = fitting.map(|candidate| candidate.zone.clone()).or_else(|| {
                model
                    .game
                    .zones
                    .iter()
                    .chain(model.game.scenarios.iter().flat_map(|scenario| &scenario.zones))
                    .find(|zone| zone.id == placement.zone_id)
                    .cloned()
            });
            let (world_width, world_height) = known_zone.as_ref().map_or((CARD_WIDTH, CARD_HEIGHT), zone_world_size);
            let zone_scale = mat.preview_scale * placement.scale as f32;
            let (zone_width, zone_height) = (world_width * zone_scale, world_height * zone_scale);
            let is_dragged = self.dragged.as_ref().is_some_and(|dragged| dragged.zone_id == placement.zone_id);
            let mut center = mat.point_at(placement.x, placement.y);
            if is_dragged && let Some(dragged) = &self.dragged {
                let (mouse_x, mouse_y) = frame.ui.mouse();
                center = (mouse_x - dragged.grab_offset.0, mouse_y - dragged.grab_offset.1);
            }
            let region = frame.ui.region(
                &format!("gb:zone:{}", placement.zone_id),
                center.0 - zone_width / 2.0,
                center.1 - zone_height / 2.0,
                zone_width,
                zone_height,
                RegionOptions::cursor("hand"),
            );
            if region.pressed {
                let (mouse_x, mouse_y) = frame.ui.mouse();
                self.dragged = Some(DraggedZone {
                    zone_id: placement.zone_id.clone(),
                    grab_offset: (mouse_x - center.0, mouse_y - center.1),
                });
            }
            if region.right_clicked {
                let (mouse_x, mouse_y) = frame.ui.mouse();
                let is_widget = known_zone.as_ref().is_some_and(|zone| zone.kind == ZoneKind::Widget);
                let items = placed_zone_menu_items(&placement.zone_id, is_widget);
                self.menu = Some(ContextMenu::new("gb:zone:menu", mouse_x, mouse_y, items));
            }
            let off_the_board =
                !(mat.x..=mat.x + mat.width).contains(&center.0) || !(mat.y..=mat.y + mat.height).contains(&center.1);
            if is_dragged && !region.held {
                self.dragged = None;
                if off_the_board {
                    removed_zone_id = Some(placement.zone_id.clone());
                    continue;
                }
                let (fraction_x, fraction_y) = mat.fraction_at(center.0, center.1);
                if let Some(stored) = model.game.game_boards[board_index]
                    .zone_placements
                    .iter_mut()
                    .find(|stored| stored.zone_id == placement.zone_id)
                {
                    stored.x = fraction_x.clamp(0.0, 1.0);
                    stored.y = fraction_y.clamp(0.0, 1.0);
                }
            }
            let alpha = match (fitting.is_some(), is_dragged && off_the_board) {
                (false, _) => 0.4,
                (true, true) => 0.5,
                (true, false) => 1.0,
            };
            match &known_zone {
                Some(zone) => {
                    let is_scenario_zone = fitting.is_some_and(|candidate| candidate.is_scenario_zone);
                    draw_zone_stand_in(
                        frame,
                        &model.folder_path,
                        zone,
                        is_scenario_zone,
                        center,
                        zone_scale,
                        alpha,
                        &display_backs,
                        scenario_card_back_id.as_deref(),
                    );
                }
                None => {
                    frame.outline_rect(
                        center.0 - zone_width / 2.0,
                        center.1 - zone_height / 2.0,
                        zone_width,
                        zone_height,
                        4.0,
                        Color::rgba(1.0, 1.0, 1.0, 0.5),
                        1.0,
                    );
                }
            }
            if fitting.is_none() {
                let size = (zone_width.min(zone_height) * 0.5).clamp(14.0, 32.0);
                frame.icon(
                    "warning_amber",
                    center.0 - size / 2.0,
                    center.1 - size / 2.0,
                    size,
                    Color::rgba(1.0, 0.76, 0.03, 1.0),
                );
            }
            let label = match (&known_zone, fitting.is_some()) {
                (Some(zone), true) => placed_zone_label(&zone.name, placement.scale),
                (Some(zone), false) => format!("{} (doesn't fit this kind of board)", zone.name),
                (None, _) => "Deleted zone".to_string(),
            };
            draw_zone_label(frame, &label, center.0, center.1 - zone_height / 2.0, alpha);
            // The × sits on the zone's top-right corner and stays while the mouse is on either.
            let button_size = 22.0;
            let button_x = center.0 + zone_width / 2.0 - button_size * 0.6;
            let button_y = center.1 - zone_height / 2.0 - button_size * 0.4;
            let (mouse_x, mouse_y) = frame.ui.mouse();
            let over_button = (button_x..=button_x + button_size).contains(&mouse_x)
                && (button_y..=button_y + button_size).contains(&mouse_y);
            if (region.hovered || over_button) && self.dragged.is_none() {
                let id = format!("gb:unplace:{}", placement.zone_id);
                let options = IconButtonOptions {
                    background: Some(Color::rgba(0.0, 0.0, 0.0, 0.7)),
                    ..IconButtonOptions::tooltip("Take off the board")
                        .id(&id)
                        .size(button_size, 16.0)
                        .color(Color::WHITE)
                };
                if frame.icon_button("close", button_x, button_y, &options) {
                    removed_zone_id = Some(placement.zone_id.clone());
                }
            }
        }
        if let Some(zone_id) = removed_zone_id {
            model.game.game_boards[board_index].zone_placements.retain(|placement| placement.zone_id != zone_id);
        }
    }
}

/// A zone's name by id, from the game's zones or any scenario's.
fn zone_name(game: &GameDefinition, zone_id: &str) -> String {
    game.zones
        .iter()
        .chain(game.scenarios.iter().flat_map(|scenario| &scenario.zones))
        .find(|zone| zone.id == zone_id)
        .map_or_else(|| zone_id.to_string(), |zone| zone.name.clone())
}

/// What a placed zone looks like in the editor: a card back for card zones (the scenario back
/// for scenario zones), the counter or token bag for widget zones.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_zone_stand_in(
    frame: &mut Frame,
    folder_path: &str,
    zone: &ZoneDefinition,
    is_scenario_zone: bool,
    center: (f32, f32),
    preview_scale: f32,
    alpha: f32,
    display_backs: &[gametable_core::models::CardBackDefinition],
    scenario_card_back_id: Option<&str>,
) {
    if zone.kind == ZoneKind::Card {
        let scenario_stand_in = CardDefinition { is_scenario_card: true, ..CardDefinition::new("", "") };
        let definition = is_scenario_zone.then_some(&scenario_stand_in);
        let card = CardDraw::new(definition, false, center.0, center.1, display_backs, scenario_card_back_id)
            .scale(preview_scale)
            .angle(orientation_turns(zone.orientation) * 2.0 * PI)
            .alpha(alpha);
        frame.draw_card(&card);
        return;
    }
    let size = COUNTER_SIZE * preview_scale;
    let (left, top) = (center.0 - size / 2.0, center.1 - size / 2.0);
    let radius = 8.0 * preview_scale;
    let image_file = match zone.widget_kind {
        ZoneWidgetKind::Counter => zone.counter_image_path.as_deref(),
        ZoneWidgetKind::TokenBag => zone.bag_image_path.as_deref(),
    };
    let texture = image_file.and_then(|file| frame.images.get(&join_path(&[folder_path, "_tokens", file])));
    let background = argb_with_alpha(zone.counter_starting_color, alpha);
    match zone.widget_kind {
        ZoneWidgetKind::Counter if texture.is_none() => frame.fill_rect(left, top, size, size, radius, background),
        _ => frame.fill_rect(left, top, size, size, radius, Color::rgba(0.0, 0.0, 0.0, 0.38 * alpha)),
    }
    if let Some(texture) = texture {
        let fit = (size / texture.width).min(size / texture.height);
        frame.draw_image(
            texture,
            center.0 - texture.width * fit / 2.0,
            center.1 - texture.height * fit / 2.0,
            fit,
            alpha,
        );
    }
    frame.outline_rect(
        left + 0.5,
        top + 0.5,
        size - 1.0,
        size - 1.0,
        radius,
        Color::rgba(1.0, 1.0, 1.0, 0.24 * alpha),
        1.0,
    );
    match zone.widget_kind {
        ZoneWidgetKind::Counter => {
            if texture.is_some() {
                frame.circle(DrawMode::Fill, center.0, center.1, 18.0 * preview_scale, background);
            }
            let text = lua_number_text(zone.counter_starting_value);
            let font_size = (22.0 * preview_scale).max(8.0);
            let options = TextOptions::new()
                .size(font_size)
                .medium()
                .width(size)
                .centered()
                .middle_of(size)
                .color(argb_with_alpha(zone.counter_starting_text_color, alpha));
            frame.text(&text, left, top, options);
        }
        ZoneWidgetKind::TokenBag => {
            if texture.is_none() {
                let icon_size = (size * 0.6).max(10.0);
                frame.icon_centered("widgets", left, top, size, size, icon_size, with_alpha(Color::WHITE, 0.7 * alpha));
            }
        }
    }
}

fn argb_with_alpha(argb: u32, alpha: f32) -> Color {
    let color = crate::ui::components::argb_color(argb);
    Color { alpha: color.alpha * alpha, ..color }
}

/// The zone name as a small label centred just above the zone (editor only).
pub(super) fn draw_zone_label(frame: &mut Frame, label: &str, center_x: f32, zone_top: f32, alpha: f32) {
    let font_size = 11.0;
    let max_width = 160.0;
    let text_width = frame.text_width(label, font_size, crate::ui::theme::Weight::Medium).min(max_width);
    let (padding_x, height) = (6.0, 18.0);
    let (label_x, label_y) = (center_x - text_width / 2.0 - padding_x, zone_top - height - 4.0);
    frame.fill_rect(
        label_x,
        label_y,
        text_width + 2.0 * padding_x,
        height,
        4.0,
        Color::rgba(0.0, 0.0, 0.0, 0.7 * alpha),
    );
    let options = TextOptions::new()
        .size(font_size)
        .medium()
        .width(text_width)
        .ellipsis()
        .middle_of(height)
        .color(Color::rgba(1.0, 1.0, 1.0, alpha));
    frame.text(label, label_x + padding_x, label_y, options);
}

#[cfg(test)]
mod tests {
    use super::*;
    use gametable_core::models::Scenario;

    fn game_with_zones() -> GameDefinition {
        let mut game = GameDefinition::new("game", "Game");
        game.zones.push(ZoneDefinition::new("deck", "Deck"));
        game.zones.push(ZoneDefinition { shared: true, ..ZoneDefinition::new("market", "Market") });
        let mut scenario = Scenario::new("s1", "Siege");
        scenario.zones.push(ZoneDefinition { shared: true, ..ZoneDefinition::new("walls", "Walls") });
        game.scenarios.push(scenario);
        game
    }

    #[test]
    fn only_widgets_offer_set_scale_and_labels_show_a_changed_scale() {
        let widget_items = placed_zone_menu_items("clues", true);
        assert_eq!(widget_items[0].action, Some(ZoneMenuAction::SetScale("clues".to_string())));
        assert_eq!(widget_items.len(), 2);
        let card_items = placed_zone_menu_items("deck", false);
        assert_eq!(card_items.len(), 1);
        assert_eq!(card_items[0].action, Some(ZoneMenuAction::TakeOff("deck".to_string())));
        assert_eq!(placed_zone_label("Clues", 1.0), "Clues");
        assert_eq!(placed_zone_label("Clues", 1.5), "Clues · 150%");
    }

    #[test]
    fn player_boards_offer_only_player_zones() {
        let game = game_with_zones();
        let ids: Vec<String> =
            placeable_zones(&game, BoardKind::Player).into_iter().map(|candidate| candidate.zone.id).collect();
        assert_eq!(ids, ["deck"]);
    }

    #[test]
    fn shared_boards_offer_shared_and_scenario_zones() {
        let game = game_with_zones();
        let zones = placeable_zones(&game, BoardKind::Shared);
        let ids: Vec<&str> = zones.iter().map(|candidate| candidate.zone.id.as_str()).collect();
        assert_eq!(ids, ["market", "walls"]);
        assert_eq!(zones[1].chip_label, "Scenario: Siege — Walls");
        assert!(zones[1].is_scenario_zone);
    }

    #[test]
    fn landscape_card_zones_are_wider_than_tall() {
        let zone = ZoneDefinition {
            orientation: gametable_core::models::CardOrientation::Left,
            ..ZoneDefinition::new("deck", "Deck")
        };
        assert_eq!(zone_world_size(&zone), (CARD_HEIGHT, CARD_WIDTH));
    }
}
