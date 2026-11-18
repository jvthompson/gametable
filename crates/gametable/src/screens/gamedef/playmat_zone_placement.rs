//! Zones placed on a playmat, edited in the Playmats tab (a GameTable addition). A row of chips
//! lists the zones that can still go on the selected mat; clicking one drops it in the mat's
//! centre. Placed zones are drawn on the table preview (a card back for card zones, the widget
//! for counters and token bags) with their name as a small label above, and can be dragged around
//! the mat, dragged off it to remove them, or removed with their × button.

use std::f32::consts::PI;

use gametable_core::models::{
    CardDefinition, GameDefinition, PlaymatType, PlaymatZonePlacement, ZoneDefinition, ZoneKind, ZoneWidgetKind,
    zone_fits_playmat_type,
};

use crate::app::Frame;
use crate::data::file_system::join_path;
use crate::graphics::draw_list::Color;
use crate::graphics::tessellation::DrawMode;
use crate::screens::gamedef::EditorModel;
use crate::screens::gamedef::common::display_card_backs;
use crate::table::card_render::{CARD_HEIGHT, CARD_WIDTH, CardDraw, orientation_turns};
use crate::table::panels::lua_number_text;
use crate::table::view::COUNTER_SIZE;
use crate::ui::components::{ChipOptions, Flow};
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::theme::with_alpha;
use crate::ui::widgets::IconButtonOptions;

/// The playmat's rectangle on screen and the preview's world-to-screen scale.
#[derive(Debug, Clone, Copy)]
pub struct MatScreenRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub preview_scale: f32,
}

impl MatScreenRect {
    fn point_at(&self, fraction_x: f64, fraction_y: f64) -> (f32, f32) {
        (self.x + fraction_x as f32 * self.width, self.y + fraction_y as f32 * self.height)
    }

    fn fraction_at(&self, screen_x: f32, screen_y: f32) -> (f64, f64) {
        (f64::from((screen_x - self.x) / self.width), f64::from((screen_y - self.y) / self.height))
    }
}

/// A zone that can be placed on the selected kind of mat.
#[derive(Debug, Clone)]
pub struct PlaceableZone {
    pub zone: ZoneDefinition,
    /// Shown on the chip: the zone name, or "Scenario: <scenario> — <zone>".
    pub chip_label: String,
    pub is_scenario_zone: bool,
}

