//! Player-to-player trading: drag cards from the other player's collection into your "Wants"
//! box, confirm when you're happy with it, and the server swaps the cards once both sides have
//! confirmed. Port of `src/ui/screens/server/trade.lua`.

use std::collections::HashMap;
use std::f32::consts::PI;

use gametable_core::json::JsonObject;
use gametable_core::models::{CardDefinition, GameDefinition};
use gametable_core::util::trim_lua_whitespace;

use crate::app::{Frame, Screen, ScreenCommand};
use crate::graphics::draw_list::Color;
use crate::graphics::tessellation::DrawMode;
use crate::net::server_client::{SharedServer, response_ok};
use crate::net::server_models::{ServerUser, TradeOffer, local_time_label, quantities};
use crate::screens::server::collection::draw_hovered_card_preview;
use crate::table::card_pool_grid::{NAME_LINES, TILE_CARD_HEIGHT, TILE_CARD_WIDTH, TILE_SCALE, any_card_turned};
use crate::table::card_render::{CardDraw, orientation_turns};
use crate::ui::card_filter_bar::CardFilterState;
use crate::ui::components::{Flow, TAB_BAR_HEIGHT};
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::overlays::ConfirmResponse;
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::{Weight, fixed, with_alpha};
use crate::ui::widgets::{ButtonOptions, ButtonVariant, IconButtonOptions, ScrollbarStyle};

/// Smaller thumbnails inside the want-boxes.
const WANT_SCALE: f32 = 0.6;
const DRAG_THRESHOLD: f32 = 4.0;

/// A press on a partner's card that becomes a drag once the mouse moves far enough.
struct Press {
    card_id: String,
    press_x: f32,
    press_y: f32,
    center_x: f32,
    center_y: f32,
}

/// A card being dragged towards the want-box; `grab` is the card centre minus the mouse.
struct Drag {
    card_id: String,
    grab_x: f32,
    grab_y: f32,
}

#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl Rect {
    fn contains(&self, (x, y): (f32, f32)) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

pub struct TradeScreen {
    server: SharedServer,
    game: GameDefinition,
    title: String,
    tab: usize,
    chat_text: String,
    seen_chat: usize,
    my_filters: CardFilterState,
    their_filters: CardFilterState,
    my_owned: Option<HashMap<String, i64>>,
    their_owned: Option<HashMap<String, i64>>,
    my_owned_request: Option<u64>,
    their_owned_request: Option<u64>,
    pressing: Option<Press>,
    dragging: Option<Drag>,
    my_want_box: Option<Rect>,
    hovered_id: Option<String>,
    confirm_leave: bool,
}

impl TradeScreen {
    pub fn new(server: SharedServer, game: GameDefinition) -> TradeScreen {
        let mut screen = TradeScreen {
            my_filters: CardFilterState::new(&game),
            their_filters: CardFilterState::new(&game),
            server,
            game,
            title: "Trade".to_string(),
            tab: 0,
            chat_text: String::new(),
            seen_chat: 0,
            my_owned: None,
            their_owned: None,
            my_owned_request: None,
            their_owned_request: None,
            pressing: None,
            dragging: None,
            my_want_box: None,
            hovered_id: None,
            confirm_leave: false,
        };
        screen.refresh_mine();
        screen.refresh_theirs();
        screen
    }

    fn trade_id(&self) -> Option<String> {
        self.server.borrow().trade.as_ref().map(|trade| trade.id.clone())
    }

    fn refresh_mine(&mut self) {
        let payload = JsonObject::new().with("gameId", self.game.id.as_str());
        self.my_owned_request = Some(self.server.borrow_mut().request("collectionGet", payload));
    }

    fn refresh_theirs(&mut self) {
        let Some(trade_id) = self.trade_id() else { return };
        let payload = JsonObject::new().with("tradeId", trade_id);
        self.their_owned_request = Some(self.server.borrow_mut().request("tradeCollectionGet", payload));
    }

