//! Peer hosting: accepts players, runs the hello/welcome handshake, keeps the lobby roster,
//! heartbeats and kicks. Port of `src/net/host_server.lua` (`host_server.dart`).
//!
//! Instead of the original's callbacks, the server queues [`HostServerEvent`]s; once a game is
//! running its host engine drains them each frame. Before that (in the lobby) they are dropped,
//! as the original's unset callbacks did.

use std::collections::{BTreeMap, HashMap};
use std::net::{TcpListener, TcpStream};
use std::time::Instant;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use gametable_core::color::BOARD_WIDGET_COLOR_PALETTE;
use gametable_core::host_engine::HostTransport;
use gametable_core::json::{JsonObject, JsonValue};
use gametable_core::models::{PlayerInfo, PlayerRole};
use gametable_core::protocol::{DEFAULT_GAME_PORT, HEARTBEAT_TIMEOUT_SECONDS, WireMessage, encode_line};
use gametable_core::uuid::new_v4_uuid;

use super::connection::{Connection, HeartbeatAction};
use super::upnp::{MappingRequest, PortMapping, release_mapping};

/// What happened on the server since the last [`HostServer::take_events`].
#[derive(Debug, Clone, PartialEq)]
pub enum HostServerEvent {
    /// A message from a seated player (not a handshake or ping).
    Incoming { player_id: String, message: WireMessage },
    /// Someone joined or left.
    RosterChanged,
}

struct ConnectedClient {
    connection: Connection,
    name: String,
    color: u32,
}

struct PendingConnection {
    connection: Connection,
    since: Instant,
}

pub struct HostServer {
    listener: Option<TcpListener>,
    pub port: u16,
    host_player: PlayerInfo,
    pub max_players: usize,
    host_avatar_bytes: Option<Vec<u8>>,
    clients: HashMap<String, ConnectedClient>,
    join_order: Vec<String>,
    avatars: HashMap<String, Vec<u8>>,
    pending: Vec<PendingConnection>,
    seat_order: Option<Vec<String>>,
    /// While set (a game is in progress), a hello must reclaim a disconnected seat or is refused.
    reclaimable_ids: Option<Box<dyn Fn(&str) -> bool>>,
    /// Queue events for an engine (set once a game starts).
    pub collect_events: bool,
    events: Vec<HostServerEvent>,
    upnp_request: Option<MappingRequest>,
    upnp_mapping: Option<PortMapping>,
    /// `None` while asking the router, then whether the port was forwarded.
    pub upnp_mapped: Option<bool>,
    pub public_ip: Option<String>,
    /// Bumped whenever a player's avatar arrives (screens re-register avatars then).
    pub avatar_revision: u64,
    /// The Discord party this hosted session advertises.
    pub party_id: String,
    last_update_frame: Option<u64>,
    stopped: bool,
}

impl HostServer {
    /// `H:start(hostPlayer, maxPlayers, hostAvatarBytes, port)`: binds and starts the UPnP request.
    pub fn start(
        host_player: PlayerInfo,
        max_players: usize,
        host_avatar_bytes: Option<Vec<u8>>,
        port: Option<u16>,
    ) -> Result<HostServer, String> {
        let listener =
            TcpListener::bind(("0.0.0.0", port.unwrap_or(DEFAULT_GAME_PORT))).map_err(|error| match error.kind() {
                std::io::ErrorKind::AddrInUse => "address already in use".to_string(),
                _ => error.to_string(),
            })?;
        listener.set_nonblocking(true).map_err(|error| error.to_string())?;
        let port = listener.local_addr().map_err(|error| error.to_string())?.port();
        let skip_upnp = std::env::var("GAMETABLE_NO_UPNP").is_ok();
        Ok(HostServer {
            listener: Some(listener),
            port,
            host_player,
            max_players,
            host_avatar_bytes,
            clients: HashMap::new(),
            join_order: Vec::new(),
            avatars: HashMap::new(),
            pending: Vec::new(),
            seat_order: None,
            reclaimable_ids: None,
            collect_events: false,
            events: Vec::new(),
            upnp_request: (!skip_upnp).then(|| MappingRequest::start(port, "Cardtable")),
            upnp_mapping: None,
            upnp_mapped: if skip_upnp { Some(false) } else { None },
            public_ip: None,
            avatar_revision: 0,
            party_id: new_v4_uuid(),
            last_update_frame: None,
            stopped: false,
        })
    }

    pub fn host_player(&self) -> &PlayerInfo {
        &self.host_player
    }

