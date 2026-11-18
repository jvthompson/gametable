//! The authoritative host loop. Port of `src/game/host_engine.lua`.
//!
//! Applies client requests (always as the sending connection's player, after ownership checks),
//! sends each other player a filtered `fullState` once per revision, and relays cosmetic drag and
//! arrow previews. The same engine runs in a peer-hosted game and inside the lobby server.
//!
//! Requests are untyped JSON from the network. A request the original would have refused, or that
//! would have raised a Lua error, changes nothing and gets no reply. Where a malformed request
//! (a missing or non-numeric coordinate, a fractional count) made the original do something
//! half-defined, this port refuses it instead; real clients never send such requests.

use std::collections::{BTreeSet, HashSet};

use crate::deck_widget_zones::parse_deck_widget_zone_id;
use crate::json::{JsonObject, JsonValue};
use crate::models::{
    BoardWidgetKind, DeckConfig, GameDefinition, PlayerInfo, PlayerRole, PlaymatType, PlaymatZonePosition, TableState,
    WidgetSizeOverride,
};
use crate::session::{ArrowDragPreview, CardDragPreview, Session, SessionError};
use crate::stack_utils::stack_of;
use crate::state_filter::filter_state_for_recipient;

/// The connection side of a host: a peer-hosting TCP server, or a lobby-server room.
pub trait HostTransport {
    /// Everyone currently connected, host included (role `host`).
    fn roster(&self) -> Vec<PlayerInfo>;
    /// Sends one message to one player.
    fn send_to(&mut self, player_id: &str, message_type: &str, payload: JsonValue);
    /// Sends one message to every connected player except `except_player_id`.
    fn broadcast(&mut self, message_type: &str, payload: JsonValue, except_player_id: Option<&str>);
}

pub struct HostEngine {
    pub session: Session,
    /// Who never receives state (the in-process host, or a synthetic server id).
    host_player_id: String,
    /// Who may do host-only things (load a scenario, set the shared playmat).
    owner_id: String,
    last_broadcast_revision: Option<i64>,
    last_seen_notification_count: u64,
    previously_connected_ids: HashSet<String>,
}

/// Reads typed fields from a request payload, treating wrong types as absent (Lua's `num`/`str`).
struct Payload<'a>(&'a JsonValue);

impl<'a> Payload<'a> {
    fn text(&self, key: &str) -> Option<&'a str> {
        self.0.get(key).and_then(JsonValue::as_str)
    }
    fn number(&self, key: &str) -> Option<f64> {
        self.0.get(key).and_then(JsonValue::as_f64)
    }
    /// A whole number, or `None` when absent, non-numeric or fractional.
    fn whole_number(&self, key: &str) -> Option<i64> {
        self.number(key).filter(|number| number.fract() == 0.0 && number.is_finite()).map(|number| number as i64)
    }
    /// A whole number with a default for an absent (or non-numeric) field; `None` for a fraction.
    fn whole_number_or(&self, key: &str, default_value: i64) -> Option<i64> {
        match self.number(key) {
            None => Some(default_value),
            Some(_) => self.whole_number(key),
        }
    }
    fn is_true(&self, key: &str) -> bool {
        self.0.get(key).is_some_and(JsonValue::is_exactly_true)
    }
    fn color(&self, key: &str) -> Option<u32> {
        self.whole_number(key).and_then(|number| u32::try_from(number).ok())
    }
    fn text_list(&self, key: &str) -> Vec<&'a str> {
        self.0
            .get(key)
            .and_then(JsonValue::as_array)
            .map_or_else(Vec::new, |items| items.iter().filter_map(JsonValue::as_str).collect())
    }
    /// Raw list items (for checks where non-text ids are simply refused).
    fn list(&self, key: &str) -> &'a [JsonValue] {
        self.0.get(key).and_then(JsonValue::as_array).map_or(&[], Vec::as_slice)
    }
}

impl HostEngine {
    /// `owner_id` defaults to `host_player_id` (peer hosting, where they are the same person).
    pub fn new(session: Session, host_player_id: impl Into<String>, owner_id: Option<String>) -> Self {
        let host_player_id = host_player_id.into();
        Self {
            session,
            owner_id: owner_id.unwrap_or_else(|| host_player_id.clone()),
            host_player_id,
            last_broadcast_revision: None,
            last_seen_notification_count: 0,
            previously_connected_ids: HashSet::new(),
        }
    }

