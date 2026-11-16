//! Peer-to-peer networking: the wire connection, hosting, joining, this machine's addresses and
//! UPnP. Ports of `src/net/protocol.lua`, `host_server.lua`, `game_client.lua`, `netinfo.lua` and
//! `upnp.lua`, on `std` sockets with helper threads where the original used LÖVE threads.

pub mod connection;
pub mod game_client;
pub mod host_server;
pub mod netinfo;
pub mod plain_http;
pub mod upnp;
