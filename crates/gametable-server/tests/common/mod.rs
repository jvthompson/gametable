//! A test harness for the lobby server: a real server on a loopback port with a tiny game, and
//! scripted clients that speak the wire protocol. Port of the harness in the Go server's
//! `server_test.go`.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::mpsc;

use gametable_core::json::{self, JsonObject, JsonValue};
use gametable_server::games::GameLibrary;
use gametable_server::server_log::ServerLog;
use gametable_server::store::{Store, sha256_hex};
use gametable_server::{PROTOCOL_VERSION, Server};

/// One private hand-less game: a shared deck of 6 cards in one set, two rarities.
pub const MINI_GAME: &str = r#"{
  "id": "mini", "name": "Mini",
  "sets": [{"id": "s1", "name": "Set One", "packSettings": [{"tag": "Common", "count": 2}, {"tag": "Rare", "count": 1}],
    "cards": [
      {"id": "c1", "cardTitle": "One", "types": ["Common"], "imagePath": "c1.png"},
      {"id": "c2", "cardTitle": "Two", "types": ["Common"]},
      {"id": "c3", "cardTitle": "Three", "types": ["Common"]},
      {"id": "r1", "cardTitle": "Rare One", "types": ["Rare"]},
      {"id": "t1", "cardTitle": "Token", "types": ["Common"], "unownable": true},
      {"id": "l1", "cardTitle": "Land", "types": ["Common"], "isFreeCard": true}
    ]}],
  "tagGroups": [{"id": "rarity", "name": "Rarity", "tags": ["Common", "Rare"]}],
  "zones": [
    {"id": "deck", "name": "Deck", "shared": true, "entries": [{"definitionId": "c1", "quantity": 3}, {"definitionId": "c2", "quantity": 3}]},
    {"id": "mine", "name": "My Deck", "dealsBuiltDeck": true}
  ]
}"#;

/// A running test server; its data folder is removed when dropped.
pub struct TestServer {
    pub server: Arc<Server>,
    pub address: String,
    pub data_folder: PathBuf,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.server.close();
        let _ = std::fs::remove_dir_all(&self.data_folder);
    }
}

impl TestServer {
    /// Adds a game folder holding just a `gamedef.json`, then rescans the library.
    pub fn install_game(&self, game_id: &str, definition: &str) {
        let folder = self.data_folder.join("games").join(game_id);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("gamedef.json"), definition).unwrap();
        let mut library = self.server.library.write().unwrap();
        library.scan().unwrap();
        assert!(library.game(game_id).is_some(), "{game_id} did not load: {:?}", library.scan_errors());
    }
}

pub async fn start_server() -> TestServer {
    let data_folder =
        std::env::temp_dir().join(format!("gametable-server-test-{}", gametable_core::uuid::new_v4_uuid()));
    let game_folder = data_folder.join("games").join("mini");
    std::fs::create_dir_all(game_folder.join("s1")).unwrap();
    std::fs::create_dir_all(game_folder.join("savegames")).unwrap();
    std::fs::write(game_folder.join("gamedef.json"), MINI_GAME).unwrap();
    std::fs::write(game_folder.join("s1").join("c1.png"), b"\x89PNGfake-image-bytes").unwrap();
    std::fs::write(game_folder.join("savegames").join("x.json"), b"{}").unwrap();
    let store = Store::open(&data_folder.join("test.db")).unwrap();
    let mut library = GameLibrary::new(data_folder.join("games"), Vec::new());
    library.scan().unwrap();
    let server = Server::new(store, library, ServerLog::silent()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let serving = server.clone();
    tokio::spawn(async move { serving.serve(listener).await });
    TestServer { server, address, data_folder }
}

/// One received message.
#[derive(Debug, Clone)]
pub struct Message {
    pub message_type: String,
    pub payload: JsonValue,
    /// The whole line as received, for substring checks.
    pub line: String,
}

impl Message {
    pub fn contains(&self, text: &str) -> bool {
        self.line.contains(text)
    }
}

/// A `response` payload.
#[derive(Debug, Clone)]
pub struct Response(pub JsonValue);

impl Response {
    pub fn ok(&self) -> bool {
        self.0.get("ok").and_then(JsonValue::as_bool) == Some(true)
    }
    pub fn error(&self) -> &str {
        self.0.get("error").and_then(JsonValue::as_str).unwrap_or("")
    }
    pub fn text(&self, key: &str) -> String {
        self.0.get(key).and_then(JsonValue::as_str).unwrap_or("").to_string()
    }
    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.0.get(key)
    }
    /// `user.<key>` as text.
    pub fn user_text(&self, key: &str) -> String {
        self.0.get("user").and_then(|user| user.get(key)).and_then(JsonValue::as_str).unwrap_or("").to_string()
    }
    pub fn user_is_admin(&self) -> bool {
        self.0.get("user").and_then(|user| user.get("isAdmin")).and_then(JsonValue::as_bool) == Some(true)
    }
}

/// A scripted client.
pub struct TestClient {
    writer: OwnedWriteHalf,
    incoming: mpsc::UnboundedReceiver<Message>,
    /// Messages received while waiting for something else.
    seen: VecDeque<Message>,
    next_rid: i64,
}

/// Builds an object payload.
pub fn object<const N: usize>(entries: [(&str, JsonValue); N]) -> JsonValue {
    let mut object = JsonObject::new();
    for (key, value) in entries {
        object.insert(key, value);
    }
    JsonValue::Object(object)
}

pub fn texts(items: &[&str]) -> JsonValue {
    JsonValue::Array(items.iter().map(|item| JsonValue::from(*item)).collect())
}

/// The client's password hash for an email and password.
pub fn password_hash(email: &str, password: &str) -> String {
    sha256_hex(format!("cardflux:{}:{password}", email.to_lowercase()).as_bytes())
}

