//! A headless peer for checking multiplayer interop with the original CardTable (or another
//! GameTable). UPnP is skipped.
//!
//! `interop_peer host <port>`: hosts a 2-player Standard 52-Card Deck game, waits for the joined
//! player to hold two cards in hand, prints the result and exits.
//!
//! `interop_peer join <port>`: joins 127.0.0.1:<port>, waits for the game and state, asks to draw
//! two cards from the shared deck, waits until they are in this player's hand and exits.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gametable::data::game_loader::load_standard_deck;
use gametable::net::game_client::{ClientStatus, GameClient};
use gametable::net::host_server::HostServer;
use gametable::table::TableBackend;
use gametable::table::actions::{TableAction, request_message};
use gametable::table::network::NetworkHostBackend;
use gametable_core::host_engine::HostEngine;
use gametable_core::json::JsonValue;
use gametable_core::models::{CardZone, GameDefinition, PlayerInfo, PlayerRole, TableState};
use gametable_core::session::{DealOptions, Session, SessionEnvironment};

const TIMEOUT: Duration = Duration::from_secs(60);
const FRAME: Duration = Duration::from_millis(16);

fn hand_count(state: &TableState, owner: &str) -> usize {
    state.cards().iter().filter(|card| card.zone == CardZone::Hand && card.owner_id.as_deref() == Some(owner)).count()
}

fn host(port: u16) -> Result<(), String> {
    let host = PlayerInfo::new("rust-host", "RustHost", PlayerRole::Host, 0xFF45_5A64);
    let server = Rc::new(RefCell::new(HostServer::start(host, 2, None, Some(port))?));
    println!("hosting on port {}", server.borrow().port);
    let deadline = Instant::now() + TIMEOUT;
    let mut frame_index = 0;
    while server.borrow().roster().len() < 2 {
        frame_index += 1;
        server.borrow_mut().update(frame_index);
        if Instant::now() > deadline {
            return Err("nobody joined".to_string());
        }
        std::thread::sleep(FRAME);
    }
    let roster = server.borrow().roster();
    let joiner = roster[1].clone();
    println!("joined: {} ({}), color {:08X}", joiner.name, joiner.id, joiner.color);
    println!("joiner avatar bytes: {}", server.borrow().avatars_by_player_id().get(&joiner.id).map_or(0, Vec::len));
    let game = load_standard_deck();
    server.borrow_mut().broadcast_message("gameData", game.to_json(), None);
    let empty = HashMap::new();
    let options = DealOptions {
        game,
        players: roster,
        local_player_id: "rust-host".to_string(),
        deck_configs_by_player_id: Some(&empty),
        shared_deck_configs_by_zone_id: Some(&empty),
        is_local_practice: false,
    };
    let session = Session::deal_from_zones(options, SessionEnvironment::system());
    let mut backend = NetworkHostBackend::new(HostEngine::new(session, "rust-host", None), server.clone());
    loop {
        frame_index += 1;
        backend.update(frame_index);
        let state = backend.session().state();
        if hand_count(state, &joiner.id) == 2 {
            println!("OK: the joined player drew 2 cards (revision {})", state.revision);
            for entry in &state.log {
                println!("log: {}", entry.message);
            }
            break;
        }
        if Instant::now() > deadline {
            return Err("the joined player never drew".to_string());
        }
        std::thread::sleep(FRAME);
    }
    // Give the last state a moment to reach the player, then end the game.
    for _ in 0..30 {
        frame_index += 1;
        backend.update(frame_index);
        std::thread::sleep(FRAME);
    }
    backend.dispose();
    server.borrow_mut().stop(Some("hostLeft"));
    Ok(())
}

fn join(port: u16) -> Result<(), String> {
    let mut client = GameClient::new();
    client.connect("127.0.0.1", port, "RustJoiner", 0xFFE9_1E63, Some(vec![0x89, b'P', b'N', b'G']), None);
    let deadline = Instant::now() + TIMEOUT;
    let mut game: Option<GameDefinition> = None;
    let mut state: Option<TableState> = None;
    let mut requested = false;
    loop {
        client.update();
        for message in client.take_messages() {
            match message.message_type.as_str() {
                "lobbyRosterUpdate" => {
                    let names: Vec<String> = message
                        .payload
                        .get("players")
                        .and_then(JsonValue::as_array)
                        .map(|players| {
                            players
                                .iter()
                                .filter_map(|player| player.get("name")?.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    let avatar_count = message
                        .payload
                        .get("avatars")
                        .and_then(JsonValue::as_object)
                        .map_or(0, |avatars| avatars.len());
                    println!("roster: {names:?}, {avatar_count} avatar(s)");
                }
                "gameData" => {
                    let definition =
                        GameDefinition::from_json(&message.payload).map_err(|error| format!("{error:?}"))?;
                    println!("gameData: {} ({} cards)", definition.name, definition.cards().len());
                    game = Some(definition);
                }
                "fullState" => state = TableState::from_json(&message.payload).ok(),
                other => println!("message: {other}"),
            }
        }
        match client.status {
            ClientStatus::Failed => return Err(client.error_message.clone().unwrap_or_default()),
            ClientStatus::Disconnected => {
                return Err(format!("disconnected: {:?}", client.disconnect_reason));
            }
            _ => {}
        }
        let me = client.assigned_player_id.clone().unwrap_or_default();
        if let (Some(game), Some(state)) = (&game, &state) {
            if !requested {
                requested = true;
                println!(
                    "welcome: host {:?}, id {me}, {} cards on the table",
                    client.opponent_name,
                    state.cards().len()
                );
                let zone_id = game.zones.iter().find(|zone| zone.shared).ok_or("no shared zone")?.id.clone();
                let (message_type, payload) = request_message(&TableAction::DrawFromZone { zone_id, count: 2 });
                client.send(message_type, payload);
            } else if hand_count(state, &me) == 2 {
                println!("OK: drew 2 cards (revision {})", state.revision);
                for entry in &state.log {
                    println!("log: {}", entry.message);
                }
                break;
            }
        }
        if Instant::now() > deadline {
            return Err("timed out".to_string());
        }
        std::thread::sleep(FRAME);
    }
    // Stay until the host ends the game (or a few seconds pass), to see its disconnect reason.
    let linger = Instant::now() + Duration::from_secs(10);
    while client.status != ClientStatus::Disconnected && Instant::now() < linger {
        client.update();
        client.take_messages();
        std::thread::sleep(FRAME);
    }
    println!("final status {:?}, reason {:?}", client.status, client.disconnect_reason);
    client.disconnect();
    Ok(())
}

fn main() {
    // SAFETY: set before any other thread starts.
    unsafe { std::env::set_var("GAMETABLE_NO_UPNP", "1") };
    let arguments: Vec<String> = std::env::args().collect();
    let port = arguments.get(2).and_then(|text| text.parse().ok()).unwrap_or(51240);
    let result = match arguments.get(1).map(String::as_str) {
        Some("host") => host(port),
        Some("join") => join(port),
        _ => Err("usage: interop_peer host|join <port>".to_string()),
    };
    if let Err(error) = result {
        eprintln!("FAILED: {error}");
        std::process::exit(1);
    }
}
