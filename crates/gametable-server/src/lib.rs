//! GameTable lobby server. Port of the Go `cardflux-server`: accounts, chat, rooms running the
//! shared rules engine, decks, collections and administration, over protocol v7.

pub mod connection;
pub mod games;
pub mod payload;
pub mod server;
pub mod server_log;
pub mod store;

pub use server::{PROTOCOL_VERSION, Server};
