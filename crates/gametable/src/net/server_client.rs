//! The connection to a CardTable server (the lobby server). Port of `src/net/server_client.lua`.
//!
//! It uses the same line protocol as peer hosting; requests carry `payload.rid` and are answered
//! with `{type="response", payload={rid, ok, error?, message?, ...}}`. A request's answer is either
//! polled with [`ServerClient::take_response`] or handed to a callback; a dropped connection
//! answers every waiting request with error `"disconnected"`.
//!
//! The lobby screens read this object's fields every frame (users, rooms, games, chat, the room
//! and trade). Game traffic for the room the player is in goes to the game channel, which
//! [`ServerGameChannel`] presents to the game screen like a peer connection.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use gametable_core::json::{JsonObject, JsonValue};
use gametable_core::models::GameDefinition;
use gametable_core::protocol::WireMessage;
use gametable_core::util::trim_lua_whitespace;
use sha2::{Digest, Sha256};

use crate::app::Frame;
use crate::data::settings::Settings;
use crate::net::connection::{Connection, HeartbeatAction};
use crate::net::game_connection::GameConnection;
use crate::net::server_models::{ServerChatLine, ServerGameInfo, ServerInfo, ServerRoom, ServerTrade, ServerUser};

pub type SharedServer = Rc<RefCell<ServerClient>>;

/// Called with a request's response (from inside [`ServerClient::update`]; it must not borrow
/// the client).
pub type ResponseCallback = Box<dyn FnOnce(&JsonValue)>;

/// Messages that belong to the game screen rather than the lobby.
const GAME_MESSAGE_TYPES: [&str; 6] =
    ["gameData", "fullState", "cardDragPreview", "cardDragPreviewEnd", "arrowDragPreview", "arrowDragPreviewEnd"];

/// `S.DISCONNECT_MESSAGES`.
fn disconnect_message_for(reason: &str) -> Option<&'static str> {
    Some(match reason {
        "kicked" => "An admin disconnected you from the server.",
        "accountDisabled" => "Your account has been disabled.",
        "accountDeleted" => "Your account has been deleted.",
        "loggedInElsewhere" => "Your account signed in from another connection.",
        "serverShutdown" => "The server shut down.",
        "serverShuttingDown" => "The server is shutting down and isn't accepting connections.",
        "versionMismatch" => "This server needs a different version of CardTable.",
        _ => return None,
    })
}

