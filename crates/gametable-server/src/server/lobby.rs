//! The lobby: its snapshot for newly signed-in clients, online users, the game list, chat and the
//! periodic token allowance. Port of `lobby.go`.

use std::sync::Arc;
use std::time::Duration;

use rand::RngExt;

use gametable_core::json::{JsonObject, JsonValue};

use super::auth::public_self;
use super::{Request, Server};
use crate::connection::encode_message;
use crate::log;
use crate::payload::{empty_object, sorted_object};
use crate::store::{ChatLine, User, unix_now};

/// Longest chat message, in characters; longer ones are cut.
const MAX_CHAT_CHARACTERS: usize = 500;
/// Room chat lines kept for players who join later.
const ROOM_CHAT_HISTORY: usize = 100;

impl Server {
    /// Everything a client needs on entering the lobby.
    pub(crate) fn lobby_state(&self, user: &User) -> JsonValue {
        let settings = self.settings();
        let chat = self.store().recent_chat(settings.chat_history);
        let chat = match chat {
            Ok(lines) => JsonValue::Array(lines.iter().map(ChatLine::to_json).collect()),
            Err(_) => JsonValue::Null,
        };
        sorted_object([
            ("server", self.server_info()),
            ("self", public_self(user)),
            ("users", self.online_users()),
            ("rooms", self.room_infos()),
            ("games", self.game_infos()),
            ("chat", chat),
        ])
    }

    /// Signed-in users, sorted by name ignoring case.
    pub(crate) fn online_users(&self) -> JsonValue {
        let state = self.state();
        let mut users: Vec<(String, JsonValue)> = state
            .client_by_user_id
            .values()
            .filter_map(|client_id| state.clients.get(client_id))
            .filter_map(|client| {
                let user = client.user.as_ref()?;
                let entry = sorted_object([
                    ("id", user.id.as_str().into()),
                    ("name", user.name.as_str().into()),
                    ("color", user.color.into()),
                    ("avatarHash", user.avatar_hash.as_str().into()),
                    ("isAdmin", user.is_admin.into()),
                    ("roomId", client.room_id.clone().unwrap_or_default().into()),
                    ("spectating", client.spectating.into()),
                ]);
                Some((user.name.to_lowercase(), entry))
            })
            .collect();
        users.sort_by(|a, b| a.0.cmp(&b.0));
        JsonValue::Array(users.into_iter().map(|(_, entry)| entry).collect())
    }

    pub(crate) fn broadcast_users(&self) {
        self.broadcast_to_signed_in("users", sorted_object([("users", self.online_users())]));
    }

    pub(crate) fn broadcast_rooms(&self) {
        self.broadcast_to_signed_in("rooms", sorted_object([("rooms", self.room_infos())]));
    }

    pub(crate) fn broadcast_server_info(&self) {
        self.broadcast_to_signed_in("serverInfo", self.server_info());
    }

    /// The lobby's view of every game.
    pub(crate) fn game_infos(&self) -> JsonValue {
        let library = self.library.read().unwrap();
        let games = library.games().map(|game| {
            let sets = game.sets.iter().map(|set| {
                let collectible_count = game.set_cards(&set.id).iter().filter(|card| card.is_collectible()).count();
                sorted_object([
                    ("id", set.id.as_str().into()),
                    ("name", set.name.as_str().into()),
                    ("cardCount", collectible_count.into()),
                ])
            });
            let starter_decks = game.starter_decks.iter().map(|deck| {
                let card_count: i64 = deck.card_quantities.values().sum();
                sorted_object([("name", deck.name.as_str().into()), ("cardCount", card_count.into())])
            });
            sorted_object([
                ("id", game.id.as_str().into()),
                ("name", game.name.as_str().into()),
                ("manifestHash", game.manifest_hash.as_str().into()),
                ("sets", JsonValue::Array(sets.collect())),
                ("starterDecks", JsonValue::Array(starter_decks.collect())),
                ("cardCount", game.cards.len().into()),
                ("removable", game.is_removable().into()),
            ])
        });
        JsonValue::Array(games.collect())
    }

    /// Records a new balance for an online user and tells them.
    pub(crate) fn set_online_tokens(&self, user_id: &str, balance: i64) {
        let connection = {
            let mut state = self.state();
            let client_id = state.client_by_user_id.get(user_id).copied();
            client_id.and_then(|id| state.clients.get_mut(&id)).map(|client| {
                if let Some(user) = &mut client.user {
                    user.tokens = balance;
                }
                client.connection.clone()
            })
        };
        if let Some(connection) = connection {
            connection.send("tokens", sorted_object([("tokens", balance.into())]));
        }
    }
}

/// Credits periodic allowances to players who stay online across a day or week boundary.
pub(super) async fn run_allowance_loop(server: Arc<Server>) {
    let mut quit = server.quit.subscribe();
    let period = Duration::from_secs(5 * 60);
    let mut timer = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    loop {
        tokio::select! {
            _ = crate::connection::wait_until_set(&mut quit) => return,
            _ = timer.tick() => {
                let settings = server.settings();
                if !settings.is_collection_mode() || settings.allowance_period == "none" || settings.allowance_amount <= 0 {
                    continue;
                }
                let server = server.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let online_user_ids: Vec<String> = server.state().client_by_user_id.keys().cloned().collect();
                    for user_id in online_user_ids {
                        if let Ok((balance, true)) = server.grant_allowance_now(&user_id, &settings) {
                            server.set_online_tokens(&user_id, balance);
                        }
                    }
                })
                .await;
            }
        }
    }
}