    /// Every player's avatar bytes, the host's included.
    pub fn avatars_by_player_id(&self) -> HashMap<String, Vec<u8>> {
        let mut avatars = self.avatars.clone();
        if let Some(bytes) = &self.host_avatar_bytes {
            avatars.insert(self.host_player.id.clone(), bytes.clone());
        }
        avatars
    }

    pub fn set_seat_order(&mut self, ids: Vec<String>) {
        self.seat_order = Some(ids);
    }

    /// While a game runs: who may reconnect (a disconnected seat).
    pub fn set_reclaimable_check(&mut self, check: Option<Box<dyn Fn(&str) -> bool>>) {
        self.reclaimable_ids = check;
    }

    /// The host then everyone who joined, in seat order once seats are assigned.
    pub fn roster(&self) -> Vec<PlayerInfo> {
        let mut list = vec![self.host_player.clone()];
        for id in &self.join_order {
            if let Some(client) = self.clients.get(id) {
                list.push(PlayerInfo::new(id.clone(), client.name.clone(), PlayerRole::Client, client.color));
            }
        }
        let Some(order) = &self.seat_order else { return list };
        order.iter().filter_map(|id| list.iter().find(|player| player.id == *id).cloned()).collect()
    }

    fn client_count(&self) -> usize {
        self.clients.len()
    }

    /// A requested colour if free, else the first unused palette colour.
    fn resolve_color(&self, requested: Option<u32>) -> u32 {
        let used: Vec<u32> =
            std::iter::once(self.host_player.color).chain(self.clients.values().map(|client| client.color)).collect();
        if let Some(requested) = requested.filter(|color| !used.contains(color)) {
            return requested;
        }
        BOARD_WIDGET_COLOR_PALETTE
            .iter()
            .copied()
            .find(|color| !used.contains(color))
            .unwrap_or_else(|| requested.unwrap_or(BOARD_WIDGET_COLOR_PALETTE[0]))
    }

    fn handle_hello(&mut self, mut connection: Connection, message: &WireMessage) {
        if self.client_count() + 1 >= self.max_players {
            connection.close();
            return;
        }
        let rejoin_id = message.get("rejoinPlayerId").and_then(JsonValue::as_str).map(str::to_string);
        let reclaimed = match (&rejoin_id, &self.reclaimable_ids) {
            (Some(id), Some(check)) => check(id),
            _ => false,
        };
        if self.reclaimable_ids.is_some() && !reclaimed {
            connection.send("disconnect", JsonObject::new().with("reason", "seatUnavailable").into());
            connection.close_after_flush();
            return;
        }
        let name = message.get("name").and_then(JsonValue::as_str).unwrap_or("Player").to_string();
        let requested = message.get("color").and_then(JsonValue::as_f64).map(|color| color as u32);
        let color = self.resolve_color(requested);
        let id = if reclaimed { rejoin_id.unwrap_or_else(new_v4_uuid) } else { new_v4_uuid() };
        if let Some(bytes) = message.get("avatar").and_then(JsonValue::as_str).and_then(|text| BASE64.decode(text).ok())
        {
            self.avatars.insert(id.clone(), bytes);
            self.avatar_revision += 1;
        }
        connection.start_heartbeat();
        self.clients.insert(id.clone(), ConnectedClient { connection, name, color });
        if !self.join_order.contains(&id) {
            self.join_order.push(id.clone());
        }
        let welcome = JsonObject::new()
            .with("hostName", self.host_player.name.as_str())
            .with("playerId", id.as_str())
            .with("assignedColor", f64::from(color))
            .with("reconnected", reclaimed);
        self.send_to_player(&id, "welcome", welcome.into());
        self.broadcast_roster();
    }

    fn handle_disconnect(&mut self, id: &str) {
        let Some(mut client) = self.clients.remove(id) else { return };
        client.connection.close();
        self.join_order.retain(|joined| joined != id);
        self.broadcast_roster();
    }