    fn finish_requests(&mut self) {
        for (request, owned) in
            [(&mut self.my_owned_request, &mut self.my_owned), (&mut self.their_owned_request, &mut self.their_owned)]
        {
            let Some(rid) = *request else { continue };
            let Some(response) = self.server.borrow_mut().take_response(rid) else { continue };
            *request = None;
            if response_ok(&response) {
                *owned = Some(quantities(response.get("cards")));
            }
        }
    }

    fn offer(&self, card_id: &str, delta: f64) {
        let Some(trade_id) = self.trade_id() else { return };
        let payload = JsonObject::new().with("tradeId", trade_id).with("cardId", card_id).with("delta", delta);
        self.server.borrow_mut().request_ignored("tradeOffer", payload);
    }

    /// Drag to offer: press a card in the partner's collection, drag it onto your own want-box,
    /// release to add it.
    fn update_drag(&mut self, frame: &mut Frame) {
        if let Some(drag) = &self.dragging {
            if frame.ui.mouse_released(1) {
                if self.my_want_box.is_some_and(|rect| rect.contains(frame.ui.mouse())) {
                    self.offer(&drag.card_id, 1.0);
                }
                self.dragging = None;
            }
            return;
        }
        let Some(press) = &self.pressing else { return };
        if !frame.ui.mouse_down(1) {
            self.pressing = None;
            return;
        }
        let (mouse_x, mouse_y) = frame.ui.mouse();
        if (mouse_x - press.press_x).abs() > DRAG_THRESHOLD || (mouse_y - press.press_y).abs() > DRAG_THRESHOLD {
            self.dragging = Some(Drag {
                card_id: press.card_id.clone(),
                grab_x: press.center_x - press.press_x,
                grab_y: press.center_y - press.press_y,
            });
            self.pressing = None;
        }
    }