impl TestClient {
    pub async fn dial(address: &str) -> TestClient {
        let stream = TcpStream::connect(address).await.unwrap();
        let (read_half, writer) = stream.into_split();
        let (sender, incoming) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut lines = BufReader::with_capacity(1 << 20, read_half).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(decoded) = json::decode(&line) else { continue };
                let Some(message_type) = decoded.get("type").and_then(JsonValue::as_str) else { continue };
                if message_type == "ping" {
                    continue;
                }
                let payload = decoded.get("payload").cloned().unwrap_or(JsonValue::Null);
                let message = Message { message_type: message_type.to_string(), payload, line };
                if sender.send(message).is_err() {
                    return;
                }
            }
        });
        TestClient { writer, incoming, seen: VecDeque::new(), next_rid: 0 }
    }

    /// Dials and says hello with the current protocol.
    pub async fn connect(address: &str) -> TestClient {
        let mut client = TestClient::dial(address).await;
        client
            .send(
                "hello",
                object([("name", "x".into()), ("color", 1.into()), ("serverProtocol", PROTOCOL_VERSION.into())]),
            )
            .await;
        client.expect("serverInfo").await;
        client
    }

    /// Connects and registers an account with password "pw" and color 42.
    pub async fn register(address: &str, email: &str, name: &str) -> (TestClient, Response) {
        let mut client = TestClient::connect(address).await;
        let response = client
            .request(
                "register",
                object([
                    ("email", email.into()),
                    ("passwordHash", password_hash(email, "pw").into()),
                    ("name", name.into()),
                    ("color", 42.into()),
                ]),
            )
            .await;
        assert!(response.ok(), "register {name}: {:?}", response.0);
        client.expect("lobbyState").await;
        (client, response)
    }

    pub async fn send(&mut self, message_type: &str, payload: JsonValue) {
        let line = object([("type", message_type.into()), ("payload", payload)]);
        let mut text = json::encode_compact(&line).unwrap();
        text.push('\n');
        let _ = self.writer.write_all(text.as_bytes()).await;
    }

    /// Writes one line exactly as given (a newline is added).
    pub async fn send_raw(&mut self, line: &str) {
        let mut text = line.to_string();
        text.push('\n');
        let _ = self.writer.write_all(text.as_bytes()).await;
    }

    /// Closes the connection.
    pub async fn disconnect(&mut self) {
        let _ = self.writer.shutdown().await;
    }

    /// Waits for the next message of a type, remembering the ones skipped.
    pub async fn expect(&mut self, message_type: &str) -> Message {
        if let Some(position) = self.seen.iter().position(|message| message.message_type == message_type) {
            return self.seen.remove(position).unwrap();
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            match tokio::time::timeout_at(deadline, self.incoming.recv()).await {
                Ok(Some(message)) if message.message_type == message_type => return message,
                Ok(Some(message)) => self.seen.push_back(message),
                Ok(None) => panic!("connection closed waiting for {message_type}"),
                Err(_) => panic!("timed out waiting for {message_type} (seen {:?})", self.seen_types()),
            }
        }
    }

    /// Lets messages arrive for a moment, then returns the newest of a type.
    pub async fn latest(&mut self, message_type: &str) -> Option<Message> {
        tokio::time::sleep(Duration::from_millis(150)).await;
        while let Ok(message) = self.incoming.try_recv() {
            self.seen.push_back(message);
        }
        let newest = self.seen.iter().rev().find(|message| message.message_type == message_type).cloned();
        if newest.is_some() {
            self.seen.clear();
        }
        newest
    }

    pub fn seen_types(&self) -> Vec<String> {
        self.seen.iter().map(|message| message.message_type.clone()).collect()
    }

    /// Sends a request with a new `rid` and waits for its response.
    pub async fn request(&mut self, message_type: &str, payload: JsonValue) -> Response {
        self.next_rid += 1;
        let rid = self.next_rid;
        let mut payload = match payload {
            JsonValue::Object(object) => object,
            _ => JsonObject::new(),
        };
        payload.insert("rid", rid);
        self.send(message_type, JsonValue::Object(payload)).await;
        loop {
            let message = self.expect("response").await;
            if message.payload.get("rid").and_then(JsonValue::as_f64) == Some(rid as f64) {
                return Response(message.payload);
            }
        }
    }
}

pub fn empty() -> JsonValue {
    JsonValue::Object(JsonObject::new())
}

/// Parsed parts of a `fullState` payload.
pub struct TableView {
    pub revision: i64,
    /// `(id, connected)`.
    pub players: Vec<(String, Option<bool>)>,
    pub cards: Vec<CardView>,
}

pub struct CardView {
    pub instance_id: String,
    pub definition_id: String,
    pub zone: String,
    pub zone_id: String,
    pub owner_id: String,
}

pub fn parse_full_state(message: &Message) -> TableView {
    let text = |value: &JsonValue, key: &str| value.get(key).and_then(JsonValue::as_str).unwrap_or("").to_string();
    let payload = &message.payload;
    let list = |key: &str| payload.get(key).and_then(JsonValue::as_array).cloned().unwrap_or_default();
    TableView {
        revision: payload.get("revision").and_then(JsonValue::as_f64).unwrap_or(0.0) as i64,
        players: list("players")
            .iter()
            .map(|player| (text(player, "id"), player.get("connected").and_then(JsonValue::as_bool)))
            .collect(),
        cards: list("cards")
            .iter()
            .map(|card| CardView {
                instance_id: text(card, "instanceId"),
                definition_id: text(card, "definitionId"),
                zone: text(card, "zone"),
                zone_id: text(card, "zoneId"),
                owner_id: text(card, "ownerId"),
            })
            .collect(),
    }
}
