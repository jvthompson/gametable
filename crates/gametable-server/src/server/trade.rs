//! Private two-player card trades in collection mode. Port of `trade.go`.
//!
//! ```text
//! A -> tradeRequest {targetId, gameId}     B <- tradeInvite      (expires after 30 s)
//! B -> tradeRespond {tradeId, accept}      A, B <- trade {...}   or A <- tradeClosed{declined}
//! either -> tradeOffer {tradeId, cardId, delta}     (what the caller wants from the other)
//! either -> tradeConfirm {tradeId, confirmed}       both confirmed: the cards change hands
//! either -> tradeLeave {tradeId}
//! ```

use std::collections::BTreeMap;
use std::time::Duration;

use gametable_core::json::{JsonObject, JsonValue};

use super::decks::quantities_object;
use super::{ClientId, Request, Server, ServerState};
use crate::connection::{Connection, encode_message};
use crate::log;
use crate::payload::sorted_object;
use crate::store::{ChatLine, User, new_id, unix_now};

/// How long an invite waits for an answer.
const PENDING_TRADE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TRADE_CHAT_CHARACTERS: usize = 500;
const TRADE_CHAT_HISTORY: usize = 100;

/// An invite waiting on `tradeRespond`. Guarded by the server state lock.
pub(crate) struct PendingTrade {
    pub id: String,
    pub game_id: String,
    pub game_name: String,
    pub from: ClientId,
    pub to: ClientId,
}

/// What one side wants from the other.
#[derive(Default)]
pub(crate) struct TradeOffer {
    /// Card id -> copies this user wants from the other.
    pub cards: BTreeMap<String, i64>,
    pub confirmed: bool,
}

/// A running trade. Guarded by the server state lock, like rooms.
pub(crate) struct Trade {
    pub id: String,
    pub game_id: String,
    pub game_name: String,
    pub a_user_id: String,
    pub b_user_id: String,
    pub a: ClientId,
    pub b: ClientId,
    /// Keyed by the *wanting* user's id.
    pub offers: BTreeMap<String, TradeOffer>,
    pub chat: Vec<ChatLine>,
}

impl Trade {
    fn has_client(&self, client_id: ClientId) -> bool {
        self.a == client_id || self.b == client_id
    }

    fn other_user_id(&self, user_id: &str) -> &str {
        if self.a_user_id == user_id { &self.b_user_id } else { &self.a_user_id }
    }

    fn connections(&self, state: &ServerState) -> Vec<Connection> {
        [self.a, self.b].iter().filter_map(|id| state.clients.get(id)).map(|c| c.connection.clone()).collect()
    }

    fn unconfirm_all(&mut self) {
        for offer in self.offers.values_mut() {
            offer.confirmed = false;
        }
    }

    /// The shared view of the trade sent to both participants.
    fn info(&self, state: &ServerState) -> JsonValue {
        let summary = |client_id: ClientId, user_id: &str| match state.user_of(client_id) {
            Some(user) => user_summary(user),
            None => sorted_object([("id", user_id.into())]),
        };
        let mut offers = JsonObject::new();
        for (user_id, offer) in &self.offers {
            offers.insert(
                user_id,
                sorted_object([("cards", quantities_object(&offer.cards)), ("confirmed", offer.confirmed.into())]),
            );
        }
        // Go encodes a trade without chat lines as `null`.
        let chat = if self.chat.is_empty() {
            JsonValue::Null
        } else {
            JsonValue::Array(self.chat.iter().map(ChatLine::to_json).collect())
        };
        sorted_object([
            ("id", self.id.as_str().into()),
            ("gameId", self.game_id.as_str().into()),
            ("gameName", self.game_name.as_str().into()),
            ("a", summary(self.a, &self.a_user_id)),
            ("b", summary(self.b, &self.b_user_id)),
            ("offers", JsonValue::Object(offers)),
            ("chat", chat),
        ])
    }
}

