//! Collection mode: browse the cards you own for a game. Packs and starter decks are bought from
//! the Card Shop ([`super::shop`]). Port of `src/ui/screens/server/collection.lua`.

use std::collections::HashMap;
use std::f32::consts::PI;

use gametable_core::json::JsonObject;
use gametable_core::models::{CardDefinition, GameDefinition};

use crate::app::{Frame, Screen};
use crate::graphics::draw_list::Color;
use crate::graphics::tessellation::DrawMode;
use crate::net::server_client::{SharedServer, response_message, response_ok};
use crate::net::server_models::{ServerGameInfo, quantities};
use crate::screens::server::GameLoad;
use crate::table::card_pool_grid::{NAME_LINES, TILE_CARD_HEIGHT, TILE_CARD_WIDTH, TILE_SCALE};
use crate::table::card_render::{CardDraw, definition_faces, orientation_turns};
use crate::ui::card_filter_bar::CardFilterState;
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::theme::fixed;
use crate::ui::widgets::ToggleOptions;

/// The loaded collection.
struct OpenCollection {
    game: GameDefinition,
    owned: HashMap<String, i64>,
    filters: CardFilterState,
    owned_only: bool,
}

pub struct CollectionScreen {
    server: SharedServer,
    title: String,
    load: Option<GameLoad>,
    /// The game waiting for its `collectionGet` answer.
    pending: Option<(u64, GameDefinition)>,
    collection: Option<OpenCollection>,
    hovered_id: Option<String>,
}

/// Space shows the hovered card big, away from the mouse (`C:drawPreview`).
pub fn draw_hovered_card_preview(frame: &mut Frame, game: &GameDefinition, hovered: Option<&CardDefinition>) {
    let Some(definition) = hovered else { return };
    if frame.ui.any_overlay_open() || frame.ui.focus.is_some() || !frame.keyboard.is_down(&["space"]) {
        return;
    }
    let (screen_width, screen_height) = frame.ui.screen_size();
    let (mouse_x, _) = frame.ui.mouse();
    let faces = definition_faces(definition, &game.card_backs, None);
    frame.draw_card_preview(&faces, &game.card_backs, screen_width, screen_height, mouse_x, None);
}

impl CollectionScreen {
    pub fn new(frame: &mut Frame, server: SharedServer, info: &ServerGameInfo) -> CollectionScreen {
        let load = GameLoad::start(frame, &server, &info.id);
        CollectionScreen {
            server,
            title: "Collection".to_string(),
            load: Some(load),
            pending: None,
            collection: None,
            hovered_id: None,
        }
    }

    fn advance_loading(&mut self, frame: &mut Frame) {
        if let Some(load) = &mut self.load
            && let Some(result) = load.show(frame, &self.server)
        {
            self.load = None;
            if let Some(game) = result {
                let rid = self
                    .server
                    .borrow_mut()
                    .request("collectionGet", JsonObject::new().with("gameId", game.id.as_str()));
                self.pending = Some((rid, game));
            }
        }
        let Some((rid, _)) = &self.pending else { return };
        let Some(response) = self.server.borrow_mut().take_response(*rid) else { return };
        let Some((_, game)) = self.pending.take() else { return };
        if !response_ok(&response) {
            frame.ui.alert("Couldn't Load Collection", &response_message(&response, "Something went wrong."));
            return;
        }
        self.title = format!("Collection -- {}", game.name);
        let filters = CardFilterState::new(&game);
        self.collection =
            Some(OpenCollection { owned: quantities(response.get("cards")), game, filters, owned_only: true });
    }

