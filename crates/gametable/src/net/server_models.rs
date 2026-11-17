//! What a GameTable server sends to the lobby, read leniently from its JSON (missing fields take
//! defaults, as the original's plain Lua tables did).

use std::collections::HashMap;

use gametable_core::json::JsonValue;

fn text(value: &JsonValue, key: &str) -> String {
    value.get(key).and_then(JsonValue::as_str).unwrap_or("").to_string()
}

fn number(value: &JsonValue, key: &str) -> f64 {
    value.get(key).and_then(JsonValue::as_f64).unwrap_or(0.0)
}

fn flag(value: &JsonValue, key: &str) -> bool {
    value.get(key).and_then(JsonValue::as_bool).unwrap_or(false)
}

fn list<T>(value: &JsonValue, key: &str, parse: impl Fn(&JsonValue) -> T) -> Vec<T> {
    value.get(key).and_then(JsonValue::as_array).map(|items| items.iter().map(parse).collect()).unwrap_or_default()
}

/// `card id -> copies` maps (collections, trade offers).
pub fn quantities(value: Option<&JsonValue>) -> HashMap<String, i64> {
    value
        .and_then(JsonValue::as_object)
        .map(|object| object.iter().map(|(id, count)| (id.clone(), count.as_f64().unwrap_or(0.0) as i64)).collect())
        .unwrap_or_default()
}

/// The server's `serverInfo`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerInfo {
    pub name: Option<String>,
    pub motd: String,
    pub mode: String,
    pub registration_open: bool,
    pub pack_cost: Option<i64>,
    pub starter_deck_cost: Option<i64>,
}

impl ServerInfo {
    pub fn parse(value: &JsonValue) -> ServerInfo {
        ServerInfo {
            name: value.get("name").and_then(JsonValue::as_str).map(str::to_string),
            motd: text(value, "motd"),
            mode: text(value, "mode"),
            registration_open: value.get("registrationOpen").and_then(JsonValue::as_bool) != Some(false),
            pack_cost: value.get("packCost").and_then(JsonValue::as_f64).map(|cost| cost as i64),
            starter_deck_cost: value.get("starterDeckCost").and_then(JsonValue::as_f64).map(|cost| cost as i64),
        }
    }

    /// The server's name, or "GameTable Server".
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or("GameTable Server")
    }
}

/// A user as the lobby lists them (and the signed-in user's own record).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerUser {
    pub id: String,
    pub name: String,
    pub email: String,
    pub color: u32,
    pub avatar_hash: String,
    pub is_admin: bool,
    pub tokens: i64,
    /// The room they're in, or empty.
    pub room_id: String,
    pub spectating: bool,
    /// Admin listing only.
    pub disabled: bool,
    pub online: bool,
}

impl ServerUser {
    pub fn parse(value: &JsonValue) -> ServerUser {
        ServerUser {
            id: text(value, "id"),
            name: text(value, "name"),
            email: text(value, "email"),
            color: value.get("color").and_then(JsonValue::as_f64).map_or(0xFF45_5A64, |color| color as u32),
            avatar_hash: text(value, "avatarHash"),
            is_admin: flag(value, "isAdmin"),
            tokens: number(value, "tokens") as i64,
            room_id: text(value, "roomId"),
            spectating: flag(value, "spectating"),
            disabled: flag(value, "disabled"),
            online: flag(value, "online"),
        }
    }
}

/// One seat of a room.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoomSeat {
    pub id: String,
    pub name: String,
    pub color: u32,
    pub connected: bool,
}

/// A game (room) on the server.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerRoom {
    pub id: String,
    pub name: String,
    pub game_id: String,
    pub game_name: String,
    pub owner_id: String,
    pub max_players: usize,
    /// "waiting" or "playing".
    pub state: String,
    pub collection_mode: bool,
    pub seats: Vec<RoomSeat>,
    pub spectators: usize,
    pub created_at: f64,
}

impl ServerRoom {
    pub fn parse(value: &JsonValue) -> ServerRoom {
        ServerRoom {
            id: text(value, "id"),
            name: text(value, "name"),
            game_id: text(value, "gameId"),
            game_name: text(value, "gameName"),
            owner_id: text(value, "ownerId"),
            max_players: number(value, "maxPlayers") as usize,
            state: text(value, "state"),
            collection_mode: flag(value, "collectionMode"),
            seats: list(value, "seats", |seat| RoomSeat {
                id: text(seat, "id"),
                name: text(seat, "name"),
                color: number(seat, "color") as u32,
                connected: flag(seat, "connected"),
            }),
            spectators: number(value, "spectators") as usize,
            created_at: number(value, "createdAt"),
        }
    }

    pub fn is_playing(&self) -> bool {
        self.state == "playing"
    }
}

/// A set of a server game (packs are sold per set).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerSetInfo {
    pub id: String,
    pub name: String,
    pub card_count: i64,
}

/// A starter deck of a server game.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StarterDeckInfo {
    pub name: String,
    pub card_count: i64,
}

/// A game the server offers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerGameInfo {
    pub id: String,
    pub name: String,
    pub manifest_hash: String,
    pub sets: Vec<ServerSetInfo>,
    pub starter_decks: Vec<StarterDeckInfo>,
    pub card_count: i64,
    pub removable: bool,
}