    /// Starts serving: clients already connected count as connected, then everyone gets the state.
    pub fn start(&mut self, transport: &mut dyn HostTransport) {
        for player in transport.roster() {
            if player.role != PlayerRole::Host {
                self.previously_connected_ids.insert(player.id);
            }
        }
        self.last_seen_notification_count = self.session.notification_count();
        self.broadcast_state(transport);
    }

    pub fn owner_id(&self) -> &str {
        &self.owner_id
    }

    /// Whether a reconnecting client may reclaim this seat (a known, currently disconnected player).
    pub fn is_reclaimable_player_id(&self, player_id: &str) -> bool {
        self.session.state().players.iter().any(|player| player.id == player_id && !player.connected)
    }

    /// Call when the transport's roster changes: new players get the game definition, departed
    /// players' previews are cleared, and connection flags are updated.
    pub fn on_roster(&mut self, roster: &[PlayerInfo], transport: &mut dyn HostTransport) {
        let connected: HashSet<String> =
            roster.iter().filter(|player| player.role != PlayerRole::Host).map(|player| player.id.clone()).collect();
        // Sorted so the order of these messages is deterministic (Lua's hash order was not).
        let newly_connected: BTreeSet<&String> =
            connected.iter().filter(|id| !self.previously_connected_ids.contains(*id)).collect();
        for player_id in newly_connected {
            transport.send_to(player_id, "gameData", self.session.game.to_json());
        }
        let departed: BTreeSet<String> = self.previously_connected_ids.difference(&connected).cloned().collect();
        for player_id in departed {
            self.clear_card_drag_preview(&player_id, transport);
            self.clear_arrow_drag_preview(&player_id, transport);
        }
        self.session.sync_connected_player_ids(&connected);
        self.previously_connected_ids = connected;
        self.broadcast_if_notified(transport);
    }

    /// Runs session timers (arrow expiry) and sends any resulting state.
    pub fn tick(&mut self, transport: &mut dyn HostTransport) {
        self.session.update();
        self.broadcast_if_notified(transport);
    }

    /// Sends state if the session notified since the last check (the original's listener).
    pub fn broadcast_if_notified(&mut self, transport: &mut dyn HostTransport) {
        let notification_count = self.session.notification_count();
        if notification_count != self.last_seen_notification_count {
            self.last_seen_notification_count = notification_count;
            self.broadcast_state(transport);
        }
    }

    /// Sends every non-host player their filtered view, once per revision.
    pub fn broadcast_state(&mut self, transport: &mut dyn HostTransport) {
        let state = self.session.state();
        if self.last_broadcast_revision == Some(state.revision) {
            return;
        }
        self.last_broadcast_revision = Some(state.revision);
        let visible_zone_ids: HashSet<String> =
            self.session.effective_zones().filter(|zone| zone.visible_to_all).map(|zone| zone.id.clone()).collect();
        for player in &state.players {
            if player.id != self.host_player_id {
                let filtered = filter_state_for_recipient(state, &player.id, &visible_zone_ids);
                transport.send_to(&player.id, "fullState", filtered.to_json());
            }
        }
    }

    // -----------------------------------------------------------------------
    // Permission checks
    // -----------------------------------------------------------------------

    fn state(&self) -> &TableState {
        self.session.state()
    }

    /// Whose zone a request addresses: a deck widget's owner, nobody for a shared zone, otherwise
    /// the requester's own copy.
    fn zone_owner_id(&self, zone_id: &str, requester: &str) -> Option<String> {
        if let Some(parsed) = parse_deck_widget_zone_id(zone_id) {
            return self.state().widget_by_id(parsed.widget_instance_id).and_then(|widget| widget.owner_id.clone());
        }
        let zone = self.session.effective_zones().find(|zone| zone.id == zone_id);
        if zone.is_some_and(|zone| zone.shared) { None } else { Some(requester.to_string()) }
    }

    fn allowed_deck_widget_zone(&self, zone_id: Option<&str>, requester: &str) -> bool {
        let Some(parsed) = zone_id.and_then(parse_deck_widget_zone_id) else { return true };
        self.state()
            .widget_by_id(parsed.widget_instance_id)
            .is_some_and(|widget| widget.owner_id.as_deref() == Some(requester))
    }