    fn draw_collection(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let Some(collection) = &mut self.collection else { return };
        let game = &collection.game;
        let owned = &collection.owned;
        // Free cards are always owned, in unlimited copies (not counted in total).
        let (mut distinct, mut total, mut collectible) = (0i64, 0i64, 0i64);
        for card in game.cards() {
            if card.unownable {
                continue;
            }
            collectible += 1;
            let count = owned.get(&card.id).copied().unwrap_or(0);
            if card.is_free_card {
                distinct += 1;
            } else if count > 0 {
                distinct += 1;
                total += count;
            }
        }
        frame.text("Your Cards", x, y, TextOptions::new().size(18.0));
        let summary = format!("{distinct} of {collectible} cards  ·  {total} copies");
        frame.text(&summary, x, y + 26.0, TextOptions::new().size(13.0).color(palette.on_surface_variant));
        let (filters_bottom, _) =
            frame.card_filters("co", &mut collection.filters, game, x, y + 52.0, width - 150.0, true, &[]);
        collection.owned_only = frame.checkbox(
            "co:owned",
            x + width - 130.0,
            y + 52.0 + 7.0,
            collection.owned_only,
            &ToggleOptions::label("Owned only"),
        );
        let row_y = frame.card_search(
            "co",
            &mut collection.filters,
            game,
            x,
            filters_bottom + 8.0,
            width,
            Some(palette.surface),
            None,
        ) + 4.0;
        let owned_only = collection.owned_only;
        let visible: Vec<&CardDefinition> = collection
            .filters
            .visible(game)
            .into_iter()
            .filter(|card| {
                !card.unownable && (!owned_only || card.is_free_card || owned.get(&card.id).copied().unwrap_or(0) > 0)
            })
            .collect();
        if visible.is_empty() {
            let message = if owned_only && distinct == 0 {
                "You don't own any cards for this game yet. Visit the Card Shop to open a pack."
            } else {
                "No cards match these filters."
            };
            frame.text(
                message,
                x,
                row_y + 24.0,
                TextOptions::new().width(width).centered().color(palette.on_surface_variant),
            );
        }
        // Same tile layout as the Deck Editor's card pool.
        let mut hovered = None;
        frame.card_pool_grid("co:grid", visible.len(), x, row_y, width, y + height - row_y, 0.0, |frame, tile| {
            let definition = visible[tile.index];
            let name_height = frame.card_pool_name_height();
            let count = owned.get(&definition.id).copied().unwrap_or(0);
            let have = definition.is_free_card || count > 0;
            let region = frame.ui.region(
                &format!("co:card:{}", definition.id),
                tile.center_x - TILE_CARD_WIDTH / 2.0,
                tile.y,
                TILE_CARD_WIDTH,
                TILE_CARD_HEIGHT + 6.0 + name_height,
                RegionOptions::default(),
            );
            let draw = CardDraw::new(
                Some(definition),
                true,
                tile.center_x,
                tile.y + TILE_CARD_HEIGHT / 2.0,
                &game.card_backs,
                None,
            )
            .scale(TILE_SCALE)
            .alpha(if have { 1.0 } else { 0.25 })
            .angle(orientation_turns(definition.orientation) * 2.0 * PI);
            frame.draw_card(&draw);
            if have {
                let badge_x = tile.center_x + TILE_CARD_WIDTH / 2.0 + 6.0 - 11.0;
                frame.circle(DrawMode::Fill, badge_x, tile.y - 6.0 + 11.0, 11.0, Color::rgba(0.0, 0.0, 0.0, 0.87));
                let label = if definition.is_free_card { "∞".to_string() } else { format!("×{count}") };
                let options =
                    TextOptions::new().size(11.0).bold().width(22.0).middle_of(22.0).centered().color(fixed::WHITE);
                frame.text(&label, tile.center_x + TILE_CARD_WIDTH / 2.0 - 16.0, tile.y - 6.0, options);
            }
            let name_color = if have { palette.on_surface } else { palette.on_surface_variant };
            let options = TextOptions::new()
                .size(11.0)
                .width(tile.width)
                .centered()
                .max_lines(NAME_LINES as usize)
                .color(name_color);
            frame.text(&definition.card_title, tile.x, tile.y + TILE_CARD_HEIGHT + 6.0, options);
            if region.hovered {
                hovered = Some(definition.id.clone());
            }
        });
        self.hovered_id = hovered;
    }
}

impl Screen for CollectionScreen {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        self.advance_loading(frame);
        if self.collection.is_none() {
            frame.spinner(x + width / 2.0, y + height / 2.0, 18.0, None);
            return;
        }
        let padding = 24.0;
        self.draw_collection(frame, x + padding, y + padding, width - 2.0 * padding, height - 2.0 * padding);
        if let Some(collection) = &self.collection {
            let hovered = self.hovered_id.as_deref().and_then(|id| collection.game.card_by_id(id));
            frame.overlay(|frame| draw_hovered_card_preview(frame, &collection.game, hovered));
        }
    }
}
