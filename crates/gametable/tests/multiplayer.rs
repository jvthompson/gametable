//! A host and a joined player in one process over loopback TCP: the handshake and roster, the
//! game and state the host sends, a request from the joined player applied by the host, and the
//! kick message. Uses the bundled Standard 52-Card Deck.

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
use gametable_core::models::{CardZone, GameDefinition, PlayerInfo, PlayerRole, TableState};
use gametable_core::session::{DealOptions, Session, SessionEnvironment};

/// Pumps both ends (one "frame" each) until `done` holds, or fails after five seconds.
fn pump_until(
    server: &Rc<RefCell<HostServer>>,
    backend: &mut Option<NetworkHostBackend>,
    client: &mut GameClient,
    frame_index: &mut u64,
    mut done: impl FnMut(&mut GameClient, &HostServer) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        *frame_index += 1;
        match backend {
            Some(backend) => backend.update(*frame_index),
            None => server.borrow_mut().update(*frame_index),
        }
        client.update();
        if done(client, &server.borrow()) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting (client status {:?})", client.status);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn host_and_client_play_over_loopback() {
    // SAFETY: set before any other thread in this test binary reads the environment.
    unsafe { std::env::set_var("GAMETABLE_NO_UPNP", "1") };
    let host = PlayerInfo::new("host-id", "Hosty", PlayerRole::Host, 0xFF45_5A64);
    let server = Rc::new(RefCell::new(HostServer::start(host, 2, None, Some(0)).expect("binds an ephemeral port")));
    let port = server.borrow().port;
    let mut client = GameClient::new();
    client.connect("127.0.0.1", port, "Joiner", 0xFFE9_1E63, Some(vec![1, 2, 3]), None);
    let mut backend: Option<NetworkHostBackend> = None;
    let mut frame_index = 0;

    // Handshake: welcome, then a roster with both players and the joiner's avatar.
    pump_until(&server, &mut backend, &mut client, &mut frame_index, |client, server| {
        client.status == ClientStatus::Connected
            && server.roster().len() == 2
            && client.last_lobby_roster_update.is_some()
    });
    assert_eq!(client.opponent_name.as_deref(), Some("Hosty"));
    assert!(!client.reconnected);
    let joiner_id = client.assigned_player_id.clone().expect("the host assigns an id");
    let roster = server.borrow().roster();
    assert_eq!(roster.iter().map(|player| player.name.as_str()).collect::<Vec<_>>(), ["Hosty", "Joiner"]);
    assert_eq!(server.borrow().avatars_by_player_id().get(&joiner_id), Some(&vec![1, 2, 3]));

    // The host picks the game and deals; the joiner receives the game and the state.
    let game = load_standard_deck();
    server.borrow_mut().broadcast_message("gameData", game.to_json(), None);
    let empty = HashMap::new();
    let options = DealOptions {
        game: game.clone(),
        players: roster,
        local_player_id: "host-id".to_string(),
        deck_configs_by_player_id: Some(&empty),
        shared_deck_configs_by_zone_id: Some(&empty),
        is_local_practice: false,
    };
    let session = Session::deal_from_zones(options, SessionEnvironment::system());
    let deck_zone_id = game.zones.iter().find(|zone| zone.shared).expect("a shared deck").id.clone();
    backend = Some(NetworkHostBackend::new(HostEngine::new(session, "host-id", None), server.clone()));
    client.take_messages();
    pump_until(&server, &mut backend, &mut client, &mut frame_index, |client, _| client.last_full_state.is_some());
    let received_game = GameDefinition::from_json(&client.last_game_data.as_ref().expect("gameData").payload).unwrap();
    assert_eq!(received_game.id, game.id);
    let state = TableState::from_json(&client.last_full_state.as_ref().unwrap().payload).unwrap();
    assert_eq!(state.cards().len(), 52);

    // The joiner asks to draw two cards; the host applies it and sends back the new state.
    let (message_type, payload) = request_message(&TableAction::DrawFromZone { zone_id: deck_zone_id, count: 2 });
    client.send(message_type, payload);
    let mut latest: Option<TableState> = None;
    pump_until(&server, &mut backend, &mut client, &mut frame_index, |client, _| {
        for message in client.take_messages() {
            if message.message_type == "fullState" {
                latest = TableState::from_json(&message.payload).ok();
            }
        }
        latest
            .as_ref()
            .is_some_and(|state| state.cards().iter().filter(|card| card.zone == CardZone::Hand).count() == 2)
    });
    let host_state = backend.as_ref().unwrap().session().state();
    let hand: Vec<_> = host_state.cards().iter().filter(|card| card.zone == CardZone::Hand).collect();
    assert_eq!(hand.len(), 2);
    assert!(hand.iter().all(|card| card.owner_id.as_deref() == Some(joiner_id.as_str())));

    // Dropping out leaves the seat reclaimable: rejoining with the old id gets the same seat back.
    client.disconnect();
    let mut client = GameClient::new();
    pump_until(&server, &mut backend, &mut client, &mut frame_index, |_, server| server.roster().len() == 1);
    let seat =
        backend.as_ref().unwrap().session().state().players.iter().find(|player| player.id == joiner_id).cloned();
    assert!(seat.is_some_and(|player| !player.connected), "the seat stays, marked disconnected");
    client.connect("127.0.0.1", port, "Joiner", 0xFFE9_1E63, None, Some(joiner_id.clone()));
    pump_until(&server, &mut backend, &mut client, &mut frame_index, |client, _| {
        client.status == ClientStatus::Connected && client.last_full_state.is_some()
    });
    assert!(client.reconnected);
    assert_eq!(client.assigned_player_id.as_deref(), Some(joiner_id.as_str()));
    let state = TableState::from_json(&client.last_full_state.as_ref().unwrap().payload).unwrap();
    assert_eq!(state.cards().iter().filter(|card| card.zone == CardZone::Hand).count(), 2);

    // Kicking sends the reason and closes the connection.
    server.borrow_mut().kick(&joiner_id);
    pump_until(&server, &mut backend, &mut client, &mut frame_index, |client, _| {
        client.status == ClientStatus::Disconnected
    });
    assert_eq!(client.disconnect_reason.as_deref(), Some("kicked"));
    backend.as_mut().unwrap().dispose();
    server.borrow_mut().stop(None);
}
