//! Game rooms: creating, joining, spectating, seating, starting, leaving and closing, plus the
//! per-room engine that runs the game. Port of `rooms.go`, with the engine calls of
//! `internal/engine` and `room.lua` done natively by `gametable_core::host_engine`.
//!
//! A room's lobby-visible fields ([`RoomRecord`]) live in the server state. Once started, the
//! game itself ([`RoomRuntime`]) belongs to a thread of the room's own; other code hands it work
//! through [`RoomWorker::post`], as the Go server posts closures to the room goroutine.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use gametable_core::host_engine::{HostEngine, HostTransport};
use gametable_core::json::{self, JsonObject, JsonValue};
use gametable_core::models::{DeckConfig, GameDefinition, PlayerInfo, PlayerRole, build_standard_deck_cards};
use gametable_core::session::{DealOptions, Session, SessionEnvironment};
use gametable_core::state_filter::filter_state_for_recipient;

use super::{ClientId, Request, Server};
use crate::connection::{Connection, IncomingMessage, encode_engine_message, encode_message};
use crate::log;
use crate::payload::sorted_object;
use crate::store::{ChatLine, User, new_id, unix_now};

const MAX_SEATS: i64 = 8;
/// The engine's own player id; it never receives state.
const SERVER_PLAYER_ID: &str = "__server__";
const TICK_INTERVAL: Duration = Duration::from_millis(100);
const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(5);
const ROOM_INBOX_LENGTH: usize = 4096;