    fn draw_drag_feedback(&self, frame: &mut Frame) {
        let Some(drag) = &self.dragging else { return };
        let Some(definition) = self.game.card_by_id(&drag.card_id) else { return };
        frame.ui.set_cursor("hand");
        let mouse = frame.ui.mouse();
        if let Some(rect) = self.my_want_box
            && rect.contains(mouse)
        {
            let color = with_alpha(frame.palette().primary, 0.12);
            frame.fill_rect(rect.x, rect.y, rect.width, rect.height, 12.0, color);
        }
        let draw = CardDraw::new(
            Some(definition),
            true,
            mouse.0 + drag.grab_x,
            mouse.1 + drag.grab_y,
            &self.game.card_backs,
            None,
        )
        .scale(TILE_SCALE)
        .angle(orientation_turns(definition.orientation) * 2.0 * PI);
        frame.draw_card(&draw);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_want_box(
        &mut self,
        frame: &mut Frame,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        who: &ServerUser,
        offer: &TradeOffer,
        mine: bool,
    ) {
        let palette = frame.palette();
        frame.card(x, y, width, height, Some(12.0), None, false);
        let heading = format!("{} Wants", who.name);
        frame.text(&heading, x + 16.0, y + 12.0, TextOptions::new().bold().width(width - 32.0).ellipsis());
        if mine {
            self.my_want_box = Some(Rect { x, y, width, height });
        }
        let mut list: Vec<(&String, &i64)> = offer.cards.iter().collect();
        list.sort();
        if list.is_empty() {
            let hint = if mine { "Drag cards here from their collection." } else { "Nothing yet." };
            frame.text(
                hint,
                x + 16.0,
                y + 44.0,
                TextOptions::new().width(width - 32.0).color(palette.on_surface_variant),
            );
            return;
        }
        let (thumb_width, thumb_height) = (TILE_CARD_WIDTH * WANT_SCALE, TILE_CARD_HEIGHT * WANT_SCALE);
        let mut flow = Flow::new(x + 16.0, y + 44.0, width - 32.0, thumb_height + 6.0, 10.0);
        frame.ui.push_clip(x, y + 40.0, width, height - 44.0);
        let mut removed = None;
        for (card_id, quantity) in list {
            let Some(definition) = self.game.card_by_id(card_id) else { continue };
            let (place_x, place_y) = flow.place(thumb_width);
            let (center_x, center_y) = (place_x + thumb_width / 2.0, place_y + thumb_height / 2.0);
            let clicked = mine
                && frame
                    .ui
                    .region(
                        &format!("tr:want:{card_id}"),
                        place_x,
                        place_y,
                        thumb_width,
                        thumb_height,
                        RegionOptions::hand().with_tooltip(Some("Remove")),
                    )
                    .clicked;
            let draw = CardDraw::new(Some(definition), true, center_x, center_y, &self.game.card_backs, None)
                .scale(TILE_SCALE * WANT_SCALE)
                .angle(orientation_turns(definition.orientation) * 2.0 * PI);
            frame.draw_card(&draw);
            if *quantity > 1 {
                frame.draw_count_badge(
                    center_x + thumb_width / 2.0 - 10.0,
                    center_y - thumb_height / 2.0 + 10.0,
                    *quantity as usize,
                    0.8,
                );
            }
            if clicked {
                removed = Some(card_id.clone());
            }
        }
        frame.ui.pop_clip();
        if let Some(card_id) = removed {
            self.offer(&card_id, -1.0);
        }
    }

    /// One of the two collections: the Collection screen's grid, but only ever cards that are
    /// actually owned, and the partner's grid is draggable.
    #[allow(clippy::too_many_arguments)]
    fn draw_grid(&mut self, frame: &mut Frame, theirs: bool, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let scope = if theirs { "theirs" } else { "mine" };
        let prefix = format!("tr:{scope}");
        let game = &self.game;
        let empty = HashMap::new();
        let (filters, owned) = if theirs {
            (&mut self.their_filters, self.their_owned.as_ref().unwrap_or(&empty))
        } else {
            (&mut self.my_filters, self.my_owned.as_ref().unwrap_or(&empty))
        };
        let (filters_bottom, _) = frame.card_filters(&prefix, filters, game, x, y, width, true, &[]);
        let row_y =
            frame.card_search(&prefix, filters, game, x, filters_bottom + 8.0, width, Some(palette.surface), None)
                + 4.0;
        let visible: Vec<&CardDefinition> = filters
            .visible(game)
            .into_iter()
            .filter(|card| !card.unownable && (card.is_free_card || owned.get(&card.id).copied().unwrap_or(0) > 0))
            .collect();
        if visible.is_empty() {
            let options = TextOptions::new().width(width).centered().color(palette.on_surface_variant);
            frame.text("No cards to show here.", x, row_y + 24.0, options);
        }
        let mut hovered = None;
        let mut pressed = None;
        let grid_id = format!("tr:grid:{scope}");
        let has_turned_cards = any_card_turned(visible.iter().copied());
        frame.card_pool_grid(
            &grid_id,
            visible.len(),
            x,
            row_y,
            width,
            y + height - row_y,
            0.0,
            1.0,
            has_turned_cards,
            |frame, tile| {
                let definition = visible[tile.index];
                let name_height = frame.card_pool_name_height();
                let count = owned.get(&definition.id).copied().unwrap_or(0);
                let region_options = if theirs { RegionOptions::hand() } else { RegionOptions::default() };
                let region = frame.ui.region(
                    &format!("tr:card:{scope}:{}", definition.id),
                    tile.center_x - TILE_CARD_WIDTH / 2.0,
                    tile.y,
                    TILE_CARD_WIDTH,
                    TILE_CARD_HEIGHT + 6.0 + name_height,
                    region_options,
                );
                let center_y = tile.y + TILE_CARD_HEIGHT / 2.0;
                let draw = CardDraw::new(Some(definition), true, tile.center_x, center_y, &game.card_backs, None)
                    .scale(TILE_SCALE)
                    .angle(orientation_turns(definition.orientation) * 2.0 * PI);
                frame.draw_card(&draw);
                let badge_x = tile.center_x + TILE_CARD_WIDTH / 2.0 + 6.0 - 11.0;
                frame.circle(DrawMode::Fill, badge_x, tile.y - 6.0 + 11.0, 11.0, Color::rgba(0.0, 0.0, 0.0, 0.87));
                let label = if definition.is_free_card { "∞".to_string() } else { format!("×{count}") };
                let options =
                    TextOptions::new().size(11.0).bold().width(22.0).middle_of(22.0).centered().color(fixed::WHITE);
                frame.text(&label, tile.center_x + TILE_CARD_WIDTH / 2.0 - 16.0, tile.y - 6.0, options);
                let options = TextOptions::new().size(11.0).width(tile.width).centered().max_lines(NAME_LINES as usize);
                frame.text(&definition.card_title, tile.x, tile.y + TILE_CARD_HEIGHT + 6.0, options);
                if region.hovered {
                    hovered = Some(definition.id.clone());
                }
                if theirs && region.pressed {
                    pressed = Some((definition.id.clone(), tile.center_x, center_y));
                }
            },
        );
        self.hovered_id = hovered;
        if let Some((card_id, center_x, center_y)) = pressed
            && self.pressing.is_none()
            && self.dragging.is_none()
        {
            let (press_x, press_y) = frame.ui.mouse();
            self.pressing = Some(Press { card_id, press_x, press_y, center_x, center_y });
        }
    }

    fn draw_confirm(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, my_offer: &TradeOffer) {
        let (label, variant, icon) = if my_offer.confirmed {
            ("Confirmed -- waiting...", ButtonVariant::Tonal, "hourglass_empty")
        } else {
            ("Confirm Trade", ButtonVariant::Filled, "check")
        };
        let options = ButtonOptions::variant(variant).id("tr:confirm").icon(icon);
        if frame.button(label, x, y, Some(280.0f32.min(width)), Some(48.0), &options)
            && let Some(trade_id) = self.trade_id()
        {
            let payload = JsonObject::new().with("tradeId", trade_id).with("confirmed", !my_offer.confirmed);
            self.server.borrow_mut().request_ignored("tradeConfirm", payload);
        }
    }

    fn draw_chat(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let lines = self.server.borrow().trade_chat.clone();
        frame.fill_rect(x, y, width, height, 12.0, palette.surface_container);
        frame.text("Chat", x + 16.0, y + 12.0, TextOptions::new().bold());
        let input_height = 48.0;
        let (list_y, list_height) = (y + 40.0, height - 40.0 - input_height - 24.0);
        let offset = frame.begin_scroll("tr:chat", x + 8.0, list_y, width - 16.0, list_height);
        let mut line_y = list_y - offset;
        let text_width = width - 32.0;
        for line in &lines {
            let head = format!("{}  {}", local_time_label(line.at), line.name);
            frame.text(&head, x + 16.0, line_y, TextOptions::new().size(12.0).bold().color(palette.on_surface_variant));
            line_y += 18.0;
            frame.text(&line.text, x + 16.0, line_y, TextOptions::new().width(text_width));
            line_y += frame.text_height(&line.text, text_width, 14.0, Weight::Regular) + 10.0;
        }
        frame.end_scroll("tr:chat", line_y + offset - list_y, &ScrollbarStyle::default());
        if lines.len() != self.seen_chat {
            self.seen_chat = lines.len();
            frame.ui.scroll_to_end("tr:chat");
        }
        if lines.is_empty() {
            let options =
                TextOptions::new().width(width).middle_of(list_height).centered().color(palette.on_surface_variant);
            frame.text("Say hello!", x, list_y, options);
        }
        let options = TextFieldOptions {
            label_background: Some(palette.surface_container),
            max_length: Some(500),
            ..TextFieldOptions::label("Message")
        };
        let field = frame.text_field(
            "tr:chatinput",
            x + 12.0,
            y + height - input_height - 12.0,
            width - 24.0 - 44.0,
            &mut self.chat_text,
            &options,
        );
        let blank = trim_lua_whitespace(&self.chat_text).is_empty();
        let options = IconButtonOptions::tooltip("Send").id("tr:send").disabled(blank);
        let send = frame.icon_button("send", x + width - 12.0 - 40.0, y + height - input_height - 12.0 + 4.0, &options);
        if (field.submitted || send)
            && !blank
            && let Some(trade_id) = self.trade_id()
        {
            let text = std::mem::take(&mut self.chat_text);
            self.server
                .borrow_mut()
                .send("chatSend", JsonObject::new().with("text", text).with("tradeId", trade_id).into());
            frame.ui.focus_field("tr:chatinput", false);
        }
    }
}

impl Screen for TradeScreen {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn enter(&mut self, _frame: &mut Frame) {
        self.server.borrow_mut().trade_screen_open = true;
    }