    fn allowed_card(&self, instance_id: Option<&str>, requester: &str) -> bool {
        instance_id
            .and_then(|id| self.state().card_by_id(id))
            .is_some_and(|card| card.owner_id.as_deref().is_none_or(|owner| owner == requester))
    }

    fn movable_widget(&self, instance_id: Option<&str>, requester: &str) -> bool {
        let Some(widget) = instance_id.and_then(|id| self.state().widget_by_id(id)) else { return true };
        if widget.zone_id.is_some() {
            return false;
        }
        widget.owner_id.as_deref().is_none_or(|owner| owner == requester)
    }

    fn allowed_widget(&self, instance_id: Option<&str>, requester: &str) -> bool {
        instance_id
            .and_then(|id| self.state().widget_by_id(id))
            .is_some_and(|widget| widget.owner_id.as_deref().is_none_or(|owner| owner == requester))
    }

    /// Another player's card token may not be attached to.
    fn allowed_attach_target(&self, card_id: Option<&str>, requester: &str) -> bool {
        match card_id.and_then(|id| self.state().widget_by_id(id)) {
            Some(widget) if widget.kind == BoardWidgetKind::CardToken => {
                widget.owner_id.as_deref().is_none_or(|owner| owner == requester)
            }
            _ => true,
        }
    }

    fn allowed_delete(&self, instance_id: Option<&str>, requester: &str) -> bool {
        let Some(widget) = instance_id.and_then(|id| self.state().widget_by_id(id)) else { return false };
        if !self.movable_widget(instance_id, requester) {
            return false;
        }
        widget.creator_id.as_deref().is_none_or(|creator| creator == requester)
    }

    fn allowed_draw_or_shuffle(&self, root_id: Option<&str>, requester: &str) -> bool {
        let Some(root_id) = root_id else { return true };
        stack_of(self.state().cards(), root_id)
            .iter()
            .all(|card| card.owner_id.as_deref().is_none_or(|owner| owner == requester))
    }

    // -----------------------------------------------------------------------
    // Requests
    // -----------------------------------------------------------------------

    /// Handles one message from a client, then sends any resulting state.
    pub fn handle_message(
        &mut self,
        client_id: &str,
        message_type: &str,
        payload: &JsonValue,
        transport: &mut dyn HostTransport,
    ) {
        if let Err(error) = self.apply_request(client_id, message_type, &Payload(payload), transport) {
            eprintln!("host: ignored bad {message_type} from {client_id}: {error}");
        }
        self.broadcast_if_notified(transport);
    }