/// Every zone that fits this playmat type: the game's own zones, and for the shared mat also
/// every scenario's zones (scenario zones are always shared).
pub fn placeable_zones(game: &GameDefinition) -> Vec<PlaceableZone> {
    let mut zones: Vec<PlaceableZone> = game
        .zones
        .iter()
        .filter(|zone| zone_fits_playmat_type(zone, game.playmat_type))
        .map(|zone| PlaceableZone { zone: zone.clone(), chip_label: zone.name.clone(), is_scenario_zone: false })
        .collect();
    if game.playmat_type == PlaymatType::Shared {
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
fn zone_world_size(zone: &ZoneDefinition) -> (f32, f32) {
    if zone.kind == ZoneKind::Widget {
        return (COUNTER_SIZE, COUNTER_SIZE);
    }
    if orientation_turns(zone.orientation) == 0.0 { (CARD_WIDTH, CARD_HEIGHT) } else { (CARD_HEIGHT, CARD_WIDTH) }
}

/// A zone being dragged around the mat: which placement, and where on the zone it was grabbed
/// (as an offset from its centre, in screen pixels).
struct DraggedZone {
    zone_id: String,
    grab_offset: (f32, f32),
}

#[derive(Default)]
pub struct ZonePlacementEditor {
    dragged: Option<DraggedZone>,
}

impl ZonePlacementEditor {
    /// The "Zones on This Mat" chips for placing zones. Returns the height used.
    pub fn draw_zone_chips(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        playmat_index: usize,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let palette = frame.palette();
        let label_height = frame.group_label("Zones on This Mat", x, y, width);
        let chips_y = y + label_height + 4.0;
        let placeable = placeable_zones(&model.game);
        let playmat = &model.game.playmats[playmat_index];
        let unplaced: Vec<&PlaceableZone> =
            placeable.iter().filter(|candidate| playmat.placement_for(&candidate.zone.id).is_none()).collect();
        let hint = if placeable.is_empty() {
            Some(match model.game.playmat_type {
                PlaymatType::Individual => "Add player zones (not shared) on the Zones tab to place them here.",
                PlaymatType::Shared => "Add shared zones or scenario zones to place them on the shared mat.",
            })
        } else if unplaced.is_empty() {
            Some("Every zone is placed. Drag a zone off the mat to take it off again.")
        } else {
            None
        };
        if let Some(hint) = hint {
            let options = TextOptions::new().size(13.0).width(width).color(palette.on_surface_variant);
            frame.text(hint, x, chips_y + 6.0, options);
            return label_height + 4.0 + 32.0;
        }
        let mut flow = Flow::new(x, chips_y, width, 32.0, 8.0);
        let mut picked_zone_id = None;
        for candidate in unplaced {
            let chip_width = frame.chip_width(&candidate.chip_label, false, true);
            let (chip_x, chip_y) = flow.place(chip_width);
            let id = format!("pt:place:{}", candidate.zone.id);
            let options = ChipOptions { id: Some(&id), icon: Some("add"), tooltip: Some("Place on the mat") };
            let (clicked, _) = frame.chip(&candidate.chip_label, chip_x, chip_y, false, &options);
            if clicked {
                picked_zone_id = Some(candidate.zone.id.clone());
            }
        }
        if let Some(zone_id) = picked_zone_id {
            model.game.playmats[playmat_index].zone_placements.push(PlaymatZonePlacement::new(zone_id, 0.5, 0.5));
        }
        flow.bottom() - y
    }

    /// Draws the placed zones on the preview mat and handles dragging and removing them.
    pub fn draw_placed_zones(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        playmat_index: usize,
        mat: MatScreenRect,
    ) {
        let placeable = placeable_zones(&model.game);
        let display_backs = display_card_backs(&model.folder_path, &model.game.card_backs);
        let scenario_card_back_id = model.game.scenario_card_back_id.clone();
        let mut removed_zone_id = None;
        let placements = model.game.playmats[playmat_index].zone_placements.clone();
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
            let (zone_width, zone_height) = (world_width * mat.preview_scale, world_height * mat.preview_scale);
            let is_dragged = self.dragged.as_ref().is_some_and(|dragged| dragged.zone_id == placement.zone_id);
            let mut center = mat.point_at(placement.x, placement.y);
            if is_dragged && let Some(dragged) = &self.dragged {
                let (mouse_x, mouse_y) = frame.ui.mouse();
                center = (mouse_x - dragged.grab_offset.0, mouse_y - dragged.grab_offset.1);
            }
            let region = frame.ui.region(
                &format!("pt:zone:{}", placement.zone_id),
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
            let off_the_mat =
                !(mat.x..=mat.x + mat.width).contains(&center.0) || !(mat.y..=mat.y + mat.height).contains(&center.1);
            if is_dragged && !region.held {
                self.dragged = None;
                if off_the_mat {
                    removed_zone_id = Some(placement.zone_id.clone());
                    continue;
                }
                let (fraction_x, fraction_y) = mat.fraction_at(center.0, center.1);
                if let Some(stored) = model.game.playmats[playmat_index]
                    .zone_placements
                    .iter_mut()
                    .find(|stored| stored.zone_id == placement.zone_id)
                {
                    stored.x = fraction_x.clamp(0.0, 1.0);
                    stored.y = fraction_y.clamp(0.0, 1.0);
                }
            }
            let alpha = match (fitting.is_some(), is_dragged && off_the_mat) {
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
                        mat.preview_scale,
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
                (Some(zone), true) => zone.name.clone(),
                (Some(zone), false) => format!("{} (doesn't fit this mat type)", zone.name),
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
                let id = format!("pt:unplace:{}", placement.zone_id);
                let options = IconButtonOptions {
                    background: Some(Color::rgba(0.0, 0.0, 0.0, 0.7)),
                    ..IconButtonOptions::tooltip("Take off the mat").id(&id).size(button_size, 16.0).color(Color::WHITE)
                };
                if frame.icon_button("close", button_x, button_y, &options) {
                    removed_zone_id = Some(placement.zone_id.clone());
                }
            }
        }
        if let Some(zone_id) = removed_zone_id {
            model.game.playmats[playmat_index].zone_placements.retain(|placement| placement.zone_id != zone_id);
        }
    }
}

/// What a placed zone looks like in the editor: a card back for card zones (the scenario back
/// for scenario zones), the counter or token bag for widget zones.
#[allow(clippy::too_many_arguments)]
fn draw_zone_stand_in(
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
fn draw_zone_label(frame: &mut Frame, label: &str, center_x: f32, zone_top: f32, alpha: f32) {
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
    fn individual_mats_offer_only_player_zones() {
        let mut game = game_with_zones();
        game.playmat_type = PlaymatType::Individual;
        let ids: Vec<String> = placeable_zones(&game).into_iter().map(|candidate| candidate.zone.id).collect();
        assert_eq!(ids, ["deck"]);
    }

    #[test]
    fn the_shared_mat_offers_shared_and_scenario_zones() {
        let mut game = game_with_zones();
        game.playmat_type = PlaymatType::Shared;
        let zones = placeable_zones(&game);
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
