//! Collection mode: the Card Shop, where players spend tokens on packs for a game's sets and on
//! starter decks. Packs use the same algorithm as the Pack Generator widget (run on the server).
//! Port of `src/ui/screens/server/shop.lua`.

use std::collections::HashMap;
use std::f32::consts::PI;

use gametable_core::json::{JsonObject, JsonValue};
use gametable_core::models::{CardDefinition, CardOrientation, GameDefinition};

use crate::app::{Frame, Screen};
use crate::data::deck_library::built_in_decks_folder;
use crate::data::image_resolver::resolve_image_any_extension;
use crate::net::server_client::{SharedServer, response_message, response_ok};
use crate::net::server_models::{ServerGameInfo, quantities};
use crate::screens::game_picker::{GameTile, grid_height};
use crate::screens::server::{GameLoad, tokens_label};
use crate::sound::SoundEffect;
use crate::table::card_pool_grid::{NAME_LINES, TILE_CARD_HEIGHT, TILE_CARD_WIDTH, TILE_SCALE};
use crate::table::card_render::{CARD_HEIGHT, CARD_WIDTH, CardDraw, definition_faces, orientation_turns};
use crate::ui::components::TAB_BAR_HEIGHT;
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::overlays::{DialogAction, DialogResponse, DialogSpec};
use crate::ui::theme::{Weight, fixed};

const TABS: [&str; 2] = ["Packs", "Decks"];
const REVEAL_WIDTH: f32 = 480.0;
const LOW_TOKEN_HINT: &str =
    "You need more tokens. Admins can grant tokens, and some servers give out a regular allowance.";

/// One card from an opened pack, and whether the collection didn't have it before.
#[derive(Clone)]
struct OpenedCard {
    card_id: String,
    is_new: bool,
}

/// The opened pack being shown: one card at a time, then all together.
enum PackDialog {
    Reveal { cards: Vec<OpenedCard>, index: usize, title: String },
    Summary { cards: Vec<OpenedCard>, title: String },
}

/// A purchase waiting for its answer.
enum Purchase {
    Pack { rid: u64, set_name: String },
    Starter { rid: u64, name: String },
}

struct OpenShop {
    game: GameDefinition,
    info: ServerGameInfo,
    owned: HashMap<String, i64>,
}

pub struct ShopScreen {
    server: SharedServer,
    info: ServerGameInfo,
    title: String,
    tab: usize,
    load: Option<GameLoad>,
    pending: Option<(u64, GameDefinition)>,
    shop: Option<OpenShop>,
    buying: Option<Purchase>,
    pack_dialog: Option<PackDialog>,
}

fn new_badge(frame: &mut Frame, x: f32, y: f32) {
    frame.fill_rect(x, y, 36.0, 18.0, 9.0, fixed::GREEN);
    let options = TextOptions::new().size(10.0).bold().width(36.0).middle_of(18.0).centered().color(fixed::WHITE);
    frame.text("NEW", x, y, options);
}

impl ShopScreen {
    pub fn new(frame: &mut Frame, server: SharedServer, info: ServerGameInfo) -> ShopScreen {
        let load = GameLoad::start(frame, &server, &info.id);
        ShopScreen {
            server,
            info,
            title: "Card Shop".to_string(),
            tab: 0,
            load: Some(load),
            pending: None,
            shop: None,
            buying: None,
            pack_dialog: None,
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
            frame.ui.alert("Couldn't Load Card Shop", &response_message(&response, "Something went wrong."));
            return;
        }
        self.title = format!("Card Shop -- {}", game.name);
        self.shop = Some(OpenShop { game, info: self.info.clone(), owned: quantities(response.get("cards")) });
    }

    fn buy_pack(&mut self, set_id: &str, set_name: &str) {
        let Some(shop) = &self.shop else { return };
        let payload = JsonObject::new().with("gameId", shop.game.id.as_str()).with("setId", set_id);
        let rid = self.server.borrow_mut().request("packBuy", payload);
        self.buying = Some(Purchase::Pack { rid, set_name: set_name.to_string() });
    }

    fn buy_starter(&mut self, name: &str) {
        let Some(shop) = &self.shop else { return };
        let payload = JsonObject::new().with("gameId", shop.game.id.as_str()).with("name", name);
        let rid = self.server.borrow_mut().request("starterBuy", payload);
        self.buying = Some(Purchase::Starter { rid, name: name.to_string() });
    }