/// `S.passwordHash`: the client hashes the password before sending it (there's no TLS on the
/// wire); the server bcrypts the hash.
pub fn password_hash(email: &str, password: &str) -> String {
    let raw = format!("cardflux:{}:{password}", trim_lua_whitespace(&email.to_lowercase()));
    Sha256::digest(raw.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}

pub const MIN_PASSWORD_LENGTH: usize = 6;

/// `S.passwordError`: why a password is too short to submit (`noun` reads "new password" where
/// that fits).
pub fn password_error(password: &str, noun: Option<&str>) -> Option<String> {
    (password.len() < MIN_PASSWORD_LENGTH)
        .then(|| format!("Use a {} of at least {MIN_PASSWORD_LENGTH} characters.", noun.unwrap_or("password")))
}

// ---------------------------------------------------------------------------
// Saved sign-ins and the last game picked, per server ("host:port")
// ---------------------------------------------------------------------------

const SESSIONS_KEY: &str = "server_sessions";
const LAST_GAME_KEY: &str = "last_selected_game";

/// A remembered sign-in: kept only when the player ticks "Remember me".
#[derive(Debug, Clone, PartialEq)]
pub struct SavedSession {
    pub token: Option<String>,
    pub email: Option<String>,
    pub remember: bool,
}

fn object_setting(settings: &Settings, key: &str) -> JsonObject {
    settings.get(key).and_then(JsonValue::as_object).cloned().unwrap_or_default()
}

/// `S.savedSession(key)`.
pub fn saved_session(settings: &Settings, server_key: &str) -> Option<SavedSession> {
    let entry = settings.get(SESSIONS_KEY)?.get(server_key)?;
    let text = |field: &str| entry.get(field).and_then(JsonValue::as_str).map(str::to_string);
    Some(SavedSession {
        token: text("token"),
        email: text("email"),
        remember: entry.get("remember").and_then(JsonValue::as_bool) == Some(true),
    })
}

/// `S.saveSession(key, token, email)`.
pub fn save_session(settings: &mut Settings, server_key: &str, token: &str, email: &str) {
    let mut all = object_setting(settings, SESSIONS_KEY);
    all.insert(server_key, JsonObject::new().with("token", token).with("email", email).with("remember", true));
    settings.set(SESSIONS_KEY, all);
}

/// `S.forgetSession(key)`: drops the token but keeps the email and Remember me, to prefill.
pub fn forget_session(settings: &mut Settings, server_key: &str) {
    let mut all = object_setting(settings, SESSIONS_KEY);
    let Some(entry) = all.get(server_key) else { return };
    let email = entry.get("email").cloned().unwrap_or(JsonValue::Null);
    let mut kept = JsonObject::new();
    if !matches!(email, JsonValue::Null) {
        kept.insert("email", email);
    }
    kept.insert("remember", true);
    all.insert(server_key, kept);
    settings.set(SESSIONS_KEY, all);
}

/// `S.clearSession(key)`: drops everything saved for the server, email included.
pub fn clear_session(settings: &mut Settings, server_key: &str) {
    let mut all = object_setting(settings, SESSIONS_KEY);
    if all.remove(server_key).is_some() {
        settings.set(SESSIONS_KEY, all);
    }
}

/// `S.lastGameId(key)`.
pub fn last_game_id(settings: &Settings, server_key: &str) -> Option<String> {
    settings.get(LAST_GAME_KEY)?.get(server_key)?.as_str().map(str::to_string)
}

/// `S.saveLastGameId(key, gameId)`.
pub fn save_last_game_id(settings: &mut Settings, server_key: &str, game_id: &str) {
    let mut all = object_setting(settings, LAST_GAME_KEY);
    all.insert(server_key, game_id);
    settings.set(LAST_GAME_KEY, all);
}

// ---------------------------------------------------------------------------
// The client
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerStatus {
    Connected,
    Disconnected,
}

/// What a waiting request's answer is for.
enum PendingResponse {
    /// Kept for [`ServerClient::take_response`].
    Poll,
    /// Dropped (fire and forget).
    Ignore,
    Callback(ResponseCallback),
}

/// Something that happened, for the lobby (or sign-in) screen to react to.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerEvent {
    LobbyState,
    Rooms,
    TradeInvite { trade_id: String, from_name: String },
    TradeUpdated,
    TradeClosed { trade_id: String, reason: String },
    RoomClosed { room_id: String, reason: String, mine: bool },
    Disconnected,
}

pub struct ServerClient {
    connection: Option<Connection>,
    pub host: String,
    pub port: u16,
    /// `host:port`, the key for saved sign-ins and the download cache.
    pub key: String,
    pub info: ServerInfo,
    pub status: ServerStatus,
    pub token: Option<String>,
    /// The signed-in user (`sc.self`).
    pub me: Option<ServerUser>,
    pub users: Vec<ServerUser>,
    pub rooms: Vec<ServerRoom>,
    pub games: Vec<ServerGameInfo>,
    pub chat: Vec<ServerChatLine>,
    pub room_chat: Vec<ServerChatLine>,
    pub trade_chat: Vec<ServerChatLine>,
    /// The room this client is in.
    pub room_id: Option<String>,
    pub spectating: bool,
    pub trade: Option<ServerTrade>,
    /// Set by the Trade screen while it is showing.
    pub trade_screen_open: bool,
    /// UI time when an announced shutdown happens.
    pub shutdown_at: Option<f64>,
    pub disconnect_reason: Option<String>,
    /// The game picked for Shop / Deck Editor / Collection / Create Game.
    pub selected_game_id: Option<String>,
    pub selected_game: Option<GameDefinition>,
    /// Downloaded games, memoized by manifest hash: game id -> (hash, game).
    pub loaded_games: HashMap<String, (String, GameDefinition)>,
    /// A game just picked on a "choose a game" screen, for the lobby to act on.
    pub picked_game: Option<(crate::screens::server::GamePickPurpose, String)>,
    next_rid: u64,
    pending: HashMap<u64, PendingResponse>,
    responses: HashMap<u64, JsonValue>,
    avatar_requests: HashMap<u64, String>,
    avatar_requested: HashSet<String>,
    avatar_keys: HashMap<String, String>,
    events: Vec<ServerEvent>,
    channel_open: bool,
    game_inbox: Vec<WireMessage>,
    last_game_data: Option<WireMessage>,
    last_full_state: Option<WireMessage>,
    last_update_frame: Option<u64>,
}

