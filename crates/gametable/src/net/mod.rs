//! Networking: the wire connection, peer hosting and joining, this machine's addresses and UPnP,
//! and the CardTable server client with its game downloads and admin uploads. Ports of
//! `src/net/*.lua`, on `std` sockets with helper threads where the original used LÖVE threads.

pub mod admin_files;
pub mod asset_channel;
pub mod asset_sync;
pub mod connection;
pub mod game_client;
pub mod game_connection;
pub mod host_server;
pub mod netinfo;
pub mod plain_http;
pub mod server_client;
pub mod server_models;
pub mod upnp;