    fn leave(&mut self, _frame: &mut Frame) {
        self.server.borrow_mut().trade_screen_open = false;
    }

    fn on_back(&mut self, _frame: &mut Frame) {
        self.confirm_leave = true;
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        self.finish_requests();
        let trade = self.server.borrow().trade.clone();
        let Some(trade) = trade else {
            frame.spinner(x + width / 2.0, y + height / 2.0, 18.0, None);
            return;
        };
        if self.their_owned.is_none() && self.their_owned_request.is_none() {
            self.refresh_theirs();
        }
        self.update_drag(frame);
        let my_id = self.server.borrow().my_id().to_string();
        let (me, them) = trade.parties(&my_id);
        let (me, them) = (me.clone(), them.clone());
        self.title = format!("Trade with {}", them.name);
        let (my_offer, their_offer) = (trade.offer_of(&me.id), trade.offer_of(&them.id));
        let padding = 24.0;
        let chat_width = 380.0f32.min((width * 0.36).floor());
        let body_width = width - 3.0 * padding - chat_width;
        let body_x = x + padding;
        let (box_gap, box_height) = (16.0, 150.0);
        let box_width = (body_width - box_gap) / 2.0;
        self.draw_want_box(frame, body_x, y + padding, box_width, box_height, &me, &my_offer, true);
        self.draw_want_box(
            frame,
            body_x + box_width + box_gap,
            y + padding,
            box_width,
            box_height,
            &them,
            &their_offer,
            false,
        );
        let mut row_y = y + padding + box_height + 16.0;
        frame.divider(body_x, row_y, body_width, None);
        row_y += 16.0;
        let their_label = format!("{}'s Collection", them.name);
        self.tab = frame.tab_bar("tr:tab", body_x, row_y, body_width, &["Your Collection", &their_label], self.tab);
        row_y += TAB_BAR_HEIGHT + 12.0;
        let confirm_height = 56.0;
        let grid_height = y + height - padding - row_y - confirm_height;
        self.draw_grid(frame, self.tab == 1, body_x, row_y, body_width, grid_height);
        self.draw_confirm(frame, body_x, row_y + grid_height + 8.0, body_width, &my_offer);
        self.draw_chat(frame, x + width - padding - chat_width, y + padding, chat_width, height - 2.0 * padding);
        frame.overlay(|frame| self.draw_drag_feedback(frame));
        let hovered = self.hovered_id.as_deref().and_then(|id| self.game.card_by_id(id));
        frame.overlay(|frame| draw_hovered_card_preview(frame, &self.game, hovered));
        if self.confirm_leave {
            match frame.confirm_dialog("tr:leave", "Leave Trade", "Cancel this trade?", "Leave", false) {
                ConfirmResponse::Open => {}
                ConfirmResponse::Cancelled => self.confirm_leave = false,
                ConfirmResponse::Confirmed => {
                    self.confirm_leave = false;
                    if let Some(trade_id) = self.trade_id() {
                        self.server
                            .borrow_mut()
                            .request_ignored("tradeLeave", JsonObject::new().with("tradeId", trade_id));
                    }
                    frame.commands.push(ScreenCommand::Pop);
                }
            }
        }
    }
}