impl ServerClient {
    /// `S.adopt(conn, host, port, info)`: takes over a connection whose hello was answered with
    /// `serverInfo`.
    pub fn adopt(mut connection: Connection, host: &str, port: u16, info: &JsonValue) -> ServerClient {
        connection.start_heartbeat();
        ServerClient {
            connection: Some(connection),
            host: host.to_string(),
            port,
            key: format!("{host}:{port}"),
            info: ServerInfo::parse(info),
            status: ServerStatus::Connected,
            token: None,
            me: None,
            users: Vec::new(),
            rooms: Vec::new(),
            games: Vec::new(),
            chat: Vec::new(),
            room_chat: Vec::new(),
            trade_chat: Vec::new(),
            room_id: None,
            spectating: false,
            trade: None,
            trade_screen_open: false,
            shutdown_at: None,
            disconnect_reason: None,
            selected_game_id: None,
            selected_game: None,
            loaded_games: HashMap::new(),
            picked_game: None,
            next_rid: 0,
            pending: HashMap::new(),
            responses: HashMap::new(),
            avatar_requests: HashMap::new(),
            avatar_requested: HashSet::new(),
            avatar_keys: HashMap::new(),
            events: Vec::new(),
            channel_open: false,
            game_inbox: Vec::new(),
            last_game_data: None,
            last_full_state: None,
            last_update_frame: None,
        }
    }

    // -- Requests ---------------------------------------------------------------------------

    fn send_request(&mut self, message_type: &str, mut payload: JsonObject, pending: PendingResponse) -> u64 {
        self.next_rid += 1;
        let rid = self.next_rid;
        payload.insert("rid", rid as f64);
        if self.status == ServerStatus::Connected {
            self.pending.insert(rid, pending);
            self.send(message_type, payload.into());
        } else {
            self.answer(rid, pending, disconnected_response());
        }
        rid
    }

    /// Sends a request whose answer is fetched with [`ServerClient::take_response`].
    pub fn request(&mut self, message_type: &str, payload: JsonObject) -> u64 {
        self.send_request(message_type, payload, PendingResponse::Poll)
    }

    /// Sends a request and ignores its answer.
    pub fn request_ignored(&mut self, message_type: &str, payload: JsonObject) {
        self.send_request(message_type, payload, PendingResponse::Ignore);
    }

    /// Sends a request whose answer goes to `done`.
    pub fn request_with(&mut self, message_type: &str, payload: JsonObject, done: ResponseCallback) {
        self.send_request(message_type, payload, PendingResponse::Callback(done));
    }

    /// The answer to a polled request, once it has arrived.
    pub fn take_response(&mut self, rid: u64) -> Option<JsonValue> {
        self.responses.remove(&rid)
    }

    fn answer(&mut self, rid: u64, pending: PendingResponse, response: JsonValue) {
        match pending {
            PendingResponse::Poll => {
                self.responses.insert(rid, response);
            }
            PendingResponse::Ignore => {}
            PendingResponse::Callback(done) => done(&response),
        }
    }

    pub fn send(&mut self, message_type: &str, payload: JsonValue) {
        if let Some(connection) = &mut self.connection {
            connection.send(message_type, payload);
        }
    }

    pub fn is_admin(&self) -> bool {
        self.me.as_ref().is_some_and(|me| me.is_admin)
    }

    pub fn is_collection_mode(&self) -> bool {
        self.info.mode == "collection"
    }

    pub fn my_id(&self) -> &str {
        self.me.as_ref().map_or("", |me| me.id.as_str())
    }

    pub fn game_by_id(&self, game_id: &str) -> Option<&ServerGameInfo> {
        self.games.iter().find(|game| game.id == game_id)
    }

    pub fn room_by_id(&self, room_id: &str) -> Option<&ServerRoom> {
        self.rooms.iter().find(|room| room.id == room_id)
    }

    /// The room this client is in (kept fresh from `rooms` broadcasts).
    pub fn current_room(&self) -> Option<&ServerRoom> {
        self.room_id.as_deref().and_then(|room_id| self.room_by_id(room_id))
    }

    pub fn take_events(&mut self) -> Vec<ServerEvent> {
        std::mem::take(&mut self.events)
    }

    // -- Avatars ----------------------------------------------------------------------------

    /// `S:avatarKey(hash)`: the image cache key for a user's avatar, fetched on first use; `None`
    /// while it loads or when they have none.
    pub fn avatar_key(&mut self, hash: &str) -> Option<String> {
        if hash.is_empty() {
            return None;
        }
        if let Some(key) = self.avatar_keys.get(hash) {
            return Some(key.clone());
        }
        if self.avatar_requested.insert(hash.to_string()) {
            let rid = self.request("avatarGet", JsonObject::new().with("hash", hash));
            self.avatar_requests.insert(rid, hash.to_string());
        }
        None
    }