fn user_summary(user: &User) -> JsonValue {
    sorted_object([
        ("id", user.id.as_str().into()),
        ("name", user.name.as_str().into()),
        ("color", user.color.into()),
        ("avatarHash", user.avatar_hash.as_str().into()),
    ])
}

/// Why a client can't trade right now, if it can't.
fn trade_ineligibility(state: &ServerState, client_id: ClientId) -> Option<&'static str> {
    let client = state.clients.get(&client_id)?;
    if client.room_id.is_some() {
        Some("inRoom")
    } else if client.trade_id.is_some() || client.pending_trade_id.is_some() {
        Some("trading")
    } else {
        None
    }
}

fn clear_pending_links(state: &mut ServerState, pending: &PendingTrade) {
    for client_id in [pending.from, pending.to] {
        if let Some(client) = state.clients.get_mut(&client_id) {
            client.pending_trade_id = None;
        }
    }
}

fn clear_trade_links(state: &mut ServerState, trade: &Trade) {
    for client_id in [trade.a, trade.b] {
        if let Some(client) = state.clients.get_mut(&client_id) {
            client.trade_id = None;
        }
    }
}

fn trade_closed(trade_id: &str, reason: &str) -> JsonValue {
    sorted_object([("tradeId", trade_id.into()), ("reason", reason.into())])
}

fn send_to_all(connections: &[Connection], message_type: &str, payload: &JsonValue) {
    let line = encode_message(message_type, payload);
    for connection in connections {
        connection.send_line(line.clone());
    }
}

impl Server {
    fn expire_pending_trade(&self, trade_id: &str) {
        let from_connection = {
            let mut state = self.state();
            let Some(pending) = state.pending_trades.remove(trade_id) else { return };
            clear_pending_links(&mut state, &pending);
            state.clients.get(&pending.from).map(|client| client.connection.clone())
        };
        if let Some(connection) = from_connection {
            connection.send("tradeClosed", trade_closed(trade_id, "expired"));
        }
    }

    /// Ends a disconnecting client's trade or pending invite, telling the other side.
    pub(crate) fn leave_trade(&self, client_id: ClientId) {
        let mut notices: Vec<(Connection, JsonValue)> = Vec::new();
        {
            let mut state = self.state();
            let Some(client) = state.clients.get(&client_id) else { return };
            let (pending_id, trade_id) = (client.pending_trade_id.clone(), client.trade_id.clone());
            if let Some(pending) = pending_id.and_then(|id| state.pending_trades.remove(&id)) {
                clear_pending_links(&mut state, &pending);
                let other = if pending.to == client_id { pending.from } else { pending.to };
                if let Some(other) = state.clients.get(&other) {
                    notices.push((other.connection.clone(), trade_closed(&pending.id, "left")));
                }
            }
            if let Some(trade) = trade_id.and_then(|id| state.trades.remove(&id)) {
                clear_trade_links(&mut state, &trade);
                let other = if trade.a == client_id { trade.b } else { trade.a };
                if let Some(other) = state.clients.get(&other) {
                    notices.push((other.connection.clone(), trade_closed(&trade.id, "left")));
                }
            }
        }
        for (connection, payload) in notices {
            connection.send("tradeClosed", payload);
        }
    }
}

