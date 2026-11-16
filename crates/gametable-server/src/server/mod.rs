//! The lobby server: accounts, chat, rooms running the game engine, decks, collections and
//! administration. Port of the Go server's `internal/server`.
//!
//! Every connection gets a tokio task that reads its messages; handlers are ordinary blocking
//! functions (they talk to SQLite and hash passwords) run with `block_in_place`. Shared lobby
//! state lives behind one mutex, like the Go server's `srv.mu`. Each running room owns its game
//! engine on a thread of its own (see `rooms`).

mod auth;
mod lobby;
mod rooms;
mod shutdown;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use gametable_core::json::{JsonObject, JsonValue};

use crate::connection::{Connection, IncomingMessage, encode_message};
use crate::games::GameLibrary;
use crate::log;
use crate::payload::{Fields, sorted, sorted_object};
use crate::server_log::ServerLog;
use crate::store::{ServerSettings, Store, StoreResult, User};

pub use auth::valid_player_name;
use rooms::RoomRecord;

/// Must match `serverProtocolVersion` in the client's `src/net/protocol.lua`.
pub const PROTOCOL_VERSION: i64 = gametable_core::SERVER_PROTOCOL_VERSION as i64;

/// Identifies one client connection (the Go server compares `*Client` pointers).
pub type ClientId = u64;

/// One connected client. Guarded by the server state lock.
pub(crate) struct ClientRecord {
    pub connection: Connection,
    pub user: Option<User>,
    pub room_id: Option<String>,
    pub spectating: bool,
}

/// Everything the Go server guards with `srv.mu`.
pub(crate) struct ServerState {
    pub clients: HashMap<ClientId, ClientRecord>,
    pub client_by_user_id: HashMap<String, ClientId>,
    pub rooms: HashMap<String, RoomRecord>,
    pub settings: ServerSettings,
    /// Set once an admin starts a shutdown; new connections and sign-ins are refused from then on.
    pub shutdown_at: Option<Instant>,
    pub shutdown_started_by: Option<ClientId>,
    pub shutdown_delay: Duration,
}

impl ServerState {
    pub fn user_of(&self, client_id: ClientId) -> Option<&User> {
        self.clients.get(&client_id).and_then(|client| client.user.as_ref())
    }
}

pub struct Server {
    pub store: Mutex<Store>,
    pub library: RwLock<GameLibrary>,
    pub log: ServerLog,
    state: Mutex<ServerState>,
    login_limiter: LoginLimiter,
    quit: watch::Sender<bool>,
    is_closed: AtomicBool,
    next_client_id: AtomicU64,
    /// Lets room threads and timers spawn work on the server's runtime.
    runtime: Mutex<Option<tokio::runtime::Handle>>,
}

impl Server {
    pub fn new(store: Store, library: GameLibrary, log: ServerLog) -> StoreResult<Arc<Server>> {
        let settings = store.settings()?;
        Ok(Arc::new(Server {
            store: Mutex::new(store),
            library: RwLock::new(library),
            log,
            state: Mutex::new(ServerState {
                clients: HashMap::new(),
                client_by_user_id: HashMap::new(),
                rooms: HashMap::new(),
                settings,
                shutdown_at: None,
                shutdown_started_by: None,
                shutdown_delay: shutdown::DEFAULT_SHUTDOWN_DELAY,
            }),
            login_limiter: LoginLimiter::default(),
            quit: watch::Sender::new(false),
            is_closed: AtomicBool::new(false),
            next_client_id: AtomicU64::new(1),
            runtime: Mutex::new(None),
        }))
    }