    fn finish_purchase(&mut self, frame: &mut Frame) {
        let rid = match &self.buying {
            Some(Purchase::Pack { rid, .. } | Purchase::Starter { rid, .. }) => *rid,
            None => return,
        };
        let Some(response) = self.server.borrow_mut().take_response(rid) else { return };
        let Some(purchase) = self.buying.take() else { return };
        let Some(shop) = &mut self.shop else { return };
        if !response_ok(&response) {
            frame.sounds.play(SoundEffect::Error);
            let title = match purchase {
                Purchase::Pack { .. } => "Couldn't Open Pack",
                Purchase::Starter { .. } => "Couldn't Buy Starter Deck",
            };
            frame.ui.alert(title, &response_message(&response, "Something went wrong."));
            return;
        }
        let before = shop.owned.clone();
        let (ids, title) = match &purchase {
            Purchase::Pack { set_name, .. } => {
                let ids: Vec<String> = response
                    .get("cards")
                    .and_then(JsonValue::as_array)
                    .map(|cards| cards.iter().filter_map(|id| id.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                (ids, format!("{set_name} Pack"))
            }
            Purchase::Starter { name, .. } => {
                let mut ids = Vec::new();
                for (id, count) in quantities(response.get("cards")) {
                    for _ in 0..count.max(0) {
                        ids.push(id.clone());
                    }
                }
                ids.sort();
                (ids, format!("{name} Starter Deck"))
            }
        };
        for id in &ids {
            *shop.owned.entry(id.clone()).or_insert(0) += 1;
        }
        if let Some(tokens) = response.get("tokens").and_then(JsonValue::as_f64)
            && let Some(me) = &mut self.server.borrow_mut().me
        {
            me.tokens = tokens as i64;
        }
        if let Purchase::Starter { name, .. } = &purchase {
            let saved = response.get("savedName").and_then(JsonValue::as_str).unwrap_or(name);
            frame.ui.toast(&format!("Added \"{saved}\" to your decks."), None);
        }
        // `S:showPack`: cards new to the collection are marked.
        let cards: Vec<OpenedCard> = ids
            .iter()
            .filter(|id| shop.game.card_by_id(id).is_some())
            .map(|id| OpenedCard { card_id: id.clone(), is_new: before.get(id).copied().unwrap_or(0) == 0 })
            .collect();
        frame.sounds.play(SoundEffect::Shuffle);
        if !cards.is_empty() {
            self.pack_dialog = Some(PackDialog::Reveal { cards, index: 0, title });
        }
    }

    /// `S:revealPack` and `S:showPackSummary`.
    fn show_pack_dialog(&mut self, frame: &mut Frame) {
        let Some(shop) = &self.shop else { return };
        let game = &shop.game;
        let Some(dialog) = &mut self.pack_dialog else { return };
        match dialog {
            PackDialog::Reveal { cards, index, title } => {
                let last = *index + 1 >= cards.len();
                let actions =
                    [DialogAction::text("Skip"), DialogAction::filled(if last { "Show All" } else { "Next Card" })];
                let spec = DialogSpec {
                    title: Some(title),
                    width: REVEAL_WIDTH,
                    dismissible: false,
                    handles_enter: true,
                    actions: &actions,
                    ..DialogSpec::default()
                };
                let card = cards[*index].clone();
                let count = cards.len();
                let shown = *index + 1;
                let response = frame.dialog("shop:reveal", &spec, |frame, x, y, width| {
                    let Some(definition) = game.card_by_id(&card.card_id) else { return 0.0 };
                    let color = frame.palette().on_surface_variant;
                    let counter = format!("Card {shown} of {count}");
                    frame.text(&counter, x, y, TextOptions::new().width(width).centered().size(13.0).color(color));
                    // Fit the card (portrait or turned) in a box under the counter.
                    let (_, screen_height) = frame.ui.screen_size();
                    let (box_width, box_height) = (width, 240.0f32.max(460.0f32.min(screen_height - 380.0)));
                    let turned = definition.orientation != CardOrientation::Portrait;
                    let (card_width, card_height) =
                        if turned { (CARD_HEIGHT, CARD_WIDTH) } else { (CARD_WIDTH, CARD_HEIGHT) };
                    let scale = (box_width / card_width).min(box_height / card_height);
                    let center_y = y + 28.0 + card_height * scale / 2.0;
                    let draw = CardDraw::new(Some(definition), true, x + width / 2.0, center_y, &game.card_backs, None)
                        .scale(scale)
                        .angle(orientation_turns(definition.orientation) * 2.0 * PI);
                    frame.draw_card(&draw);
                    if card.is_new {
                        new_badge(frame, x + (width - card_width * scale) / 2.0 + 8.0, y + 28.0 + 8.0);
                    }
                    let title_y = y + 28.0 + card_height * scale + 12.0;
                    let options = TextOptions::new().width(width).centered().bold().max_lines(2);
                    frame.text(&definition.card_title, x, title_y, options);
                    title_y - y + frame.text_height(&definition.card_title, width, 14.0, Weight::Bold) + 4.0
                });
                match response {
                    DialogResponse::Action(0) => {
                        let (cards, title) = (std::mem::take(cards), std::mem::take(title));
                        self.pack_dialog = Some(PackDialog::Summary { cards, title });
                    }
                    DialogResponse::Action(_) | DialogResponse::Enter => {
                        if last {
                            let (cards, title) = (std::mem::take(cards), std::mem::take(title));
                            self.pack_dialog = Some(PackDialog::Summary { cards, title });
                        } else {
                            *index += 1;
                            frame.sounds.play(SoundEffect::Draw);
                        }
                    }
                    DialogResponse::Open | DialogResponse::Dismissed => {}
                }
            }
            PackDialog::Summary { cards, title } => {
                let gap = 12.0;
                let name_height = frame.card_pool_name_height();
                let (tile_width, row_height) = (TILE_CARD_WIDTH + 24.0, TILE_CARD_HEIGHT + 6.0 + name_height + 4.0);
                let per_row = cards.len().min(6);
                let width = 420.0f32.max(per_row as f32 * (tile_width + gap) - gap + 48.0);
                let actions = [DialogAction::filled("Done")];
                let spec = DialogSpec { title: Some(title), width, actions: &actions, ..DialogSpec::default() };
                let response = frame.dialog("shop:summary", &spec, |frame, x, y, width| {
                    let mut hovered: Option<&CardDefinition> = None;
                    let rows = cards.len().div_ceil(per_row);
                    for (index, card) in cards.iter().enumerate() {
                        let Some(definition) = game.card_by_id(&card.card_id) else { continue };
                        let (column, row) = (index % per_row, index / per_row);
                        let row_count = per_row.min(cards.len() - row * per_row);
                        let row_width = row_count as f32 * (tile_width + gap) - gap;
                        let tile_x = x + (width - row_width) / 2.0 + column as f32 * (tile_width + gap);
                        let (center_x, tile_y) = (tile_x + tile_width / 2.0, y + 8.0 + row as f32 * (row_height + gap));
                        let region = frame.ui.region(
                            &format!("pk:{}", index + 1),
                            center_x - TILE_CARD_WIDTH / 2.0,
                            tile_y,
                            TILE_CARD_WIDTH,
                            TILE_CARD_HEIGHT + 6.0 + name_height,
                            RegionOptions::default(),
                        );
                        let draw = CardDraw::new(
                            Some(definition),
                            true,
                            center_x,
                            tile_y + TILE_CARD_HEIGHT / 2.0,
                            &game.card_backs,
                            None,
                        )
                        .scale(TILE_SCALE)
                        .angle(orientation_turns(definition.orientation) * 2.0 * PI);
                        frame.draw_card(&draw);
                        if card.is_new {
                            new_badge(frame, center_x - TILE_CARD_WIDTH / 2.0 + 4.0, tile_y + 4.0);
                        }
                        let options =
                            TextOptions::new().size(11.0).width(tile_width).centered().max_lines(NAME_LINES as usize);
                        frame.text(&definition.card_title, tile_x, tile_y + TILE_CARD_HEIGHT + 6.0, options);
                        if region.hovered {
                            hovered = Some(definition);
                        }
                    }
                    if let Some(definition) = hovered
                        && frame.keyboard.is_down(&["space"])
                    {
                        frame.overlay(|frame| {
                            let (screen_width, screen_height) = frame.ui.screen_size();
                            let (mouse_x, _) = frame.ui.mouse();
                            let faces = definition_faces(definition, &game.card_backs, None);
                            frame.draw_card_preview(
                                &faces,
                                &game.card_backs,
                                screen_width,
                                screen_height,
                                mouse_x,
                                None,
                            );
                        });
                    }
                    8.0 + rows as f32 * (row_height + gap) - gap
                });
                if response != DialogResponse::Open {
                    self.pack_dialog = None;
                }
            }
        }
    }

    /// The Packs and Decks tabs: sets or starter decks as large image tiles, the same treatment
    /// the game-selection grid uses. Returns the clicked tile.
    fn draw_tiles(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32, packs: bool) -> Option<usize> {
        let palette = frame.palette();
        let shop = self.shop.as_ref()?;
        let game = &shop.game;
        let tokens = self.server.borrow().me.as_ref().map_or(0, |me| me.tokens);
        let info = self.server.borrow().info.clone();
        let cost = if packs { info.pack_cost.unwrap_or(1) } else { info.starter_deck_cost.unwrap_or(3) };
        let (heading, per, empty) = if packs {
            ("Open Packs", "per pack", "This game has no sets, so it has no packs.")
        } else {
            ("Starter Decks", "per starter deck", "This game has no starter decks.")
        };
        frame.text(heading, x, y, TextOptions::new().size(18.0));
        let summary = format!("{}  ·  {} {per}", tokens_label(tokens), tokens_label(cost));
        frame.text(&summary, x, y + 26.0, TextOptions::new().size(13.0).color(palette.on_surface_variant));
        let mut row_y = y + 56.0;
        // Falls back to the game's default card back when no matching art file is found.
        let default_back = game.card_backs.first().and_then(|back| back.image_path.clone());
        let folder = game.folder_path.as_deref();
        let entries: Vec<(String, String, i64, Option<String>)> = if packs {
            shop.info
                .sets
                .iter()
                .map(|set| {
                    let image = resolve_image_any_extension(folder, &set.id).or_else(|| default_back.clone());
                    (format!("pack:{}", set.id), set.name.clone(), set.card_count, image)
                })
                .collect()
        } else {
            let decks_folder = folder.map(built_in_decks_folder);
            shop.info
                .starter_decks
                .iter()
                .map(|starter| {
                    let image = resolve_image_any_extension(decks_folder.as_deref(), &starter.name)
                        .or_else(|| default_back.clone());
                    (format!("starter:{}", starter.name), starter.name.clone(), starter.card_count, image)
                })
                .collect()
        };
        if entries.is_empty() {
            frame.text(empty, x, row_y, TextOptions::new().color(palette.on_surface_variant));
            return None;
        }
        let disabled = self.buying.is_some() || tokens < cost;
        let tiles: Vec<GameTile> = entries
            .iter()
            .map(|(id, name, card_count, image)| GameTile {
                id,
                name,
                folder_path: folder,
                image_path: image.as_deref(),
                caption: Some(format!("{card_count} cards  ·  {}", tokens_label(cost))),
                disabled,
                ..GameTile::default()
            })
            .collect();
        let grid = grid_height(width, tiles.len()).min(1.0f32.max(y + height - row_y));
        let scroll_id = if packs { "shop:packs" } else { "shop:starters" };
        let clicked = frame.game_grid(scroll_id, &tiles, x, row_y, width, grid, 24.0);
        row_y += grid + 24.0;
        if tokens < cost {
            let options = TextOptions::new().width(width).size(12.0).color(palette.on_surface_variant);
            frame.text(LOW_TOKEN_HINT, x, row_y, options);
        }
        clicked
    }
}

impl Screen for ShopScreen {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        self.advance_loading(frame);
        self.finish_purchase(frame);
        if self.shop.is_none() {
            frame.spinner(x + width / 2.0, y + height / 2.0, 18.0, None);
            return;
        }
        let padding = 24.0;
        self.tab = frame.tab_bar("shop:tabs", x, y, width, &TABS, self.tab);
        let body_y = y + TAB_BAR_HEIGHT + padding;
        let (body_x, body_width, body_height) = (x + padding, width - 2.0 * padding, y + height - body_y - padding);
        let packs = self.tab == 0;
        if let Some(index) = self.draw_tiles(frame, body_x, body_y, body_width, body_height, packs) {
            if packs {
                let set = self.info.sets[index].clone();
                self.buy_pack(&set.id, &set.name);
            } else {
                let name = self.info.starter_decks[index].name.clone();
                self.buy_starter(&name);
            }
        }
        self.show_pack_dialog(frame);
    }
}