    fn apply_request(
        &mut self,
        client_id: &str,
        message_type: &str,
        p: &Payload,
        transport: &mut dyn HostTransport,
    ) -> Result<(), SessionError> {
        match message_type {
            "requestMove" => {
                if self.allowed_card(p.text("instanceId"), client_id)
                    && let (Some(id), Some(x), Some(y)) = (p.text("instanceId"), p.number("x"), p.number("y"))
                {
                    self.session.move_card(id, x, y, client_id)?;
                }
            }
            "requestMoveGroup" => {
                if self.allowed_card(p.text("primaryInstanceId"), client_id)
                    && let (Some(primary), Some(x), Some(y)) =
                        (p.text("primaryInstanceId"), p.number("x"), p.number("y"))
                {
                    let passengers: Vec<String> = p
                        .list("passengerRootInstanceIds")
                        .iter()
                        .filter_map(JsonValue::as_str)
                        .filter(|id| self.allowed_card(Some(id), client_id))
                        .map(str::to_string)
                        .collect();
                    self.session.move_group(primary, &passengers, x, y, client_id)?;
                }
            }
            "requestMoveStack" => {
                if self.allowed_card(p.text("rootInstanceId"), client_id)
                    && let (Some(root), Some(x), Some(y)) = (p.text("rootInstanceId"), p.number("x"), p.number("y"))
                {
                    self.session.move_stack(root, x, y);
                }
            }
            "requestRotateStack" => {
                if self.allowed_card(p.text("rootInstanceId"), client_id)
                    && let Some(root) = p.text("rootInstanceId")
                {
                    let step = p.number("step").unwrap_or(1.0);
                    self.session.rotate_stack(root, p.is_true("clockwise"), client_id, step)?;
                }
            }
            "requestSendToBack" => {
                if self.allowed_card(p.text("rootInstanceId"), client_id)
                    && let Some(root) = p.text("rootInstanceId")
                {
                    self.session.send_to_back(root);
                }
            }
            "requestFlip" => {
                if self.allowed_card(p.text("instanceId"), client_id)
                    && let Some(id) = p.text("instanceId")
                {
                    self.session.flip_card(id, client_id)?;
                }
            }
            "requestGiveCard" => {
                let new_owner = p.text("newOwnerId");
                let valid_owner = new_owner.is_none_or(|owner| self.state().player_by_id(owner).is_some());
                if self.allowed_card(p.text("instanceId"), client_id)
                    && valid_owner
                    && let Some(id) = p.text("instanceId")
                {
                    self.session.give_card(id, new_owner, client_id)?;
                }
            }
            "requestStack" => {
                let onto = p.text("ontoInstanceId");
                if self.allowed_card(p.text("instanceId"), client_id)
                    && let (Some(id), Some(onto)) = (p.text("instanceId"), onto)
                    && self.state().card_by_id(onto).is_some()
                {
                    self.session.stack_card(id, onto, client_id)?;
                }
            }
            "requestMoveToHand" => {
                if self.allowed_card(p.text("instanceId"), client_id)
                    && let Some(id) = p.text("instanceId")
                {
                    self.session.move_to_hand(id, Some(client_id))?;
                }
            }
            "requestReorderHand" => {
                if self.allowed_card(p.text("instanceId"), client_id)
                    && let (Some(id), Some(target_index)) = (p.text("instanceId"), p.whole_number_or("targetIndex", 0))
                {
                    self.session.reorder_hand(id, target_index, Some(client_id));
                }
            }
            "requestDraw" => {
                if self.allowed_draw_or_shuffle(p.text("pileInstanceId"), client_id)
                    && let (Some(pile), Some(count)) = (p.text("pileInstanceId"), p.whole_number_or("count", 1))
                {
                    self.session.draw_card(pile, Some(client_id), count);
                }
            }
            "requestShuffle" => {
                if self.allowed_draw_or_shuffle(p.text("pileRootInstanceId"), client_id)
                    && let Some(root) = p.text("pileRootInstanceId")
                {
                    self.session.shuffle_pile(root, client_id);
                }
            }
            "requestDrawFromZone" => {
                if let (Some(zone_id), Some(count)) = (p.text("zoneId"), p.whole_number_or("count", 1)) {
                    let zone_owner = self.zone_owner_id(zone_id, client_id);
                    self.session.draw_from_zone(zone_id, zone_owner.as_deref(), Some(client_id), count)?;
                }
            }
            "requestReturnToZone" => {
                if self.allowed_card(p.text("instanceId"), client_id)
                    && self.allowed_deck_widget_zone(p.text("zoneId"), client_id)
                    && let (Some(id), Some(zone_id)) = (p.text("instanceId"), p.text("zoneId"))
                {
                    let zone_owner = self.zone_owner_id(zone_id, client_id);
                    self.session.return_to_zone(
                        id,
                        zone_id,
                        zone_owner.as_deref(),
                        p.is_true("toBottom"),
                        client_id,
                    )?;
                }
            }
            "requestShuffleZone" => {
                if let Some(zone_id) = p.text("zoneId") {
                    let zone_owner = self.zone_owner_id(zone_id, client_id);
                    self.session.shuffle_zone(zone_id, zone_owner.as_deref(), client_id)?;
                }
            }
            "requestStartSearchZone" => {
                if self.allowed_deck_widget_zone(p.text("zoneId"), client_id)
                    && let Some(zone_id) = p.text("zoneId")
                {
                    let zone_owner = self.zone_owner_id(zone_id, client_id);
                    self.session.start_search_zone(zone_id, zone_owner.as_deref(), Some(client_id))?;
                }
            }
            "requestStartSearchPile" => {
                if self.allowed_draw_or_shuffle(p.text("pileRootInstanceId"), client_id)
                    && let Some(root) = p.text("pileRootInstanceId")
                {
                    self.session.start_search_pile(root, Some(client_id));
                }
            }
            "requestStopSearch" => self.session.stop_search(Some(client_id)),
            "requestCreateWidget" => {
                let kind = p.text("kind").and_then(BoardWidgetKind::from_json_str);
                if let (Some(kind), Some(id), Some(x), Some(y)) =
                    (kind, p.text("instanceId"), p.number("x"), p.number("y"))
                {
                    self.session.create_widget(id, kind, x, y, client_id, p.text("imagePath"));
                }
            }
            "requestCreateArrow" => {
                if let (Some(id), Some(x), Some(y), Some(x2), Some(y2)) =
                    (p.text("instanceId"), p.number("x"), p.number("y"), p.number("x2"), p.number("y2"))
                {
                    self.session.create_arrow(id, x, y, x2, y2, client_id);
                }
            }
            "requestMoveWidget" => {
                if self.movable_widget(p.text("instanceId"), client_id)
                    && let (Some(id), Some(x), Some(y)) = (p.text("instanceId"), p.number("x"), p.number("y"))
                {
                    self.session.move_widget(id, x, y);
                }
            }
            "requestSetWidgetValue" => {
                if self.allowed_widget(p.text("instanceId"), client_id)
                    && let Some(id) = p.text("instanceId")
                {
                    let value = p.number("value").unwrap_or(0.0);
                    self.session.set_widget_value(id, value, client_id)?;
                }
            }
            "requestDeleteWidget" => {
                if self.allowed_delete(p.text("instanceId"), client_id)
                    && let Some(id) = p.text("instanceId")
                {
                    self.session.delete_widget(id);
                }
            }
            "requestSetWidgetColors" => {
                if self.allowed_widget(p.text("instanceId"), client_id)
                    && let (Some(id), Some(background), Some(text_color)) =
                        (p.text("instanceId"), p.color("backgroundColor"), p.color("textColor"))
                {
                    self.session.set_widget_colors(id, background, text_color);
                }
            }
            "requestSetWidgetText" => {
                if self.allowed_widget(p.text("instanceId"), client_id)
                    && let (Some(id), Some(text_color)) = (p.text("instanceId"), p.color("textColor"))
                {
                    let text = p.text("text").unwrap_or("");
                    self.session.set_widget_text(id, text, text_color);
                }
            }
            "requestSetWidgetSize" => {
                if self.allowed_widget(p.text("instanceId"), client_id)
                    && let Some(id) = p.text("instanceId")
                {
                    let size_override = if p.text("sizeOverride") == Some(WidgetSizeOverride::NATIVE_JSON_TEXT) {
                        Some(WidgetSizeOverride::NativeImageSize)
                    } else {
                        p.number("sizeOverride").map(WidgetSizeOverride::Multiplier)
                    };
                    self.session.set_widget_size(id, size_override);
                }
            }
            "requestRotateWidget" => {
                if self.allowed_widget(p.text("instanceId"), client_id)
                    && let Some(id) = p.text("instanceId")
                {
                    let step = p.number("step").unwrap_or(1.0);
                    self.session.rotate_widget(id, p.is_true("clockwise"), step);
                }
            }
            "requestFlipWidget" => {
                if self.allowed_widget(p.text("instanceId"), client_id)
                    && let Some(id) = p.text("instanceId")
                {
                    self.session.flip_widget(id);
                }
            }
            "requestSendWidgetToBack" => {
                if self.allowed_widget(p.text("instanceId"), client_id)
                    && let Some(id) = p.text("instanceId")
                {
                    self.session.send_widget_to_back(id);
                }
            }
            "requestDuplicateWidget" => {
                if self.movable_widget(p.text("sourceInstanceId"), client_id)
                    && let (Some(source), Some(new_id), Some(x), Some(y)) =
                        (p.text("sourceInstanceId"), p.text("newInstanceId"), p.number("x"), p.number("y"))
                {
                    self.session.duplicate_widget(source, new_id, x, y);
                }
            }
            "requestAttachWidgetToCard" => {
                if self.movable_widget(p.text("instanceId"), client_id)
                    && self.allowed_attach_target(p.text("cardId"), client_id)
                    && let (Some(id), Some(card_id), Some(x), Some(y)) =
                        (p.text("instanceId"), p.text("cardId"), p.number("x"), p.number("y"))
                {
                    self.session.attach_widget_to_card(id, card_id, x, y, client_id)?;
                }
            }
            "requestDrawBagToken" => {
                if self.allowed_widget(p.text("instanceId"), client_id)
                    && let Some(id) = p.text("instanceId")
                {
                    self.session.draw_bag_token(id, client_id)?;
                }
            }
            "requestReturnBagToken" => {
                if self.allowed_widget(p.text("instanceId"), client_id)
                    && let (Some(id), Some(slot_id)) = (p.text("instanceId"), p.text("slotId"))
                {
                    self.session.return_bag_token(id, slot_id, client_id)?;
                }
            }
            "requestPlaceBagTokenOnTable" => {
                if self.allowed_widget(p.text("instanceId"), client_id)
                    && let (Some(id), Some(slot_id), Some(new_id), Some(x), Some(y)) =
                        (p.text("instanceId"), p.text("slotId"), p.text("newInstanceId"), p.number("x"), p.number("y"))
                {
                    self.session.place_bag_token_on_table(id, slot_id, new_id, x, y, client_id);
                }
            }
            "requestResetBag" => {
                if self.allowed_widget(p.text("instanceId"), client_id)
                    && let Some(id) = p.text("instanceId")
                {
                    self.session.reset_bag(id, client_id)?;
                }
            }
            "requestSetBagRemaining" => {
                if self.allowed_widget(p.text("instanceId"), client_id)
                    && let Some(id) = p.text("instanceId")
                {
                    let bag_remaining: Vec<String> =
                        p.text_list("bagRemaining").into_iter().map(str::to_string).collect();
                    self.session.set_bag_remaining(id, &bag_remaining, client_id);
                }
            }
            "requestReturnImageTokenToBag" => {
                if self.movable_widget(p.text("tokenInstanceId"), client_id)
                    && self.allowed_widget(p.text("bagInstanceId"), client_id)
                    && let (Some(bag), Some(token)) = (p.text("bagInstanceId"), p.text("tokenInstanceId"))
                {
                    self.session.return_image_token_to_bag(bag, token, client_id)?;
                }
            }
            "requestGeneratePack" => {
                if let Some(set_id) = p.text("setId") {
                    self.session.generate_pack(set_id, client_id);
                }
            }
            "requestCreateCardFromLibrary" => {
                if let (Some(definition_id), Some(x), Some(y)) = (p.text("definitionId"), p.number("x"), p.number("y"))
                {
                    self.session.create_card_from_library(definition_id, x, y, client_id);
                }
            }
            "requestLoadDeckIntoZone" => {
                let deck = p.0.get("deck").map(DeckConfig::from_json);
                if let (Some(zone_id), Some(Ok(deck))) = (p.text("zoneId"), deck) {
                    self.session.load_deck_into_zone(zone_id, &deck, client_id)?;
                }
            }
            "requestLoadScenario" => {
                if client_id == self.owner_id
                    && let Some(scenario_id) = p.text("scenarioId")
                {
                    self.session.load_scenario(scenario_id, client_id);
                }
            }
            "requestSetSharedPlaymat" => {
                if client_id == self.owner_id
                    && let Some(index) = p.whole_number_or("index", 0)
                {
                    self.session.set_shared_playmat(p.is_true("on"), index);
                }
            }
            "requestSetCurrentPlayer" => self.session.set_current_player(p.text("playerId")),
            // A player moves zones on their own mat; only the owner moves zones on the shared mat.
            "requestSetPlaymatZonePositions" => {
                if let Some(playmat_index) = p.whole_number_or("playmatIndex", 0) {
                    let positions: Vec<PlaymatZonePosition> = p
                        .list("positions")
                        .iter()
                        .filter_map(|position| {
                            let mut position = PlaymatZonePosition::from_json(position).ok()?;
                            position.playmat_index = playmat_index;
                            Some(position)
                        })
                        .collect();
                    match self.session.game.playmat_type {
                        PlaymatType::Individual => {
                            self.session.set_playmat_zone_positions(Some(client_id), playmat_index, positions)
                        }
                        PlaymatType::Shared if client_id == self.owner_id => {
                            self.session.set_playmat_zone_positions(None, playmat_index, positions)
                        }
                        PlaymatType::Shared => {}
                    }
                }
            }
            "requestSetIndividualPlaymat" => {
                if let Some(index) = p.whole_number_or("index", 0) {
                    self.session.set_individual_playmat(client_id, p.is_true("on"), index);
                }
            }
            "cardDragPreview" => {
                let ids = p.list("instanceIds");
                let first_id = ids.first().and_then(JsonValue::as_str);
                if first_id.is_some() && self.allowed_card(first_id, client_id) {
                    let mut allowed_ids = vec![first_id.unwrap().to_string()];
                    allowed_ids.extend(
                        ids[1..]
                            .iter()
                            .filter_map(JsonValue::as_str)
                            .filter(|id| self.allowed_card(Some(id), client_id))
                            .map(str::to_string),
                    );
                    self.publish_card_drag_preview(client_id, allowed_ids, p.number("fx"), p.number("fy"), transport);
                }
            }
            "cardDragPreviewEnd" => self.clear_card_drag_preview(client_id, transport),
            "arrowDragPreview" => {
                let corners = (p.number("fx"), p.number("fy"), p.number("fx2"), p.number("fy2"));
                self.publish_arrow_drag_preview(client_id, corners, transport);
            }
            "arrowDragPreviewEnd" => self.clear_arrow_drag_preview(client_id, transport),
            _ => {}
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Drag previews
    // -----------------------------------------------------------------------

    /// Previews go to everyone except their sender (the in-process host sees its own drag anyway).
    fn except_for(&self, player_id: &str) -> Option<String> {
        (player_id != self.host_player_id).then(|| player_id.to_string())
    }

    pub fn publish_card_drag_preview(
        &mut self,
        player_id: &str,
        instance_ids: Vec<String>,
        fraction_x: Option<f64>,
        fraction_y: Option<f64>,
        transport: &mut dyn HostTransport,
    ) {
        let (Some(fraction_x), Some(fraction_y)) = (fraction_x, fraction_y) else { return };
        let payload = JsonObject::new()
            .with("playerId", player_id)
            .with("instanceIds", instance_ids.clone())
            .with("fx", fraction_x)
            .with("fy", fraction_y);
        self.session.card_drag_previews.insert(
            player_id.to_string(),
            CardDragPreview { player_id: player_id.to_string(), instance_ids, fraction_x, fraction_y },
        );
        transport.broadcast("cardDragPreview", payload.into(), self.except_for(player_id).as_deref());
    }

    pub fn clear_card_drag_preview(&mut self, player_id: &str, transport: &mut dyn HostTransport) {
        if self.session.card_drag_previews.remove(player_id).is_none() {
            return;
        }
        let payload = JsonObject::new().with("playerId", player_id);
        transport.broadcast("cardDragPreviewEnd", payload.into(), self.except_for(player_id).as_deref());
    }

    pub fn publish_arrow_drag_preview(
        &mut self,
        player_id: &str,
        corners: (Option<f64>, Option<f64>, Option<f64>, Option<f64>),
        transport: &mut dyn HostTransport,
    ) {
        let (Some(fraction_x), Some(fraction_y), Some(fraction_x2), Some(fraction_y2)) = corners else { return };
        self.session.arrow_drag_previews.insert(
            player_id.to_string(),
            ArrowDragPreview { player_id: player_id.to_string(), fraction_x, fraction_y, fraction_x2, fraction_y2 },
        );
        let payload = JsonObject::new()
            .with("playerId", player_id)
            .with("fx", fraction_x)
            .with("fy", fraction_y)
            .with("fx2", fraction_x2)
            .with("fy2", fraction_y2);
        transport.broadcast("arrowDragPreview", payload.into(), self.except_for(player_id).as_deref());
    }

    pub fn clear_arrow_drag_preview(&mut self, player_id: &str, transport: &mut dyn HostTransport) {
        if self.session.arrow_drag_previews.remove(player_id).is_none() {
            return;
        }
        let payload = JsonObject::new().with("playerId", player_id);
        transport.broadcast("arrowDragPreviewEnd", payload.into(), self.except_for(player_id).as_deref());
    }

    /// The game definition this engine serves.
    pub fn game(&self) -> &GameDefinition {
        &self.session.game
    }
}
