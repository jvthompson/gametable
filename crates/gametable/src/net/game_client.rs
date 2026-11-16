//! Joining a peer host. Port of `src/net/game_client.lua` (`game_client.dart`).
//!
//! If the address turns out to be a CardTable server (it answers `hello` with `serverInfo`), the
//! status becomes [`ClientStatus::Server`] and the connection is handed to the lobby client.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use gametable_core::json::{JsonObject, JsonValue};
use gametable_core::protocol::{SERVER_PROTOCOL_VERSION, WireMessage};

use super::connection::{ConnectProgress, Connection, Connector, HeartbeatAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientStatus {
    Idle,
    Connecting,
    Connected,
    Failed,
    Disconnected,
    /// The address is a CardTable server, not a peer host.
    Server,
}

struct Hello {
    name: String,
    color: u32,
    avatar_bytes: Option<Vec<u8>>,
    rejoin_player_id: Option<String>,
}

pub struct GameClient {
    pub status: ClientStatus,
    connector: Option<Connector>,
    hello: Option<Hello>,
    pub connection: Option<Connection>,
    pub error_message: Option<String>,
    pub server_info: Option<JsonValue>,
    pub opponent_name: Option<String>,
    pub assigned_player_id: Option<String>,
    pub assigned_color: Option<u32>,
    pub reconnected: bool,
    pub disconnect_reason: Option<String>,
    /// The last of these, replayed to a game screen that subscribes later.
    pub last_lobby_roster_update: Option<WireMessage>,
    pub last_game_data: Option<WireMessage>,
    pub last_full_state: Option<WireMessage>,
    /// Messages for the game screen (everything but ping, welcome and disconnect).
    inbox: Vec<WireMessage>,
}

impl Default for GameClient {
    fn default() -> Self {
        GameClient::new()
    }
}

impl GameClient {
    pub fn new() -> GameClient {
        GameClient {
            status: ClientStatus::Idle,
            connector: None,
            hello: None,
            connection: None,
            error_message: None,
            server_info: None,
            opponent_name: None,
            assigned_player_id: None,
            assigned_color: None,
            reconnected: false,
            disconnect_reason: None,
            last_lobby_roster_update: None,
            last_game_data: None,
            last_full_state: None,
            inbox: Vec::new(),
        }
    }

    pub fn connect(
        &mut self,
        host: &str,
        port: u16,
        name: &str,
        color: u32,
        avatar_bytes: Option<Vec<u8>>,
        rejoin_player_id: Option<String>,
    ) {
        self.status = ClientStatus::Connecting;
        self.hello = Some(Hello { name: name.to_string(), color, avatar_bytes, rejoin_player_id });
        self.connector = Some(Connector::start(host, port));
    }

    /// One pass: finishes connecting, reads messages, keeps the heartbeat.
    pub fn update(&mut self) {
        if let Some(connector) = &mut self.connector {
            match connector.poll() {
                ConnectProgress::Pending => return,
                ConnectProgress::Failed(message) => {
                    self.connector = None;
                    self.error_message = Some(message);
                    self.status = ClientStatus::Failed;
                    return;
                }
                ConnectProgress::Connected(mut connection) => {
                    self.connector = None;
                    connection.start_heartbeat();
                    if let Some(hello) = &self.hello {
                        let mut payload =
                            JsonObject::new().with("name", hello.name.as_str()).with("color", f64::from(hello.color));
                        if let Some(bytes) = &hello.avatar_bytes {
                            payload = payload.with("avatar", BASE64.encode(bytes));
                        }
                        if let Some(rejoin) = &hello.rejoin_player_id {
                            payload = payload.with("rejoinPlayerId", rejoin.as_str());
                        }
                        payload = payload.with("serverProtocol", f64::from(SERVER_PROTOCOL_VERSION));
                        connection.send("hello", payload.into());
                    }
                    self.connection = Some(connection);
                }
            }
        }
        if !matches!(self.status, ClientStatus::Connecting | ClientStatus::Connected) {
            return;
        }
        let Some(connection) = &mut self.connection else { return };
        let (messages, got_data) = connection.poll();
        for message in messages {
            match message.message_type.as_str() {
                "ping" => connection.send("pong", JsonObject::new().into()),
                "serverInfo" => {
                    self.server_info = Some(message.payload);
                    self.status = ClientStatus::Server;
                    return;
                }
                "welcome" => {
                    self.opponent_name = message.get("hostName").and_then(JsonValue::as_str).map(str::to_string);
                    self.assigned_player_id = message.get("playerId").and_then(JsonValue::as_str).map(str::to_string);
                    self.assigned_color =
                        message.get("assignedColor").and_then(JsonValue::as_f64).map(|color| color as u32);
                    self.reconnected = message.get("reconnected").is_some_and(JsonValue::is_exactly_true);
                    self.status = ClientStatus::Connected;
                }
                "disconnect" => {
                    self.disconnect_reason = message.get("reason").and_then(JsonValue::as_str).map(str::to_string);
                }
                other => {
                    match other {
                        "lobbyRosterUpdate" => self.last_lobby_roster_update = Some(message.clone()),
                        "gameData" => self.last_game_data = Some(message.clone()),
                        "fullState" => self.last_full_state = Some(message.clone()),
                        _ => {}
                    }
                    self.inbox.push(message);
                }
            }
        }
        let Some(connection) = &mut self.connection else { return };
        match connection.heartbeat(got_data) {
            Some(HeartbeatAction::Timeout) => self.handle_disconnect(),
            Some(HeartbeatAction::Ping) => connection.send("ping", JsonObject::new().into()),
            None => {}
        }
    }

    /// Messages for the game since the last call.
    pub fn take_messages(&mut self) -> Vec<WireMessage> {
        std::mem::take(&mut self.inbox)
    }

    fn handle_disconnect(&mut self) {
        if self.status == ClientStatus::Disconnected {
            return;
        }
        if let Some(connection) = &mut self.connection {
            connection.close();
        }
        self.status = ClientStatus::Disconnected;
    }

    pub fn send(&mut self, message_type: &str, payload: JsonValue) {
        if let Some(connection) = &mut self.connection {
            connection.send(message_type, payload);
        }
    }

    /// Closes the connection on purpose.
    pub fn disconnect(&mut self) {
        if let Some(connection) = &mut self.connection {
            connection.close();
        }
        self.connector = None;
        self.status = ClientStatus::Disconnected;
    }
}
