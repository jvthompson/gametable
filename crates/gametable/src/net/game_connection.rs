//! What the joined player's game screen talks to: a peer host connection ([`GameClient`]) or a
//! GameTable server's game channel ([`ServerGameChannel`](super::server_client::ServerGameChannel)),
//! the way the original's ClientGame took either object.

use gametable_core::json::JsonValue;
use gametable_core::protocol::WireMessage;

use crate::app::Frame;
use crate::net::game_client::{ClientStatus, GameClient};

pub trait GameConnection {
    /// Reads the network (once a frame).
    fn pump(&mut self, frame: &mut Frame);
    /// Game messages since the last call.
    fn take_messages(&mut self) -> Vec<WireMessage>;
    /// The latest roster / game / state messages, replayed to a screen that starts listening
    /// late.
    fn cached_messages(&self) -> Vec<WireMessage>;
    fn send(&mut self, message_type: &str, payload: JsonValue);
    fn is_disconnected(&self) -> bool;
    fn disconnect_reason(&self) -> Option<String>;
    /// The game is done with the connection.
    fn close(&mut self);
}

impl GameConnection for GameClient {
    fn pump(&mut self, _frame: &mut Frame) {
        self.update();
    }

    fn take_messages(&mut self) -> Vec<WireMessage> {
        GameClient::take_messages(self)
    }

    fn cached_messages(&self) -> Vec<WireMessage> {
        [&self.last_game_data, &self.last_lobby_roster_update, &self.last_full_state]
            .into_iter()
            .flatten()
            .cloned()
            .collect()
    }

    fn send(&mut self, message_type: &str, payload: JsonValue) {
        GameClient::send(self, message_type, payload);
    }

    fn is_disconnected(&self) -> bool {
        self.status == ClientStatus::Disconnected
    }

    fn disconnect_reason(&self) -> Option<String> {
        self.disconnect_reason.clone()
    }

    fn close(&mut self) {
        self.disconnect();
    }
}