    /// `S:avatarKeysByUserId()`: every online user's avatar key (for the table).
    pub fn avatar_keys_by_user_id(&mut self) -> HashMap<String, String> {
        let users: Vec<(String, String)> =
            self.users.iter().map(|user| (user.id.clone(), user.avatar_hash.clone())).collect();
        users.into_iter().filter_map(|(id, hash)| Some((id, self.avatar_key(&hash)?))).collect()
    }

    fn finish_avatar_downloads(&mut self, frame: &mut Frame) {
        let rids: Vec<u64> = self.avatar_requests.keys().copied().collect();
        for rid in rids {
            let Some(response) = self.responses.remove(&rid) else { continue };
            let Some(hash) = self.avatar_requests.remove(&rid) else { continue };
            if response.get("ok").and_then(JsonValue::as_bool) != Some(true) {
                continue;
            }
            let data = response.get("data").and_then(JsonValue::as_str).unwrap_or("");
            let Ok(bytes) = BASE64.decode(data) else { continue };
            let key = format!("srvavatar:{hash}");
            let _ = frame.images.put_bytes(&key, &bytes);
            self.avatar_keys.insert(hash, key);
        }
    }

    // -- Game channel -----------------------------------------------------------------------

    /// `S:gameChannel()`: starts passing game traffic to the game screen.
    pub fn open_channel(&mut self) {
        self.channel_open = true;
    }

    /// `S:closeChannel()`.
    pub fn close_channel(&mut self) {
        self.channel_open = false;
        self.game_inbox.clear();
        self.last_game_data = None;
        self.last_full_state = None;
    }

    pub fn channel_open(&self) -> bool {
        self.channel_open
    }

    /// The game has started (gameData arrived on the channel).
    pub fn has_game_data(&self) -> bool {
        self.last_game_data.is_some()
    }

    // -- Pump -------------------------------------------------------------------------------