impl Request<'_> {
    pub(super) fn handle_trade_request(&self, user: &User) {
        let server = self.server;
        if !server.settings().is_collection_mode() {
            self.fail("notCollection", "Trading is only available in collection mode.");
            return;
        }
        let game_id = self.fields.text("gameId");
        let target_id = self.fields.text("targetId");
        let Some(game_name) = server.library.read().unwrap().game(&game_id).map(|game| game.name.clone()) else {
            self.fail("noGame", "The server doesn't have that game.");
            return;
        };
        if target_id == user.id {
            self.fail("badTarget", "You can't trade with yourself.");
            return;
        }
        let (trade_id, target_connection, target_name) = {
            let mut state = server.state();
            let Some(target_client_id) = state.client_by_user_id.get(&target_id).copied() else {
                drop(state);
                self.fail("offline", "That player isn't online.");
                return;
            };
            if let Some(reason) = trade_ineligibility(&state, self.client_id) {
                drop(state);
                self.fail(reason, "You can't start a trade right now.");
                return;
            }
            if let Some(reason) = trade_ineligibility(&state, target_client_id) {
                drop(state);
                self.fail(reason, "That player can't trade right now.");
                return;
            }
            let trade_id = new_id();
            let pending = PendingTrade {
                id: trade_id.clone(),
                game_id: game_id.clone(),
                game_name: game_name.clone(),
                from: self.client_id,
                to: target_client_id,
            };
            state.pending_trades.insert(trade_id.clone(), pending);
            for client_id in [self.client_id, target_client_id] {
                if let Some(client) = state.clients.get_mut(&client_id) {
                    client.pending_trade_id = Some(trade_id.clone());
                }
            }
            let target = &state.clients[&target_client_id];
            let target_name = target.user.as_ref().map(|target| target.name.clone()).unwrap_or_default();
            (trade_id, target.connection.clone(), target_name)
        };
        let expiring_server = server.clone();
        let expiring_id = trade_id.clone();
        let expire = move || expiring_server.expire_pending_trade(&expiring_id);
        match server.runtime() {
            Some(runtime) => {
                runtime.spawn(async move {
                    tokio::time::sleep(PENDING_TRADE_TIMEOUT).await;
                    tokio::task::spawn_blocking(expire);
                });
            }
            None => {
                std::thread::spawn(move || {
                    std::thread::sleep(PENDING_TRADE_TIMEOUT);
                    expire();
                });
            }
        }
        self.reply(JsonObject::new().with("tradeId", trade_id.as_str()));
        target_connection.send(
            "tradeInvite",
            sorted_object([
                ("tradeId", trade_id.as_str().into()),
                ("gameId", game_id.as_str().into()),
                ("gameName", game_name.as_str().into()),
                ("fromId", user.id.as_str().into()),
                ("fromName", user.name.as_str().into()),
                ("fromColor", user.color.into()),
            ]),
        );
        log!(server, "{} asked {target_name} to trade ({game_name})", user.name);
    }

    pub(super) fn handle_trade_respond(&self) {
        let server = self.server;
        let trade_id = self.fields.text("tradeId");
        let accept = self.fields.flag("accept");
        let mut state = server.state();
        let pending = match state.pending_trades.get(&trade_id) {
            Some(pending) if pending.to == self.client_id => state.pending_trades.remove(&trade_id).unwrap(),
            _ => {
                drop(state);
                self.fail("noTrade", "That trade request is no longer available.");
                return;
            }
        };
        clear_pending_links(&mut state, &pending);
        let from_connection = state.clients.get(&pending.from).map(|client| client.connection.clone());
        if !accept {
            drop(state);
            self.reply_empty();
            if let Some(connection) = from_connection {
                connection.send("tradeClosed", trade_closed(&pending.id, "declined"));
            }
            return;
        }
        let user_id_of = |client_id| state.user_of(client_id).map(|user| user.id.clone()).unwrap_or_default();
        let name_of = |client_id| state.user_of(client_id).map(|user| user.name.clone()).unwrap_or_default();
        let (a_user_id, b_user_id) = (user_id_of(pending.from), user_id_of(pending.to));
        let (a_name, b_name) = (name_of(pending.from), name_of(pending.to));
        let trade = Trade {
            id: pending.id.clone(),
            game_id: pending.game_id.clone(),
            game_name: pending.game_name.clone(),
            offers: [(a_user_id.clone(), TradeOffer::default()), (b_user_id.clone(), TradeOffer::default())]
                .into_iter()
                .collect(),
            a_user_id,
            b_user_id,
            a: pending.from,
            b: pending.to,
            chat: Vec::new(),
        };
        for client_id in [trade.a, trade.b] {
            if let Some(client) = state.clients.get_mut(&client_id) {
                client.trade_id = Some(trade.id.clone());
            }
        }
        let info = trade.info(&state);
        let connections = trade.connections(&state);
        state.trades.insert(trade.id.clone(), trade);
        drop(state);
        self.reply_empty();
        send_to_all(&connections, "trade", &info);
        log!(server, "trade {} started ({a_name} / {b_name})", pending.id);
    }

    /// Whether the caller belongs to the active trade; fails the request when not.
    fn check_trade_member(&self, state: &ServerState, trade_id: &str) -> bool {
        state.trades.get(trade_id).is_some_and(|trade| trade.has_client(self.client_id))
    }

    pub(super) fn handle_trade_offer(&self, user: &User) {
        let server = self.server;
        let trade_id = self.fields.text("tradeId");
        let card_id = self.fields.text("cardId");
        let delta = self.fields.integer("delta");
        let (giver_id, game_id) = {
            let state = server.state();
            if !self.check_trade_member(&state, &trade_id) {
                drop(state);
                self.fail("noTrade", "That trade is no longer active.");
                return;
            }
            let trade = &state.trades[&trade_id];
            (trade.other_user_id(&user.id).to_string(), trade.game_id.clone())
        };
        let Ok(owned) = server.store().collection(&giver_id, &game_id) else {
            self.fail("internal", "Could not check the collection.");
            return;
        };
        let mut state = server.state();
        if !self.check_trade_member(&state, &trade_id) {
            drop(state);
            self.fail("noTrade", "That trade is no longer active.");
            return;
        }
        let trade = state.trades.get_mut(&trade_id).unwrap();
        let offer = trade.offers.entry(user.id.clone()).or_default();
        let before = offer.cards.get(&card_id).copied().unwrap_or(0);
        let most = owned.get(&card_id).copied().unwrap_or(0);
        let after = before.saturating_add(delta).max(0).min(most);
        if after == 0 {
            offer.cards.remove(&card_id);
        } else {
            offer.cards.insert(card_id, after);
        }
        if after != before {
            trade.unconfirm_all();
        }
        let info = state.trades[&trade_id].info(&state);
        let connections = state.trades[&trade_id].connections(&state);
        drop(state);
        self.reply_empty();
        send_to_all(&connections, "trade", &info);
    }

    pub(super) fn handle_trade_confirm(&self, user: &User) {
        let server = self.server;
        let trade_id = self.fields.text("tradeId");
        let confirmed = self.fields.flag("confirmed");
        let mut state = server.state();
        if !self.check_trade_member(&state, &trade_id) {
            drop(state);
            self.fail("noTrade", "That trade is no longer active.");
            return;
        }
        state.trades.get_mut(&trade_id).unwrap().offers.entry(user.id.clone()).or_default().confirmed = confirmed;
        let trade = &state.trades[&trade_id];
        let is_confirmed = |user_id: &str| trade.offers.get(user_id).is_some_and(|offer| offer.confirmed);
        if !(is_confirmed(&trade.a_user_id) && is_confirmed(&trade.b_user_id)) {
            let info = trade.info(&state);
            let connections = state.trades[&trade_id].connections(&state);
            drop(state);
            self.reply_empty();
            send_to_all(&connections, "trade", &info);
            return;
        }
        // A gives up what B wants; B gives up what A wants.
        let wanted_by = |user_id: &str| trade.offers.get(user_id).map(|offer| offer.cards.clone()).unwrap_or_default();
        let gives_a = wanted_by(&trade.b_user_id);
        let gives_b = wanted_by(&trade.a_user_id);
        let (a_user_id, b_user_id, game_id) = (trade.a_user_id.clone(), trade.b_user_id.clone(), trade.game_id.clone());
        drop(state);

        let executed = server.store().execute_trade(&game_id, &a_user_id, &gives_a, &b_user_id, &gives_b);
        let mut state = server.state();
        if executed.is_err() {
            if let Some(trade) = state.trades.get_mut(&trade_id) {
                trade.unconfirm_all();
            }
            let view = state.trades.get(&trade_id).map(|trade| (trade.info(&state), trade.connections(&state)));
            drop(state);
            self.fail("unavailable", "One of the offered cards is no longer available.");
            if let Some((info, connections)) = view {
                send_to_all(&connections, "trade", &info);
            }
            return;
        }
        let Some(trade) = state.trades.remove(&trade_id) else { return };
        clear_trade_links(&mut state, &trade);
        let name_of = |client_id| state.user_of(client_id).map(|user| user.name.clone()).unwrap_or_default();
        let (a_name, b_name) = (name_of(trade.a), name_of(trade.b));
        let connections = trade.connections(&state);
        drop(state);
        self.reply_empty();
        let closed = sorted_object([
            ("tradeId", trade_id.as_str().into()),
            ("reason", "completed".into()),
            ("gaveA", quantities_object(&gives_a)),
            ("gaveB", quantities_object(&gives_b)),
        ]);
        send_to_all(&connections, "tradeClosed", &closed);
        log!(server, "trade {trade_id} completed ({a_name} / {b_name})");
    }

    pub(super) fn handle_trade_leave(&self) {
        let server = self.server;
        let trade_id = self.fields.text("tradeId");
        let mut state = server.state();
        let is_pending_member = state
            .pending_trades
            .get(&trade_id)
            .is_some_and(|pending| pending.from == self.client_id || pending.to == self.client_id);
        let connections = if is_pending_member {
            let pending = state.pending_trades.remove(&trade_id).unwrap();
            clear_pending_links(&mut state, &pending);
            // Both sides are told, including the caller: a client that doesn't hear back about
            // its own trade ending would keep stale state.
            [pending.from, pending.to]
                .iter()
                .filter_map(|id| state.clients.get(id))
                .map(|client| client.connection.clone())
                .collect()
        } else if self.check_trade_member(&state, &trade_id) {
            let trade = state.trades.remove(&trade_id).unwrap();
            clear_trade_links(&mut state, &trade);
            trade.connections(&state)
        } else {
            Vec::new()
        };
        drop(state);
        self.reply_empty();
        send_to_all(&connections, "tradeClosed", &trade_closed(&trade_id, "left"));
    }

    pub(super) fn handle_trade_collection_get(&self, user: &User) {
        let server = self.server;
        let trade_id = self.fields.text("tradeId");
        let (partner_id, game_id) = {
            let state = server.state();
            if !self.check_trade_member(&state, &trade_id) {
                drop(state);
                self.fail("noTrade", "That trade is no longer active.");
                return;
            }
            let trade = &state.trades[&trade_id];
            (trade.other_user_id(&user.id).to_string(), trade.game_id.clone())
        };
        let Ok(collection) = server.store().collection(&partner_id, &game_id) else {
            self.fail("internal", "Could not load that collection.");
            return;
        };
        self.reply(JsonObject::new().with("gameId", game_id).with("cards", quantities_object(&collection)));
    }

    /// A chat line inside a trade (a `chatSend` carrying a `tradeId`).
    pub(super) fn handle_trade_chat(&self, user: &User, text: &str, trade_id: &str) {
        let mut text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        if text.chars().count() > MAX_TRADE_CHAT_CHARACTERS {
            text = text.chars().take(MAX_TRADE_CHAT_CHARACTERS).collect();
        }
        let line = ChatLine { user_id: user.id.clone(), name: user.name.clone(), text, at: unix_now() };
        let connections = {
            let mut state = self.server.state();
            if !self.check_trade_member(&state, trade_id) {
                return;
            }
            let trade = state.trades.get_mut(trade_id).unwrap();
            trade.chat.push(line.clone());
            if trade.chat.len() > TRADE_CHAT_HISTORY {
                let excess = trade.chat.len() - TRADE_CHAT_HISTORY;
                trade.chat.drain(..excess);
            }
            state.trades[trade_id].connections(&state)
        };
        send_to_all(&connections, "chat", &sorted_object([("line", line.to_json()), ("tradeId", trade_id.into())]));
    }
}