    /// One pass of the network loop; runs at most once per frame however many screens ask.
    pub fn update(&mut self, frame_index: u64) {
        if self.last_update_frame == Some(frame_index) {
            return;
        }
        self.last_update_frame = Some(frame_index);
        if let Some(request) = &mut self.upnp_request
            && let Some(mapping) = request.poll()
        {
            self.upnp_request = None;
            self.upnp_mapped = Some(mapping.is_some());
            if self.stopped {
                if let Some(mapping) = mapping {
                    release_mapping(mapping);
                }
            } else {
                self.upnp_mapping = mapping;
            }
        }
        let Some(listener) = &self.listener else { return };
        // New connections must say hello before anything else.
        let mut accepted: Vec<TcpStream> = Vec::new();
        while let Ok((stream, _)) = listener.accept() {
            accepted.push(stream);
        }
        for stream in accepted {
            if self.client_count() + 1 >= self.max_players {
                drop(stream);
            } else if let Ok(connection) = Connection::wrap(stream) {
                self.pending.push(PendingConnection { connection, since: Instant::now() });
            }
        }
        let pending = std::mem::take(&mut self.pending);
        for mut pending_connection in pending {
            let (messages, _) = pending_connection.connection.poll();
            if let Some(hello) = messages.into_iter().find(|message| message.message_type == "hello") {
                self.handle_hello(pending_connection.connection, &hello);
            } else if !pending_connection.connection.closed
                && pending_connection.since.elapsed().as_secs_f64() < HEARTBEAT_TIMEOUT_SECONDS
            {
                self.pending.push(pending_connection);
            } else {
                pending_connection.connection.close();
            }
        }
        for id in self.join_order.clone() {
            let Some(client) = self.clients.get_mut(&id) else { continue };
            let (messages, got_data) = client.connection.poll();
            for message in messages {
                if message.message_type == "ping" {
                    client.connection.send("pong", JsonObject::new().into());
                } else if self.collect_events {
                    self.events.push(HostServerEvent::Incoming { player_id: id.clone(), message });
                }
            }
            let Some(client) = self.clients.get_mut(&id) else { continue };
            match client.connection.heartbeat(got_data) {
                Some(HeartbeatAction::Timeout) => self.handle_disconnect(&id),
                Some(HeartbeatAction::Ping) => client.connection.send("ping", JsonObject::new().into()),
                None => {}
            }
        }
    }

    /// Events since the last call (empty unless `collect_events`).
    pub fn take_events(&mut self) -> Vec<HostServerEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn kick(&mut self, id: &str) {
        let Some(client) = self.clients.get_mut(id) else { return };
        client.connection.send("disconnect", JsonObject::new().with("reason", "kicked").into());
        client.connection.close_after_flush();
        self.handle_disconnect(id);
    }

    /// `lobbyRosterUpdate` to everyone (avatars keyed by player id, sorted).
    pub fn broadcast_roster(&mut self) {
        if self.collect_events {
            self.events.push(HostServerEvent::RosterChanged);
        }
        let roster = self.roster();
        let avatars: BTreeMap<String, Vec<u8>> = self.avatars_by_player_id().into_iter().collect();
        let mut avatar_object = JsonObject::new();
        for (id, bytes) in avatars {
            avatar_object = avatar_object.with(id, BASE64.encode(bytes));
        }
        let players: Vec<JsonValue> = roster.iter().map(PlayerInfo::to_json).collect();
        let payload = JsonObject::new()
            .with("maxPlayers", self.max_players as f64)
            .with("players", JsonValue::Array(players))
            .with("avatars", avatar_object);
        self.broadcast_message("lobbyRosterUpdate", payload.into(), None);
    }

    pub fn send_to_player(&mut self, id: &str, message_type: &str, payload: JsonValue) {
        if let Some(client) = self.clients.get_mut(id) {
            client.connection.send(message_type, payload);
        }
    }

    /// Encodes once and sends to every client (optionally skipping one).
    pub fn broadcast_message(&mut self, message_type: &str, payload: JsonValue, except: Option<&str>) {
        let Ok(line) = encode_line(message_type, &payload) else { return };
        for (id, client) in &mut self.clients {
            if Some(id.as_str()) != except {
                client.connection.send_raw(line.clone().into_bytes());
            }
        }
    }

    /// `H:stop(reason)`: optionally tells everyone why, closes everything, releases the port.
    pub fn stop(&mut self, reason: Option<&str>) {
        if self.stopped {
            return;
        }
        if let Some(reason) = reason {
            self.broadcast_message("disconnect", JsonObject::new().with("reason", reason).into(), None);
        }
        for client in self.clients.values_mut() {
            client.connection.close_after_flush();
        }
        for pending in &mut self.pending {
            pending.connection.close();
        }
        self.clients.clear();
        self.join_order.clear();
        self.pending.clear();
        self.listener = None;
        self.stopped = true;
        if let Some(mapping) = self.upnp_mapping.take() {
            release_mapping(mapping);
        }
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped
    }
}

impl Drop for HostServer {
    fn drop(&mut self) {
        self.stop(None);
    }
}

impl HostTransport for HostServer {
    fn roster(&self) -> Vec<PlayerInfo> {
        HostServer::roster(self)
    }

    fn send_to(&mut self, player_id: &str, message_type: &str, payload: JsonValue) {
        self.send_to_player(player_id, message_type, payload);
    }

    fn broadcast(&mut self, message_type: &str, payload: JsonValue, except_player_id: Option<&str>) {
        self.broadcast_message(message_type, payload, except_player_id);
    }
}