    pub(crate) fn state(&self) -> MutexGuard<'_, ServerState> {
        self.state.lock().unwrap()
    }

    pub(crate) fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap()
    }

    pub fn settings(&self) -> ServerSettings {
        self.state().settings.clone()
    }

    /// How long players get between an admin starting a shutdown and the server closing.
    pub fn set_shutdown_delay(&self, delay: Duration) {
        self.state().shutdown_delay = delay;
    }

    pub(crate) fn runtime(&self) -> Option<tokio::runtime::Handle> {
        self.runtime.lock().unwrap().clone()
    }

    /// Accepts connections until [`Server::close`].
    pub async fn serve(self: &Arc<Self>, listener: TcpListener) -> std::io::Result<()> {
        *self.runtime.lock().unwrap() = Some(tokio::runtime::Handle::current());
        tokio::spawn(lobby::run_allowance_loop(self.clone()));
        let mut quit = self.quit.subscribe();
        loop {
            tokio::select! {
                _ = crate::connection::wait_until_set(&mut quit) => return Ok(()),
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        tokio::spawn(self.clone().handle_connection(stream));
                    }
                    Err(error) if is_transient_accept_error(&error) => continue,
                    Err(error) => return Err(error),
                },
            }
        }
    }

    /// Stops the server: rooms close, everyone is disconnected and `serve` returns. Safe to call
    /// more than once.
    pub fn close(&self) {
        if self.is_closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.quit.send_replace(true);
        let (room_ids, connections): (Vec<String>, Vec<Connection>) = {
            let state = self.state();
            (
                state.rooms.keys().cloned().collect(),
                state.clients.values().map(|client| client.connection.clone()).collect(),
            )
        };
        for room_id in room_ids {
            self.close_room(&room_id, "serverShutdown");
        }
        for connection in connections {
            connection.close_with("serverShutdown");
        }
    }

    /// Reads the first message to decide what kind of connection this is: a client session
    /// (`hello`) or an asset download channel (`assetHello`).
    async fn handle_connection(self: Arc<Self>, stream: TcpStream) {
        let (connection, mut reader) = Connection::start(stream);
        let Some(first_message) = reader.read().await else {
            connection.close();
            return;
        };
        match first_message.message_type.as_str() {
            "hello" => {
                let protocol_version = Fields(&first_message.payload).integer("serverProtocol");
                if protocol_version == 0 {
                    // A client too old to know about servers: it expects a peer host.
                    connection.close_with("serverRequiresUpdate");
                } else if protocol_version != PROTOCOL_VERSION {
                    connection.send(
                        "disconnect",
                        sorted_object([("reason", "versionMismatch".into()), ("required", PROTOCOL_VERSION.into())]),
                    );
                } else if self.is_shutting_down() {
                    connection.close_with("serverShuttingDown");
                } else {
                    let client_id = self.next_client_id.fetch_add(1, Ordering::Relaxed);
                    connection.send("serverInfo", self.server_info());
                    self.state().clients.insert(
                        client_id,
                        ClientRecord { connection: connection.clone(), user: None, room_id: None, spectating: false },
                    );
                    while let Some(message) = reader.read().await {
                        let server = self.clone();
                        let client_connection = connection.clone();
                        tokio::task::block_in_place(move || {
                            server.handle_message(client_id, &client_connection, &message);
                        });
                    }
                    tokio::task::block_in_place(|| self.drop_client(client_id));
                }
            }
            "assetHello" => {
                // TODO(Phase 2, assets): the asset download channel.
            }
            _ => {}
        }
        connection.close();
    }

    pub(crate) fn server_info(&self) -> JsonValue {
        let settings = self.settings();
        sorted_object([
            ("name", settings.server_name.into()),
            ("motd", settings.motd.into()),
            ("protocol", PROTOCOL_VERSION.into()),
            ("mode", settings.mode.into()),
            ("registrationOpen", settings.registration_open.into()),
            ("packCost", settings.pack_cost.into()),
            ("starterDeckCost", settings.starter_deck_cost.into()),
        ])
    }

    /// Removes a disconnected client from the lobby and its room.
    fn drop_client(self: &Arc<Self>, client_id: ClientId) {
        let (was_online_as, room_id) = {
            let mut state = self.state();
            let Some(client) = state.clients.get(&client_id) else { return };
            let (user, room_id) = (client.user.clone(), client.room_id.clone());
            let mut was_online_as = None;
            if let Some(user) = user
                && state.client_by_user_id.get(&user.id) == Some(&client_id)
            {
                state.client_by_user_id.remove(&user.id);
                was_online_as = Some(user.name);
            }
            (was_online_as, room_id)
        };
        if let Some(name) = &was_online_as {
            log!(self, "{name} disconnected");
        }
        if let Some(room_id) = room_id {
            self.leave_room(client_id, &room_id, false);
        }
        // TODO(Phase 2, trading): leave any trade.
        self.state().clients.remove(&client_id);
        if was_online_as.is_some() {
            self.broadcast_users();
            self.shutdown_if_empty();
        }
    }

    /// The connections of every signed-in client.
    pub(crate) fn signed_in_connections(&self) -> Vec<Connection> {
        let state = self.state();
        state.client_by_user_id.values().filter_map(|id| state.clients.get(id)).map(|c| c.connection.clone()).collect()
    }

    pub(crate) fn broadcast_to_signed_in(&self, message_type: &str, payload: JsonValue) {
        let line = encode_message(message_type, &payload);
        for connection in self.signed_in_connections() {
            connection.send_line(line.clone());
        }
    }

    // -----------------------------------------------------------------------
    // Dispatch
    // -----------------------------------------------------------------------

    fn handle_message(self: &Arc<Self>, client_id: ClientId, connection: &Connection, message: &IncomingMessage) {
        let request = Request {
            server: self,
            client_id,
            connection,
            message,
            fields: Fields(&message.payload),
            rid: message.field("rid").cloned(),
        };
        let message_type = message.message_type.as_str();
        if request.handle_public(message_type) {
            return;
        }
        let Some(user) = request.current_user() else {
            if request.rid.is_some() {
                request.fail("notLoggedIn", "Sign in first.");
            }
            return;
        };
        if request.handle_signed_in(message_type, &user) {
            return;
        }
        if ADMIN_MESSAGE_TYPES.contains(&message_type) {
            if !user.is_admin {
                request.fail("notAdmin", "Only admins can do that.");
                return;
            }
            request.handle_admin(message_type, &user);
            return;
        }
        // Anything else is game traffic for the room the client is playing in.
        self.forward_to_room(client_id, message);
    }
}

