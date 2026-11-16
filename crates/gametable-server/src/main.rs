//! GameTable lobby server entry point. Port of the Go `cmd/cardflux-server`.
//!
//! ```text
//! gametable-server [--data ./data] [--port 51234]
//! ```
//!
//! The data directory holds `cardflux.db` (accounts, decks, collections, settings), `games/` (one
//! folder per game, same layout as the client's game library) and `server.log`. Back up by copying
//! `cardflux.db` while the server is stopped.

use std::path::PathBuf;
use std::process::ExitCode;

use gametable_server::games::{GameLibrary, STANDARD_52_GAMEDEF};
use gametable_server::server_log::ServerLog;
use gametable_server::store::Store;
use gametable_server::{PROTOCOL_VERSION, Server};

struct Options {
    data_folder: PathBuf,
    port: u16,
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options { data_folder: PathBuf::from("data"), port: gametable_core::DEFAULT_NETWORK_PORT };
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        // Go's flag package accepts -name, --name, -name=value and --name=value.
        let trimmed = argument.trim_start_matches('-');
        let (name, inline_value) = match trimmed.split_once('=') {
            Some((name, value)) => (name.to_string(), Some(value.to_string())),
            None => (trimmed.to_string(), None),
        };
        let mut value =
            || inline_value.clone().or_else(|| arguments.next()).ok_or(format!("flag needs an argument: -{name}"));
        match name.as_str() {
            "data" => options.data_folder = PathBuf::from(value()?),
            "port" => {
                let text = value()?;
                options.port = text.parse().map_err(|_| format!("invalid value {text:?} for flag -port"))?;
            }
            "h" | "help" => {
                return Err("Usage: gametable-server [--data DIR] [--port PORT]".to_string());
            }
            _ => return Err(format!("flag provided but not defined: -{name}")),
        }
    }
    Ok(options)
}

fn main() -> ExitCode {
    let options = match parse_options() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    match run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn run(options: Options) -> Result<(), String> {
    let games_folder = options.data_folder.join("games");
    std::fs::create_dir_all(&games_folder).map_err(|error| error.to_string())?;
    let log =
        ServerLog::to_stdout_and_file(&options.data_folder.join("server.log")).map_err(|error| error.to_string())?;
    log.write(&format!("GameTable server {} (protocol {PROTOCOL_VERSION})", env!("CARGO_PKG_VERSION")));

    let store = Store::open(&options.data_folder.join("cardflux.db")).map_err(|error| format!("database: {error}"))?;
    let mut library = GameLibrary::new(&games_folder, vec![("standard_52".to_string(), STANDARD_52_GAMEDEF)]);
    library.scan().map_err(|error| format!("games: {error}"))?;
    for game in library.games() {
        let starters = match game.starter_decks.len() {
            0 => "no starter decks".to_string(),
            1 => "1 starter deck".to_string(),
            count => format!("{count} starter decks"),
        };
        log.write(&format!(
            "game: {} ({}), {} cards, {} files, {starters}",
            game.name,
            game.id,
            game.cards.len(),
            game.files.len()
        ));
    }
    for error in library.scan_errors() {
        log.write(&format!("game error: {error}"));
    }

    let server = Server::new(store, library, log).map_err(|error| error.to_string())?;
    let runtime =
        tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|error| error.to_string())?;
    runtime.block_on(async move {
        let listener =
            tokio::net::TcpListener::bind(("0.0.0.0", options.port)).await.map_err(|error| error.to_string())?;
        server.log.write(&format!(
            "GameTable server (protocol {PROTOCOL_VERSION}) listening on port {}, data in {}",
            options.port,
            options.data_folder.display()
        ));
        let signal_server = server.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                signal_server.log.write("shutting down");
                signal_server.close();
            }
        });
        server.serve(listener).await.map_err(|error| error.to_string())
    })
}
