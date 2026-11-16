//! Shared rules engine, data models, byte-exact JSON format and network protocol for GameTable.
//!
//! This crate is the Rust port of CardTable's shared Lua engine (`lib/json.lua`, `src/core/*`,
//! `src/game/session.lua` and `src/game/host_engine.lua`). Both the client and the lobby server
//! depend on it, so the rules can never drift apart.

pub mod json;

/// Version of the lobby-server wire protocol. Must match the original's `serverProtocolVersion`.
pub const SERVER_PROTOCOL_VERSION: u32 = 7;

/// TCP port used by both peer hosting and the lobby server when none is specified.
pub const DEFAULT_NETWORK_PORT: u16 = 51234;