impl ServerGameInfo {
    pub fn parse(value: &JsonValue) -> ServerGameInfo {
        ServerGameInfo {
            id: text(value, "id"),
            name: text(value, "name"),
            manifest_hash: text(value, "manifestHash"),
            sets: list(value, "sets", |set| ServerSetInfo {
                id: text(set, "id"),
                name: text(set, "name"),
                card_count: number(set, "cardCount") as i64,
            }),
            starter_decks: list(value, "starterDecks", |deck| StarterDeckInfo {
                name: text(deck, "name"),
                card_count: number(deck, "cardCount") as i64,
            }),
            card_count: number(value, "cardCount") as i64,
            removable: flag(value, "removable"),
        }
    }
}

/// One chat line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerChatLine {
    /// Unix seconds.
    pub at: f64,
    pub name: String,
    pub text: String,
}

impl ServerChatLine {
    pub fn parse(value: &JsonValue) -> ServerChatLine {
        ServerChatLine { at: number(value, "at"), name: text(value, "name"), text: text(value, "text") }
    }
}

/// One side's offer in a trade.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TradeOffer {
    pub cards: HashMap<String, i64>,
    pub confirmed: bool,
}

/// A running trade.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerTrade {
    pub id: String,
    pub game_id: String,
    pub game_name: String,
    pub a: ServerUser,
    pub b: ServerUser,
    /// By user id.
    pub offers: HashMap<String, TradeOffer>,
    pub chat: Vec<ServerChatLine>,
}

impl ServerTrade {
    pub fn parse(value: &JsonValue) -> ServerTrade {
        let offers = value
            .get("offers")
            .and_then(JsonValue::as_object)
            .map(|offers| {
                offers
                    .iter()
                    .map(|(user_id, offer)| {
                        (
                            user_id.clone(),
                            TradeOffer { cards: quantities(offer.get("cards")), confirmed: flag(offer, "confirmed") },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        ServerTrade {
            id: text(value, "id"),
            game_id: text(value, "gameId"),
            game_name: text(value, "gameName"),
            a: value.get("a").map(ServerUser::parse).unwrap_or_default(),
            b: value.get("b").map(ServerUser::parse).unwrap_or_default(),
            offers,
            chat: list(value, "chat", ServerChatLine::parse),
        }
    }

    /// (me, them).
    pub fn parties(&self, my_id: &str) -> (&ServerUser, &ServerUser) {
        if self.a.id == my_id { (&self.a, &self.b) } else { (&self.b, &self.a) }
    }

    pub fn offer_of(&self, user_id: &str) -> TradeOffer {
        self.offers.get(user_id).cloned().unwrap_or_default()
    }
}

/// `os.date("%H:%M", at)` in local time.
pub fn local_time_label(unix_seconds: f64) -> String {
    use sdl3::sys::time::{SDL_DateTime, SDL_TimeToDateTime};
    let mut date_time = SDL_DateTime::default();
    let ticks = (unix_seconds * 1e9) as i64;
    if unsafe { SDL_TimeToDateTime(ticks, &mut date_time, true) } {
        format!("{:02}:{:02}", date_time.hour, date_time.minute)
    } else {
        "--:--".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gametable_core::json::decode;

    #[test]
    fn parses_rooms_trades_and_games() {
        let room = ServerRoom::parse(&decode(
            r#"{"collectionMode":true,"createdAt":5,"gameId":"g","gameName":"G","id":"r1","maxPlayers":3,"name":"R","ownerId":"u1","seats":[{"color":4278190080,"connected":true,"id":"u1","name":"Ann"}],"spectators":2,"state":"playing"}"#,
        )
        .unwrap());
        assert!(room.is_playing() && room.collection_mode);
        assert_eq!((room.max_players, room.spectators, room.seats[0].name.as_str()), (3, 2, "Ann"));
        let trade = ServerTrade::parse(&decode(
            r#"{"a":{"id":"u1","name":"Ann"},"b":{"id":"u2","name":"Bob"},"chat":null,"gameId":"g","id":"t","offers":{"u1":{"cards":{"c1":2},"confirmed":true},"u2":{"cards":{},"confirmed":false}}}"#,
        )
        .unwrap());
        let (me, them) = trade.parties("u2");
        assert_eq!((me.name.as_str(), them.name.as_str()), ("Bob", "Ann"));
        assert_eq!(trade.offer_of("u1").cards["c1"], 2);
        assert!(trade.offer_of("u1").confirmed && trade.chat.is_empty());
        let game = ServerGameInfo::parse(&decode(
            r#"{"cardCount":10,"id":"g","name":"G","sets":[{"cardCount":4,"id":"s","name":"S"}],"starterDecks":[{"cardCount":40,"name":"Red"}]}"#,
        )
        .unwrap());
        assert_eq!((game.sets[0].card_count, game.starter_decks[0].name.as_str()), (4, "Red"));
        let info = ServerInfo::parse(&decode(r#"{"motd":"hi","mode":"collection"}"#).unwrap());
        assert!(info.registration_open && info.display_name() == "GameTable Server");
    }
}