    fn handle(&mut self, frame: &mut Frame, message: WireMessage) {
        let payload = &message.payload;
        let list_of = |key: &str| payload.get(key).and_then(JsonValue::as_array).cloned().unwrap_or_default();
        let sorted_rooms = |items: Vec<JsonValue>| {
            let mut rooms: Vec<ServerRoom> = items.iter().map(ServerRoom::parse).collect();
            rooms.sort_by(|first, second| first.created_at.total_cmp(&second.created_at));
            rooms
        };
        match message.message_type.as_str() {
            "response" => {
                let Some(rid) = payload.get("rid").and_then(JsonValue::as_f64).map(|rid| rid as u64) else { return };
                if let Some(pending) = self.pending.remove(&rid) {
                    self.answer(rid, pending, message.payload.clone());
                }
            }
            "lobbyState" => {
                if let Some(server) = payload.get("server") {
                    self.info = ServerInfo::parse(server);
                }
                self.me = payload.get("self").filter(|me| me.as_object().is_some()).map(ServerUser::parse);
                self.users = list_of("users").iter().map(ServerUser::parse).collect();
                self.rooms = sorted_rooms(list_of("rooms"));
                self.games = list_of("games").iter().map(ServerGameInfo::parse).collect();
                self.chat = list_of("chat").iter().map(ServerChatLine::parse).collect();
                self.events.push(ServerEvent::LobbyState);
            }
            "users" => self.users = list_of("users").iter().map(ServerUser::parse).collect(),
            "rooms" => {
                self.rooms = sorted_rooms(list_of("rooms"));
                self.events.push(ServerEvent::Rooms);
            }
            "games" => self.games = list_of("games").iter().map(ServerGameInfo::parse).collect(),
            "serverInfo" => self.info = ServerInfo::parse(payload),
            "self" => self.me = Some(ServerUser::parse(payload)),
            "tokens" => {
                if let Some(me) = &mut self.me {
                    me.tokens = payload.get("tokens").and_then(JsonValue::as_f64).unwrap_or(0.0) as i64;
                }
            }
            "newToken" => {
                let token = payload.get("token").and_then(JsonValue::as_str).unwrap_or("").to_string();
                self.token = Some(token.clone());
                // Replace the remembered token, but don't start remembering one.
                if let Some(saved) = saved_session(frame.settings, &self.key)
                    && saved.remember
                    && saved.token.is_some()
                {
                    let email = saved.email.or_else(|| self.me.as_ref().map(|me| me.email.clone())).unwrap_or_default();
                    save_session(frame.settings, &self.key, &token, &email);
                }
            }
            "chat" => {
                let line = payload.get("line").map(ServerChatLine::parse).unwrap_or_default();
                let list = if payload.get("roomId").and_then(JsonValue::as_str).is_some() {
                    &mut self.room_chat
                } else if payload.get("tradeId").and_then(JsonValue::as_str).is_some() {
                    &mut self.trade_chat
                } else {
                    &mut self.chat
                };
                list.push(line);
                if list.len() > 200 {
                    list.remove(0);
                }
            }
            "chatCleared" => self.chat.clear(),
            "toast" => frame.ui.toast(payload.get("message").and_then(JsonValue::as_str).unwrap_or(""), None),
            "tradeInvite" => self.events.push(ServerEvent::TradeInvite {
                trade_id: payload.get("tradeId").and_then(JsonValue::as_str).unwrap_or("").to_string(),
                from_name: payload.get("fromName").and_then(JsonValue::as_str).unwrap_or("").to_string(),
            }),
            "trade" => {
                let trade = ServerTrade::parse(payload);
                if self.trade.as_ref().is_none_or(|current| current.id != trade.id) {
                    self.trade_chat = trade.chat.clone();
                }
                self.trade = Some(trade);
                self.events.push(ServerEvent::TradeUpdated);
            }
            "tradeClosed" => {
                let trade_id = payload.get("tradeId").and_then(JsonValue::as_str).unwrap_or("").to_string();
                if self.trade.as_ref().is_some_and(|trade| trade.id == trade_id) {
                    self.trade = None;
                }
                let reason = payload.get("reason").and_then(JsonValue::as_str).unwrap_or("").to_string();
                self.events.push(ServerEvent::TradeClosed { trade_id, reason });
            }
            "serverShutdown" => {
                // An admin scheduled a shutdown: warn on whatever screen is showing.
                let seconds = payload.get("seconds").and_then(JsonValue::as_f64).unwrap_or(0.0);
                self.shutdown_at = Some(frame.ui.time() + seconds);
                let minutes = ((seconds / 60.0 + 0.5).floor() as i64).max(1);
                let unit = if minutes == 1 { " minute" } else { " minutes" };
                frame.ui.alert(
                    "Server Shutting Down",
                    &format!(
                        "The server will shut down in {minutes}{unit}. Finish up and save anything you want to keep."
                    ),
                );
            }
            "roomClosed" => {
                let room_id = payload.get("roomId").and_then(JsonValue::as_str).unwrap_or("").to_string();
                let mine = self.room_id.as_deref() == Some(room_id.as_str());
                if mine {
                    self.room_id = None;
                    self.spectating = false;
                    self.room_chat.clear();
                }
                let reason = payload.get("reason").and_then(JsonValue::as_str).unwrap_or("").to_string();
                self.events.push(ServerEvent::RoomClosed { room_id, reason, mine });
            }
            "disconnect" => {
                self.disconnect_reason = payload.get("reason").and_then(JsonValue::as_str).map(str::to_string);
            }
            // Game messages only matter while a table's channel is open.
            message_type if GAME_MESSAGE_TYPES.contains(&message_type) && self.channel_open => {
                match message_type {
                    "gameData" => self.last_game_data = Some(message.clone()),
                    "fullState" => self.last_full_state = Some(message.clone()),
                    _ => {}
                }
                self.game_inbox.push(message);
            }
            _ => {}
        }
    }

    /// One pass (at most once a frame): reads messages, keeps the heartbeat, finishes avatars.
    pub fn update(&mut self, frame: &mut Frame) {
        let frame_index = frame.ui.frame_index();
        if self.last_update_frame == Some(frame_index) {
            return;
        }
        self.last_update_frame = Some(frame_index);
        if self.status != ServerStatus::Connected {
            return;
        }
        let Some((messages, got_data)) = self.connection.as_mut().map(Connection::poll) else { return };
        for message in messages {
            if message.message_type == "ping" {
                self.send("pong", JsonObject::new().into());
            } else {
                self.handle(frame, message);
            }
        }
        let mut lost = false;
        if let Some(connection) = &mut self.connection {
            match connection.heartbeat(got_data) {
                Some(HeartbeatAction::Timeout) => lost = true,
                Some(HeartbeatAction::Ping) => connection.send("ping", JsonObject::new().into()),
                None => {}
            }
            lost |= connection.closed;
        }
        if lost {
            self.handle_disconnect();
        }
        self.finish_avatar_downloads(frame);
    }