/// Every admin-only message (the Go server's `adminHandlers`).
const ADMIN_MESSAGE_TYPES: [&str; 14] = [
    "adminUsers",
    "adminUserUpdate",
    "adminUserDelete",
    "adminKick",
    "adminSettingsGet",
    "adminSettingsSet",
    "adminRescanGames",
    "adminCloseRoom",
    "adminClearCollection",
    "adminFileList",
    "adminFileDelete",
    "adminGameDelete",
    "adminGameInstall",
    "adminShutdown",
];

fn is_transient_accept_error(error: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    matches!(
        error.kind(),
        ErrorKind::ConnectionAborted | ErrorKind::ConnectionReset | ErrorKind::Interrupted | ErrorKind::TimedOut
    )
}

/// One message being handled, with the helpers handlers use to answer it.
pub(crate) struct Request<'a> {
    pub server: &'a Arc<Server>,
    pub client_id: ClientId,
    pub connection: &'a Connection,
    pub message: &'a IncomingMessage,
    pub fields: Fields<'a>,
    /// The request id to echo; `None` when the message had none (echoed as `null`).
    pub rid: Option<JsonValue>,
}

impl Request<'_> {
    fn handle_public(&self, message_type: &str) -> bool {
        match message_type {
            "register" => self.handle_register(),
            "login" => self.handle_login(),
            "resume" => self.handle_resume(),
            _ => return false,
        }
        true
    }

    fn handle_signed_in(&self, message_type: &str, user: &User) -> bool {
        match message_type {
            "logout" => self.handle_logout(),
            "profileUpdate" => self.handle_profile_update(user),
            "avatarGet" => self.handle_avatar_get(),
            "chatSend" => self.handle_chat(user),
            "gameList" => self.handle_game_list(),
            "gameManifest" => self.handle_game_manifest(),
            "roomCreate" => self.handle_room_create(user),
            "roomJoin" => self.handle_room_join(user),
            "roomSpectate" => self.handle_room_spectate(user),
            "roomLeave" => self.handle_room_leave(),
            "roomSeats" => self.handle_room_seats(user),
            "roomStart" => self.handle_room_start(user),
            "roomClose" => self.handle_room_close(user),
            "deckList" | "deckGet" | "deckSave" | "deckDelete" | "collectionGet" | "packBuy" | "starterBuy"
            | "tradeRequest" | "tradeRespond" | "tradeOffer" | "tradeConfirm" | "tradeLeave" | "tradeCollectionGet" => {
                // TODO(Phase 2): decks, shop and trading handlers.
            }
            _ => return false,
        }
        true
    }

    fn handle_admin(&self, message_type: &str, user: &User) {
        match message_type {
            "adminShutdown" => self.handle_admin_shutdown(user),
            "adminCloseRoom" => self.handle_room_close(user),
            _ => {
                // TODO(Phase 2): the remaining admin handlers.
            }
        }
    }

    /// A signed-in client's account, as of now.
    pub fn current_user(&self) -> Option<User> {
        self.server.state().user_of(self.client_id).cloned()
    }

    /// `response {rid, ok: true, ...data}`.
    pub fn reply(&self, data: JsonObject) {
        let mut data = data;
        data.insert("rid", self.rid.clone().unwrap_or(JsonValue::Null));
        data.insert("ok", true);
        self.connection.send("response", sorted(data));
    }

    pub fn reply_empty(&self) {
        self.reply(JsonObject::new());
    }

    /// `response {rid, ok: false, error: code, message}`.
    pub fn fail(&self, code: &str, message: &str) {
        self.connection.send(
            "response",
            sorted_object([
                ("rid", self.rid.clone().unwrap_or(JsonValue::Null)),
                ("ok", false.into()),
                ("error", code.into()),
                ("message", message.into()),
            ]),
        );
    }

    pub fn toast(&self, message: &str) {
        self.connection.send("toast", sorted_object([("message", message.into())]));
    }
}