/// The engine id a spectator watches under. It is never a player id, so the state filter hides
/// every hand and private zone from it, even when the spectator also holds a seat in the game.
fn spectator_id(user_id: &str) -> String {
    format!("spectator:{user_id}")
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Seat {
    pub id: String,
    pub name: String,
    pub color: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoomPhase {
    Waiting,
    Playing,
}

impl RoomPhase {
    fn as_wire_text(self) -> &'static str {
        match self {
            RoomPhase::Waiting => "waiting",
            RoomPhase::Playing => "playing",
        }
    }
}

/// A room as the lobby sees it. Guarded by the server state lock.
pub(crate) struct RoomRecord {
    pub id: String,
    pub name: String,
    pub game_id: String,
    pub game_name: String,
    pub max_players: i64,
    pub collection_mode: bool,
    pub created_at: i64,
    pub owner_id: String,
    pub seats: Vec<Seat>,
    pub phase: RoomPhase,
    /// User id -> client, for seated players in the room.
    pub players: HashMap<String, ClientId>,
    /// User id -> client, for spectators.
    pub spectators: HashMap<String, ClientId>,
    pub chat: Vec<ChatLine>,
    pub worker: RoomWorker,
}

impl RoomRecord {
    pub fn is_waiting(&self) -> bool {
        self.phase == RoomPhase::Waiting
    }

    fn seat_index(&self, user_id: &str) -> Option<usize> {
        self.seats.iter().position(|seat| seat.id == user_id)
    }

    /// Everyone in the room.
    pub fn member_client_ids(&self) -> Vec<ClientId> {
        self.players.values().chain(self.spectators.values()).copied().collect()
    }

    /// The lobby's view of the room.
    fn info(&self) -> JsonValue {
        let seats = self.seats.iter().map(|seat| {
            sorted_object([
                ("id", seat.id.as_str().into()),
                ("name", seat.name.as_str().into()),
                ("color", seat.color.into()),
                ("connected", self.players.contains_key(&seat.id).into()),
            ])
        });
        sorted_object([
            ("id", self.id.as_str().into()),
            ("name", self.name.as_str().into()),
            ("gameId", self.game_id.as_str().into()),
            ("gameName", self.game_name.as_str().into()),
            ("ownerId", self.owner_id.as_str().into()),
            ("maxPlayers", self.max_players.into()),
            ("state", self.phase.as_wire_text().into()),
            ("collectionMode", self.collection_mode.into()),
            ("seats", JsonValue::Array(seats.collect())),
            ("spectators", self.spectators.len().into()),
            ("createdAt", self.created_at.into()),
        ])
    }
}

// ---------------------------------------------------------------------------
// The room thread
// ---------------------------------------------------------------------------

type RoomWork = Box<dyn FnOnce(&mut RoomRuntime) + Send>;

/// The handle for giving work to a room's thread.
pub(crate) struct RoomWorker {
    inbox: SyncSender<RoomWork>,
    stopped: Arc<AtomicBool>,
}

impl RoomWorker {
    fn spawn(server: Arc<Server>, room_id: String) -> RoomWorker {
        let (inbox, work_receiver) = std::sync::mpsc::sync_channel(ROOM_INBOX_LENGTH);
        let stopped = Arc::new(AtomicBool::new(false));
        let thread_stopped = stopped.clone();
        let runtime = RoomRuntime {
            server,
            room_id: room_id.clone(),
            game: None,
            live_players: HashMap::new(),
            live_spectators: HashMap::new(),
            empty_since: None,
        };
        let spawned = std::thread::Builder::new()
            .name(format!("room-{room_id}"))
            .spawn(move || run_room_thread(runtime, work_receiver, thread_stopped));
        if let Err(error) = spawned {
            eprintln!("room {room_id}: could not start its thread: {error}");
        }
        RoomWorker { inbox, stopped }
    }

    /// Queues work for the room's thread; dropped (and logged) if the inbox is full.
    fn post(&self, server: &Server, room_id: &str, work: impl FnOnce(&mut RoomRuntime) + Send + 'static) {
        if self.stopped.load(Ordering::SeqCst) {
            return;
        }
        if let Err(TrySendError::Full(_)) = self.inbox.try_send(Box::new(work)) {
            log!(server, "room {room_id}: inbox full, dropping work");
        }
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _ = self.inbox.try_send(Box::new(|_| {})); // wake the thread
    }
}

/// Runs posted work, ticks timers and closes the room once nobody has been at the table for the
/// idle timeout.
fn run_room_thread(mut runtime: RoomRuntime, work_receiver: Receiver<RoomWork>, stopped: Arc<AtomicBool>) {
    let mut next_tick_at = Instant::now() + TICK_INTERVAL;
    let mut next_idle_check_at = Instant::now() + IDLE_CHECK_INTERVAL;
    while !stopped.load(Ordering::SeqCst) {
        let wait = next_tick_at.min(next_idle_check_at).saturating_duration_since(Instant::now());
        match work_receiver.recv_timeout(wait) {
            Ok(work) => {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                work(&mut runtime);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let now = Instant::now();
        if now >= next_tick_at {
            runtime.tick();
            next_tick_at = now + TICK_INTERVAL;
        }
        if now >= next_idle_check_at {
            runtime.check_idle();
            next_idle_check_at = now + IDLE_CHECK_INTERVAL;
        }
    }
}

/// A connected member of a running table.
#[derive(Clone)]
pub(crate) struct LiveMember {
    pub client_id: ClientId,
    pub connection: Connection,
}

/// A started game.
struct RoomGame {
    host: HostEngine,
    /// Every seated player, in seat order (what the transport reports as its roster).
    seated_players: Vec<PlayerInfo>,
    spectator_ids: BTreeSet<String>,
    last_spectator_revision: Option<i64>,
    last_spectator_notification_count: u64,
}

/// What the room's thread owns.
pub(crate) struct RoomRuntime {
    server: Arc<Server>,
    room_id: String,
    game: Option<RoomGame>,
    /// Seated players connected to the table, by user id.
    live_players: HashMap<String, LiveMember>,
    /// Spectators, by spectator id.
    live_spectators: HashMap<String, LiveMember>,
    empty_since: Option<Instant>,
}

/// Delivers engine messages to the room's connected members.
struct RoomTransport<'a> {
    seated_players: &'a [PlayerInfo],
    live_players: &'a HashMap<String, LiveMember>,
    live_spectators: &'a HashMap<String, LiveMember>,
}

impl HostTransport for RoomTransport<'_> {
    fn roster(&self) -> Vec<PlayerInfo> {
        self.seated_players.to_vec()
    }

    fn send_to(&mut self, player_id: &str, message_type: &str, payload: JsonValue) {
        let member = self.live_players.get(player_id).or_else(|| self.live_spectators.get(player_id));
        if let Some(member) = member {
            member.connection.send_line(encode_engine_message(message_type, &payload));
        }
    }

    fn broadcast(&mut self, message_type: &str, payload: JsonValue, except_player_id: Option<&str>) {
        let line = encode_engine_message(message_type, &payload);
        for (id, member) in self.live_players.iter().chain(self.live_spectators.iter()) {
            if Some(id.as_str()) != except_player_id {
                member.connection.send_line(line.clone());
            }
        }
    }
}

/// What a room needs to start its game.
struct GameStartPlan {
    definition_bytes: Vec<u8>,
    seats: Vec<Seat>,
    /// Zone id -> built-in deck JSON, for shared zones with a `deckName`.
    shared_deck_bytes: Vec<(String, Vec<u8>)>,
    owner_id: String,
}

impl RoomGame {
    /// Deals a new game, as `room.lua`'s `R.start` does, and sends everyone their table.
    fn start(
        plan: GameStartPlan,
        runtime_members: (&HashMap<String, LiveMember>, &HashMap<String, LiveMember>),
    ) -> Result<RoomGame, String> {
        let definition_json = json::decode_bytes(&plan.definition_bytes).map_err(|error| error.to_string())?;
        let mut game = GameDefinition::from_json(&definition_json).map_err(|error| error.to_string())?;
        // As the client's game loader does: games with a standard-deck zone carry the 52 generated cards.
        if game.zones.iter().any(|zone| zone.standard_deck) {
            let existing_ids: HashSet<String> = game.cards().iter().map(|card| card.id.clone()).collect();
            let missing: Vec<_> =
                build_standard_deck_cards().into_iter().filter(|card| !existing_ids.contains(&card.id)).collect();
            game.cards_mut().extend(missing);
        }
        let seated_players: Vec<PlayerInfo> = plan
            .seats
            .iter()
            .map(|seat| PlayerInfo::new(&seat.id, &seat.name, PlayerRole::Client, seat.color as u32))
            .collect();
        let mut shared_decks = HashMap::new();
        for (zone_id, deck_bytes) in &plan.shared_deck_bytes {
            let deck_json = json::decode_bytes(deck_bytes).map_err(|error| error.to_string())?;
            shared_decks.insert(zone_id.clone(), DeckConfig::from_json(&deck_json).map_err(|error| error.to_string())?);
        }
        let no_player_decks = HashMap::new();
        let session = Session::deal_from_zones(
            DealOptions {
                game,
                players: seated_players.clone(),
                local_player_id: SERVER_PLAYER_ID.to_string(),
                deck_configs_by_player_id: Some(&no_player_decks),
                shared_deck_configs_by_zone_id: Some(&shared_decks),
                is_local_practice: false,
            },
            SessionEnvironment::system(),
        );
        let mut host = HostEngine::new(session, SERVER_PLAYER_ID, Some(plan.owner_id));
        let (live_players, live_spectators) = runtime_members;
        let mut transport = RoomTransport { seated_players: &seated_players, live_players, live_spectators };
        // Everyone already has the game from the lobby, but gameData moves a waiting client onto the table.
        let game_data = host.game().to_json();
        for player in &seated_players {
            transport.send_to(&player.id, "gameData", game_data.clone());
        }
        host.start(&mut transport);
        let last_spectator_notification_count = host.session.notification_count();
        Ok(RoomGame {
            host,
            seated_players,
            spectator_ids: BTreeSet::new(),
            last_spectator_revision: None,
            last_spectator_notification_count,
        })
    }
}

impl RoomRuntime {
    /// Runs one engine call with the room's transport, then updates spectators if the game changed.
    fn with_game(&mut self, call: impl FnOnce(&mut HostEngine, &mut RoomTransport, &[PlayerInfo])) {
        let Some(game) = &mut self.game else { return };
        let mut transport = RoomTransport {
            seated_players: &game.seated_players,
            live_players: &self.live_players,
            live_spectators: &self.live_spectators,
        };
        call(&mut game.host, &mut transport, &game.seated_players);
        // room.lua's session listener: spectators get each new revision.
        let notification_count = game.host.session.notification_count();
        if notification_count == game.last_spectator_notification_count {
            return;
        }
        game.last_spectator_notification_count = notification_count;
        let revision = game.host.session.state().revision;
        if game.last_spectator_revision == Some(revision) {
            return;
        }
        game.last_spectator_revision = Some(revision);
        for spectator_id in &game.spectator_ids {
            send_spectator_state(game, &self.live_spectators, spectator_id);
        }
    }

    fn tick(&mut self) {
        self.with_game(|host, transport, _| host.tick(transport));
    }

    /// Tells the engine which seated players are connected.
    fn sync_roster(&mut self) {
        let live_ids: HashSet<String> = self.live_players.keys().cloned().collect();
        self.with_game(|host, transport, seated_players| {
            let connected: Vec<PlayerInfo> =
                seated_players.iter().filter(|player| live_ids.contains(&player.id)).cloned().collect();
            host.on_roster(&connected, transport);
        });
    }

    fn add_spectator(&mut self, spectator_id: &str) {
        let Some(game) = &mut self.game else { return };
        game.spectator_ids.insert(spectator_id.to_string());
        if let Some(member) = self.live_spectators.get(spectator_id) {
            member.connection.send_line(encode_engine_message("gameData", &game.host.game().to_json()));
        }
        send_spectator_state(game, &self.live_spectators, spectator_id);
    }

    fn remove_spectator(&mut self, spectator_id: &str) {
        if let Some(game) = &mut self.game {
            game.spectator_ids.remove(spectator_id);
        }
    }

    fn check_idle(&mut self) {
        if self.game.is_none() {
            return;
        }
        if !self.live_players.is_empty() {
            self.empty_since = None;
            return;
        }
        let empty_since = *self.empty_since.get_or_insert_with(Instant::now);
        let idle_minutes = self.server.settings().room_idle_timeout_minutes;
        if idle_minutes > 0 && empty_since.elapsed() >= Duration::from_secs(idle_minutes as u64 * 60) {
            self.server.close_room(&self.room_id, "idle");
        }
    }
}

/// The table as a spectator sees it: every hand and private zone hidden. Like the original, it
/// uses the game's own zones, not a loaded scenario's.
fn send_spectator_state(game: &RoomGame, live_spectators: &HashMap<String, LiveMember>, spectator_id: &str) {
    let Some(member) = live_spectators.get(spectator_id) else { return };
    let visible_zone_ids: HashSet<String> =
        game.host.game().zones.iter().filter(|zone| zone.visible_to_all).map(|zone| zone.id.clone()).collect();
    let filtered = filter_state_for_recipient(game.host.session.state(), spectator_id, &visible_zone_ids);
    member.connection.send_line(encode_engine_message("fullState", &filtered.to_json()));
}

// ---------------------------------------------------------------------------
// Server-side room bookkeeping
// ---------------------------------------------------------------------------

impl Server {
    pub(crate) fn room_infos(&self) -> JsonValue {
        let state = self.state();
        JsonValue::Array(state.rooms.values().map(RoomRecord::info).collect())
    }

    /// Whether the client holds a seat in a game that has started.
    pub(crate) fn is_in_running_room(&self, client_id: ClientId) -> bool {
        let state = self.state();
        let Some(client) = state.clients.get(&client_id) else { return false };
        let room = client.room_id.as_ref().and_then(|room_id| state.rooms.get(room_id));
        room.is_some_and(|room| room.phase == RoomPhase::Playing) && !client.spectating
    }

    /// Takes a client out of a room. Before the game starts that gives up the seat; once it is
    /// running the seat is kept so the player can rejoin.
    pub(crate) fn leave_room(self: &Arc<Self>, client_id: ClientId, room_id: &str, _explicit: bool) {
        let close_it = {
            let mut state = self.state();
            let Some(client) = state.clients.get_mut(&client_id) else { return };
            if client.room_id.as_deref() != Some(room_id) {
                return;
            }
            let spectating = client.spectating;
            client.room_id = None;
            client.spectating = false;
            let Some(room) = state.rooms.get_mut(room_id) else { return };
            let member_id = room
                .players
                .iter()
                .chain(room.spectators.iter())
                .find(|(_, id)| **id == client_id)
                .map(|(user_id, _)| user_id.clone())
                .unwrap_or_default();
            let mut close_it = false;
            if spectating {
                room.spectators.remove(&member_id);
                let watcher_id = spectator_id(&member_id);
                room.worker.post(self, room_id, move |runtime| {
                    runtime.live_spectators.remove(&watcher_id);
                    runtime.remove_spectator(&watcher_id);
                });
            } else {
                room.players.remove(&member_id);
                if room.is_waiting() {
                    if let Some(index) = room.seat_index(&member_id) {
                        room.seats.remove(index);
                    }
                    if room.seats.is_empty() {
                        close_it = true;
                    } else if room.owner_id == member_id {
                        room.owner_id = room.seats[0].id.clone();
                    }
                } else {
                    room.worker.post(self, room_id, move |runtime| {
                        if runtime.live_players.get(&member_id).is_some_and(|member| member.client_id == client_id) {
                            runtime.live_players.remove(&member_id);
                            runtime.sync_roster();
                        }
                    });
                }
            }
            close_it
        };
        if close_it {
            self.close_room(room_id, "empty");
            return;
        }
        self.broadcast_rooms();
        self.broadcast_users();
    }

    /// Ends a room and sends everyone in it back to the lobby.
    pub(crate) fn close_room(&self, room_id: &str, reason: &str) {
        let (room, member_connections) = {
            let mut state = self.state();
            let Some(room) = state.rooms.remove(room_id) else { return };
            let mut member_connections = Vec::new();
            for client_id in room.member_client_ids() {
                if let Some(client) = state.clients.get_mut(&client_id) {
                    if client.room_id.as_deref() == Some(room_id) {
                        client.room_id = None;
                        client.spectating = false;
                    }
                    member_connections.push(client.connection.clone());
                }
            }
            (room, member_connections)
        };
        room.worker.stop();
        let message =
            encode_message("roomClosed", &sorted_object([("roomId", room_id.into()), ("reason", reason.into())]));
        for connection in member_connections {
            connection.send_line(message.clone());
        }
        log!(self, "room {:?} closed ({reason})", room.name);
        self.broadcast_rooms();
        self.broadcast_users();
    }

    /// Passes game traffic from a seated player to their room's engine.
    pub(crate) fn forward_to_room(self: &Arc<Self>, client_id: ClientId, message: &IncomingMessage) {
        let (room_id, game_id, user_id, collection_mode, connection) = {
            let state = self.state();
            let Some(client) = state.clients.get(&client_id) else { return };
            let Some(room) = client.room_id.as_ref().and_then(|room_id| state.rooms.get(room_id)) else { return };
            if client.spectating || room.phase != RoomPhase::Playing {
                return;
            }
            let Some(user) = &client.user else { return };
            (room.id.clone(), room.game_id.clone(), user.id.clone(), room.collection_mode, client.connection.clone())
        };
        if collection_mode {
            if is_blocked_in_collection_mode(message) {
                connection
                    .send("toast", sorted_object([("message", "That isn't available in collection mode.".into())]));
                return;
            }
            if message.message_type == "requestLoadDeckIntoZone"
                && let Some(problem) = self.check_deck_against_collection(&user_id, &game_id, &message.payload)
            {
                connection.send("toast", sorted_object([("message", problem.into())]));
                return;
            }
        }
        let message = message.clone();
        let state = self.state();
        let Some(room) = state.rooms.get(&room_id) else { return };
        room.worker.post(self, &room_id, move |runtime| {
            if runtime.game.is_none()
                || runtime.live_players.get(&user_id).is_none_or(|member| member.client_id != client_id)
            {
                return;
            }
            runtime.with_game(|host, transport, _| {
                host.handle_message(&user_id, &message.message_type, &message.payload, transport);
            });
        });
    }
}

/// A room's chat history for `roomJoin`/`roomSpectate`. Go copies it into a nil slice, so an
/// empty history is sent as `null`.
fn chat_history_json(chat: &[ChatLine]) -> JsonValue {
    if chat.is_empty() { JsonValue::Null } else { JsonValue::Array(chat.iter().map(ChatLine::to_json).collect()) }
}

/// Messages that collection-mode games refuse: they would conjure cards a player doesn't own.
/// `requestCreateCardFromLibrary` is deliberately allowed (the Card Library stays available).
fn is_blocked_in_collection_mode(message: &IncomingMessage) -> bool {
    match message.message_type.as_str() {
        "requestGeneratePack" => true,
        "requestCreateWidget" => message.field("kind").and_then(JsonValue::as_str) == Some("packGenerator"),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

impl Request<'_> {
    /// The room named by the request's `roomId`, or a `noRoom` failure.
    fn requested_room_id(&self) -> Option<String> {
        let room_id = self.fields.text("roomId");
        if self.server.state().rooms.contains_key(&room_id) {
            Some(room_id)
        } else {
            self.fail("noRoom", "That game no longer exists.");
            None
        }
    }

    pub(super) fn handle_room_create(&self, user: &User) {
        let server = self.server;
        let (game_id, game_name) = {
            let library = server.library.read().unwrap();
            match library.game(&self.fields.text("gameId")) {
                Some(game) => (game.id.clone(), game.name.clone()),
                None => {
                    drop(library);
                    self.fail("noGame", "The server doesn't have that game.");
                    return;
                }
            }
        };
        let max_players = self.fields.integer("maxPlayers");
        if !(1..=MAX_SEATS).contains(&max_players) {
            self.fail("badPlayers", &format!("Players must be between 1 and {MAX_SEATS}."));
            return;
        }
        let mut name = self.fields.text("name").trim().to_string();
        if name.is_empty() {
            name = format!("{}'s game", user.name);
        }
        if name.chars().count() > 40 {
            name = name.chars().take(40).collect();
        }
        let settings = server.settings();
        let room_id = new_id();
        let info = {
            let mut state = server.state();
            let Some(client) = state.clients.get(&self.client_id) else { return };
            if client.room_id.is_some() {
                drop(state);
                self.fail("inRoom", "Leave your current game first.");
                return;
            }
            if settings.max_rooms > 0 && state.rooms.len() as i64 >= settings.max_rooms {
                drop(state);
                self.fail("tooManyRooms", "The server has reached its game limit.");
                return;
            }
            let room = RoomRecord {
                id: room_id.clone(),
                name: name.clone(),
                game_id,
                game_name: game_name.clone(),
                max_players,
                collection_mode: settings.is_collection_mode(),
                created_at: unix_now(),
                owner_id: user.id.clone(),
                seats: vec![Seat { id: user.id.clone(), name: user.name.clone(), color: user.color }],
                phase: RoomPhase::Waiting,
                players: HashMap::from([(user.id.clone(), self.client_id)]),
                spectators: HashMap::new(),
                chat: Vec::new(),
                worker: RoomWorker::spawn(server.clone(), room_id.clone()),
            };
            let info = room.info();
            state.rooms.insert(room_id.clone(), room);
            let client = state.clients.get_mut(&self.client_id).expect("checked above");
            client.room_id = Some(room_id);
            client.spectating = false;
            info
        };
        log!(server, "{} created room {:?} ({game_name})", user.name, name);
        self.reply(JsonObject::new().with("room", info));
        server.broadcast_rooms();
        server.broadcast_users();
    }

    pub(super) fn handle_room_join(&self, user: &User) {
        let Some(room_id) = self.requested_room_id() else { return };
        let server = self.server;
        let (info, chat) = {
            let mut state = server.state();
            let Some(client) = state.clients.get(&self.client_id) else { return };
            let (client_room_id, was_spectating) = (client.room_id.clone(), client.spectating);
            if client_room_id.as_ref().is_some_and(|current| *current != room_id) {
                drop(state);
                self.fail("inRoom", "Leave your current game first.");
                return;
            }
            let Some(room) = state.rooms.get_mut(&room_id) else {
                drop(state);
                self.fail("noRoom", "That game no longer exists.");
                return;
            };
            let seat_index = room.seat_index(&user.id);
            if room.is_waiting() && seat_index.is_none() && room.seats.len() as i64 >= room.max_players {
                drop(state);
                self.fail("full", "That game is full.");
                return;
            }
            if !room.is_waiting() && seat_index.is_none() {
                drop(state);
                self.fail("started", "That game has already started. You can spectate it instead.");
                return;
            }
            if was_spectating {
                room.spectators.remove(&user.id);
                let watcher_id = spectator_id(&user.id);
                room.worker.post(server, &room_id, move |runtime| {
                    runtime.live_spectators.remove(&watcher_id);
                    runtime.remove_spectator(&watcher_id);
                });
            }
            if seat_index.is_none() {
                room.seats.push(Seat { id: user.id.clone(), name: user.name.clone(), color: user.color });
            }
            room.players.insert(user.id.clone(), self.client_id);
            let playing = room.phase == RoomPhase::Playing;
            let info = room.info();
            let chat = chat_history_json(&room.chat);
            if playing {
                // Rejoining a running game: the engine sends gameData and fullState.
                let member = LiveMember { client_id: self.client_id, connection: self.connection.clone() };
                let user_id = user.id.clone();
                room.worker.post(server, &room_id, move |runtime| {
                    runtime.live_players.insert(user_id, member);
                    runtime.sync_roster();
                });
            }
            let client = state.clients.get_mut(&self.client_id).expect("checked above");
            client.spectating = false;
            client.room_id = Some(room_id.clone());
            (info, chat)
        };
        self.reply(JsonObject::new().with("room", info).with("chat", chat));
        server.broadcast_rooms();
        server.broadcast_users();
    }

    pub(super) fn handle_room_spectate(&self, user: &User) {
        let Some(room_id) = self.requested_room_id() else { return };
        let server = self.server;
        let (info, chat) = {
            let mut state = server.state();
            let Some(client) = state.clients.get(&self.client_id) else { return };
            if client.room_id.is_some() {
                drop(state);
                self.fail("inRoom", "Leave your current game first.");
                return;
            }
            let Some(room) = state.rooms.get_mut(&room_id) else {
                drop(state);
                self.fail("noRoom", "That game no longer exists.");
                return;
            };
            room.spectators.insert(user.id.clone(), self.client_id);
            let info = room.info();
            let chat = chat_history_json(&room.chat);
            if room.phase == RoomPhase::Playing {
                let member = LiveMember { client_id: self.client_id, connection: self.connection.clone() };
                let watcher_id = spectator_id(&user.id);
                room.worker.post(server, &room_id, move |runtime| {
                    runtime.live_spectators.insert(watcher_id.clone(), member);
                    runtime.add_spectator(&watcher_id);
                });
            }
            let client = state.clients.get_mut(&self.client_id).expect("checked above");
            client.room_id = Some(room_id.clone());
            client.spectating = true;
            (info, chat)
        };
        self.reply(JsonObject::new().with("room", info).with("chat", chat));
        server.broadcast_rooms();
        server.broadcast_users();
    }

    pub(super) fn handle_room_leave(&self) {
        let room_id = self.server.state().clients.get(&self.client_id).and_then(|client| client.room_id.clone());
        if let Some(room_id) = room_id {
            self.server.leave_room(self.client_id, &room_id, true);
        }
        self.reply_empty();
    }

    /// The room the client owns and that has not started, or a failure.
    fn owned_waiting_room_id(&self, user: &User) -> Option<String> {
        let state = self.server.state();
        let client = state.clients.get(&self.client_id)?;
        let room = client.room_id.as_ref().and_then(|room_id| state.rooms.get(room_id));
        let failure = match room {
            None => Some(("noRoom", "You're not in a game.")),
            Some(_) if client.spectating => Some(("noRoom", "You're not in a game.")),
            Some(room) if room.owner_id != user.id => Some(("notOwner", "Only the game's creator can do that.")),
            Some(room) if !room.is_waiting() => Some(("started", "The game has already started.")),
            Some(room) => return Some(room.id.clone()),
        };
        drop(state);
        if let Some((code, message)) = failure {
            self.fail(code, message);
        }
        None
    }

    pub(super) fn handle_room_seats(&self, user: &User) {
        let order = self.fields.text_list("order");
        let Some(room_id) = self.owned_waiting_room_id(user) else { return };
        {
            let mut state = self.server.state();
            let Some(room) = state.rooms.get_mut(&room_id) else { return };
            let mut remaining: HashMap<String, Seat> =
                room.seats.iter().map(|seat| (seat.id.clone(), seat.clone())).collect();
            let mut ordered = Vec::new();
            for id in &order {
                if let Some(seat) = remaining.remove(id) {
                    ordered.push(seat);
                }
            }
            // Anyone the client left out keeps their place at the end.
            for seat in &room.seats {
                if remaining.contains_key(&seat.id) {
                    ordered.push(seat.clone());
                }
            }
            room.seats = ordered;
        }
        self.reply_empty();
        self.server.broadcast_rooms();
    }

    pub(super) fn handle_room_start(&self, user: &User) {
        let Some(room_id) = self.owned_waiting_room_id(user) else { return };
        let server = self.server;
        let game_id = server.state().rooms.get(&room_id).map(|room| room.game_id.clone()).unwrap_or_default();
        let (definition_bytes, shared_deck_bytes) = {
            let library = server.library.read().unwrap();
            let Some(game) = library.game(&game_id) else {
                drop(library);
                self.fail("noGame", "The server no longer has this game.");
                return;
            };
            // Shared zones with a deckName start from the game's built-in deck.
            let mut shared_deck_bytes = Vec::new();
            for zone in &game.zones {
                if zone.shared
                    && !zone.standard_deck
                    && !zone.deck_name.is_empty()
                    && let Some(bytes) = game.built_in_deck(&zone.deck_name)
                    && is_valid_json_for_go(&bytes)
                {
                    shared_deck_bytes.retain(|(zone_id, _): &(String, Vec<u8>)| *zone_id != zone.id);
                    shared_deck_bytes.push((zone.id.clone(), bytes));
                }
            }
            (game.definition_bytes.clone(), shared_deck_bytes)
        };
        let (room_name, game_name) = {
            let mut state = server.state();
            match state.rooms.get_mut(&room_id) {
                Some(room) if room.is_waiting() => room.phase = RoomPhase::Playing,
                _ => return,
            }
            let room = &state.rooms[&room_id];
            let live_players: HashMap<String, LiveMember> = room
                .players
                .iter()
                .filter_map(|(user_id, client_id)| {
                    let connection = state.clients.get(client_id)?.connection.clone();
                    Some((user_id.clone(), LiveMember { client_id: *client_id, connection }))
                })
                .collect();
            let live_spectators: HashMap<String, LiveMember> = room
                .spectators
                .iter()
                .filter_map(|(user_id, client_id)| {
                    let connection = state.clients.get(client_id)?.connection.clone();
                    Some((spectator_id(user_id), LiveMember { client_id: *client_id, connection }))
                })
                .collect();
            let plan = GameStartPlan {
                definition_bytes,
                seats: room.seats.clone(),
                shared_deck_bytes,
                owner_id: room.owner_id.clone(),
            };
            let server_for_room = server.clone();
            let room_id_for_room = room_id.clone();
            room.worker.post(server, &room_id, move |runtime| {
                runtime.live_players = live_players;
                runtime.live_spectators = live_spectators;
                match RoomGame::start(plan, (&runtime.live_players, &runtime.live_spectators)) {
                    Ok(game) => {
                        runtime.game = Some(game);
                        runtime.sync_roster();
                        let watcher_ids: Vec<String> = runtime.live_spectators.keys().cloned().collect();
                        for watcher_id in watcher_ids {
                            runtime.add_spectator(&watcher_id);
                        }
                    }
                    Err(error) => {
                        log!(server_for_room, "room {room_id_for_room} start: {error}");
                        server_for_room.close_room(&room_id_for_room, "error");
                    }
                }
            });
            (room.name.clone(), room.game_name.clone())
        };
        self.reply_empty();
        log!(server, "room {room_name:?} started ({game_name})");
        server.broadcast_rooms();
    }

    pub(super) fn handle_room_close(&self, user: &User) {
        let Some(room_id) = self.requested_room_id() else { return };
        let allowed = {
            let state = self.server.state();
            state.rooms.get(&room_id).is_some_and(|room| room.owner_id == user.id) || user.is_admin
        };
        if !allowed {
            self.fail("notOwner", "Only the game's creator can end it.");
            return;
        }
        self.reply_empty();
        self.server.close_room(&room_id, "closedByOwner");
    }
}

/// Go's `json.Valid`: like our decoder, except that a byte-order mark makes the text invalid.
fn is_valid_json_for_go(bytes: &[u8]) -> bool {
    !bytes.starts_with(b"\xEF\xBB\xBF") && json::decode_bytes(bytes).is_ok()
}