/// Accepts "d6", "D20" or a bare "6"; refuses anything below 2 or above 100000.
fn parse_die_size(argument: &str) -> Option<i64> {
    let lowered = argument.trim().to_lowercase();
    let digits = lowered.strip_prefix('d').unwrap_or(&lowered);
    let sides: i64 = digits.parse().ok()?;
    (2..=100_000).contains(&sides).then_some(sides)
}

impl Request<'_> {
    pub(super) fn handle_game_list(&self) {
        self.reply(JsonObject::new().with("games", self.server.game_infos()));
    }

    pub(super) fn handle_game_manifest(&self) {
        let game_id = self.fields.text("gameId");
        let library = self.server.library.read().unwrap();
        let Some(game) = library.game(&game_id) else {
            drop(library);
            self.fail("noGame", "The server doesn't have that game.");
            return;
        };
        let files = game.files.iter().map(|file| {
            JsonValue::Object(
                JsonObject::new()
                    .with("path", file.path.as_str())
                    .with("sha256", file.sha256.as_str())
                    .with("size", file.size as f64),
            )
        });
        let data = JsonObject::new()
            .with("gameId", game.id.as_str())
            .with("name", game.name.as_str())
            .with("manifestHash", game.manifest_hash.as_str())
            .with("files", JsonValue::Array(files.collect()));
        drop(library);
        self.reply(data);
    }

    pub(super) fn handle_chat(&self, user: &User) {
        let fields = self.fields;
        let room_id = fields.text("roomId");
        let trade_id = fields.text("tradeId");
        if !trade_id.is_empty() {
            self.handle_trade_chat(user, &fields.text("text"), &trade_id);
            return;
        }
        let mut text = fields.text("text").trim().to_string();
        if text.is_empty() {
            return;
        }
        if text.chars().count() > MAX_CHAT_CHARACTERS {
            text = text.chars().take(MAX_CHAT_CHARACTERS).collect();
        }
        let server = self.server;
        if let Some(command) = text.strip_prefix('/') {
            match self.handle_chat_command(command, user, &room_id) {
                Some(result) => text = result,
                None => return,
            }
        }
        let line = ChatLine { user_id: user.id.clone(), name: user.name.clone(), text, at: unix_now() };
        if room_id.is_empty() {
            log!(server, "[lobby chat] {}: {}", user.name, line.text);
            let chat_history = server.settings().chat_history;
            let _ = server.store().add_chat(&line, chat_history);
            server.broadcast_to_signed_in("chat", sorted_object([("line", line.to_json())]));
            return;
        }
        let (members, room_name) = {
            let mut state = server.state();
            let client_room_id = state.clients.get(&self.client_id).and_then(|client| client.room_id.clone());
            if client_room_id.as_deref() != Some(room_id.as_str()) {
                return;
            }
            let Some(room) = state.rooms.get_mut(&room_id) else { return };
            room.chat.push(line.clone());
            if room.chat.len() > ROOM_CHAT_HISTORY {
                let excess = room.chat.len() - ROOM_CHAT_HISTORY;
                room.chat.drain(..excess);
            }
            let room_name = room.name.clone();
            let member_ids = room.member_client_ids();
            let members: Vec<_> =
                member_ids.iter().filter_map(|id| state.clients.get(id)).map(|c| c.connection.clone()).collect();
            (members, room_name)
        };
        log!(server, "[room {room_name:?} chat] {}: {}", user.name, line.text);
        let message =
            encode_message("chat", &sorted_object([("line", line.to_json()), ("roomId", room_id.as_str().into())]));
        for connection in members {
            connection.send_line(message.clone());
        }
    }

    /// Turns a slash command into the text broadcast as the sender's chat line. `None` when nothing
    /// should be broadcast (an error was toasted to the sender, or the command cleared the chat).
    fn handle_chat_command(&self, command_text: &str, user: &User, room_id: &str) -> Option<String> {
        let (command, argument) = match command_text.split_once(' ') {
            Some((command, argument)) => (command.to_lowercase(), argument.trim()),
            None => (command_text.to_lowercase(), ""),
        };
        match command.as_str() {
            "clear" => {
                if !room_id.is_empty() {
                    self.toast("/clear only works in the lobby chat.");
                } else if !user.is_admin {
                    self.toast("Only admins can clear chat.");
                } else if self.server.store().clear_chat().is_err() {
                    self.toast("Couldn't clear chat.");
                } else {
                    self.server.broadcast_to_signed_in("chatCleared", empty_object());
                }
                None
            }
            "flip" => {
                let result = if rand::rng().random_range(0..2) == 1 { "Tails" } else { "Heads" };
                Some(format!("flipped: {result}"))
            }
            "roll" => match parse_die_size(argument) {
                Some(sides) => Some(format!("rolled d{sides}: {}", rand::rng().random_range(1..=sides))),
                None => {
                    self.toast("Usage: /roll d6 (try d4, d6, d8, d10, d12, d20, d100, ...)");
                    None
                }
            },
            _ => {
                self.toast(&format!("Unknown command: /{command}. Try /flip or /roll d6."));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_die_size;

    #[test]
    fn die_sizes() {
        assert_eq!(parse_die_size("d6"), Some(6));
        assert_eq!(parse_die_size(" D20 "), Some(20));
        assert_eq!(parse_die_size("100"), Some(100));
        assert_eq!(parse_die_size("+8"), Some(8));
        assert_eq!(parse_die_size("d1"), None);
        assert_eq!(parse_die_size("d100001"), None);
        assert_eq!(parse_die_size("dabc"), None);
        assert_eq!(parse_die_size("dd6"), None);
        assert_eq!(parse_die_size(""), None);
    }
}