    fn handle_disconnect(&mut self) {
        if self.status == ServerStatus::Disconnected {
            return;
        }
        self.status = ServerStatus::Disconnected;
        if let Some(connection) = &mut self.connection {
            connection.close();
        }
        for (rid, pending) in std::mem::take(&mut self.pending) {
            self.answer(rid, pending, disconnected_response());
        }
        self.events.push(ServerEvent::Disconnected);
    }

    /// Leaves the server on purpose.
    pub fn disconnect(&mut self) {
        self.close_channel();
        if let Some(connection) = &mut self.connection {
            connection.close();
        }
        self.status = ServerStatus::Disconnected;
        self.events.clear();
    }

    /// `S:disconnectMessage()`.
    pub fn disconnect_message(&self) -> String {
        self.disconnect_reason
            .as_deref()
            .and_then(disconnect_message_for)
            .unwrap_or("Lost connection to the server.")
            .to_string()
    }
}

fn disconnected_response() -> JsonValue {
    JsonObject::new()
        .with("ok", false)
        .with("error", "disconnected")
        .with("message", "Lost connection to the server.")
        .into()
}

/// A response's `ok`.
pub fn response_ok(response: &JsonValue) -> bool {
    response.get("ok").and_then(JsonValue::as_bool) == Some(true)
}

/// A response's `message`, or `fallback`.
pub fn response_message(response: &JsonValue, fallback: &str) -> String {
    response.get("message").and_then(JsonValue::as_str).unwrap_or(fallback).to_string()
}

/// The room's game traffic, presented to the game screen like a peer connection
/// (`S:gameChannel()`).
pub struct ServerGameChannel {
    pub server: SharedServer,
}

impl GameConnection for ServerGameChannel {
    fn pump(&mut self, frame: &mut Frame) {
        self.server.borrow_mut().update(frame);
    }

    fn take_messages(&mut self) -> Vec<WireMessage> {
        std::mem::take(&mut self.server.borrow_mut().game_inbox)
    }

    fn cached_messages(&self) -> Vec<WireMessage> {
        let server = self.server.borrow();
        [&server.last_game_data, &server.last_full_state].into_iter().flatten().cloned().collect()
    }

    fn send(&mut self, message_type: &str, payload: JsonValue) {
        self.server.borrow_mut().send(message_type, payload);
    }

    fn is_disconnected(&self) -> bool {
        self.server.borrow().status == ServerStatus::Disconnected
    }

    fn disconnect_reason(&self) -> Option<String> {
        self.server.borrow().disconnect_reason.clone()
    }

    /// The game is done with the channel; the server connection stays up for the lobby.
    fn close(&mut self) {
        self.server.borrow_mut().close_channel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_matches_the_original() {
        // sha256("cardflux:ann@example.com:secret1")
        let expected: String =
            Sha256::digest(b"cardflux:ann@example.com:secret1").iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(password_hash("  Ann@Example.COM ", "secret1"), expected);
        assert_eq!(expected.len(), 64);
        assert_eq!(password_error("12345", None), Some("Use a password of at least 6 characters.".to_string()));
        assert_eq!(
            password_error("12345", Some("new password")).unwrap(),
            "Use a new password of at least 6 characters."
        );
        assert_eq!(password_error("123456", None), None);
    }

    #[test]
    fn sessions_and_last_game() {
        let mut settings = Settings::in_memory();
        assert_eq!(saved_session(&settings, "h:1"), None);
        save_session(&mut settings, "h:1", "tok", "a@b.c");
        assert_eq!(
            saved_session(&settings, "h:1"),
            Some(SavedSession { token: Some("tok".into()), email: Some("a@b.c".into()), remember: true })
        );
        forget_session(&mut settings, "h:1");
        assert_eq!(
            saved_session(&settings, "h:1"),
            Some(SavedSession { token: None, email: Some("a@b.c".into()), remember: true })
        );
        clear_session(&mut settings, "h:1");
        assert_eq!(saved_session(&settings, "h:1"), None);
        save_last_game_id(&mut settings, "h:1", "lorcana");
        assert_eq!(last_game_id(&settings, "h:1").as_deref(), Some("lorcana"));
        assert_eq!(last_game_id(&settings, "h:2"), None);
    }
}
