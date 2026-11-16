//! GameTable lobby server entry point. Port of the Go `cardflux-server`.

mod games;
mod store;

fn main() {
    println!(
        "GameTable server (protocol {}, default port {})",
        gametable_core::SERVER_PROTOCOL_VERSION,
        gametable_core::DEFAULT_NETWORK_PORT
    );
}