// ---------------------------------------------------------------------------
// Login rate limiting
// ---------------------------------------------------------------------------

#[derive(Default)]
struct LoginFailures {
    failure_count: u32,
    blocked_until: Option<Instant>,
}

/// Blocks an IP or email for a while after five failed logins, doubling each further failure up
/// to 32 times the base 30 seconds.
#[derive(Default)]
struct LoginLimiter {
    failures_by_key: Mutex<HashMap<String, LoginFailures>>,
}

impl LoginLimiter {
    /// How long the most-blocked key must still wait.
    fn remaining_block(&self, keys: &[String]) -> Duration {
        let failures_by_key = self.failures_by_key.lock().unwrap();
        let now = Instant::now();
        keys.iter()
            .filter_map(|key| failures_by_key.get(key)?.blocked_until)
            .map(|until| until.saturating_duration_since(now))
            .max()
            .unwrap_or_default()
    }

    fn record_failure(&self, keys: &[String]) {
        let mut failures_by_key = self.failures_by_key.lock().unwrap();
        for key in keys {
            let failures = failures_by_key.entry(key.clone()).or_default();
            failures.failure_count += 1;
            if failures.failure_count >= 5 {
                let block = Duration::from_secs(30) * (1 << (failures.failure_count - 5).min(5));
                failures.blocked_until = Some(Instant::now() + block);
            }
        }
    }

    fn record_success(&self, keys: &[String]) {
        let mut failures_by_key = self.failures_by_key.lock().unwrap();
        for key in keys {
            failures_by_key.remove(key);
        }
    }
}

/// A duration rounded to whole seconds, written like Go's `time.Duration.String`
/// ("45s", "2m0s", "1h0m30s").
pub(crate) fn go_duration_text(duration: Duration) -> String {
    let total_seconds = (duration.as_millis() + 500) / 1000;
    let hours = total_seconds / 3600;
    let minutes = total_seconds % 3600 / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}h{minutes}m{seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m{seconds}s")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_are_written_like_go() {
        assert_eq!(go_duration_text(Duration::from_millis(0)), "0s");
        assert_eq!(go_duration_text(Duration::from_millis(29_400)), "29s");
        assert_eq!(go_duration_text(Duration::from_millis(29_500)), "30s");
        assert_eq!(go_duration_text(Duration::from_secs(60)), "1m0s");
        assert_eq!(go_duration_text(Duration::from_secs(960)), "16m0s");
        assert_eq!(go_duration_text(Duration::from_secs(3630)), "1h0m30s");
    }

    #[test]
    fn login_blocks_grow_after_five_failures() {
        let limiter = LoginLimiter::default();
        let keys = vec!["ip:1.2.3.4".to_string()];
        for _ in 0..4 {
            limiter.record_failure(&keys);
        }
        assert_eq!(limiter.remaining_block(&keys), Duration::ZERO);
        limiter.record_failure(&keys);
        let first_block = limiter.remaining_block(&keys);
        assert!(first_block > Duration::from_secs(29) && first_block <= Duration::from_secs(30));
        limiter.record_failure(&keys);
        assert!(limiter.remaining_block(&keys) > Duration::from_secs(59));
        for _ in 0..10 {
            limiter.record_failure(&keys);
        }
        assert!(limiter.remaining_block(&keys) <= Duration::from_secs(960));
        limiter.record_success(&keys);
        assert_eq!(limiter.remaining_block(&keys), Duration::ZERO);
    }
}
