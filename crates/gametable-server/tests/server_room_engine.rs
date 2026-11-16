//! Game rooms running the native engine. Ports of the Go server's `engine_test.go` cases that
//! are about game behaviour (the VM-pool and Lua-timeout tests have no counterpart without Lua).

mod common;

use common::*;
use gametable_core::json::JsonValue;
use gametable_server::games::STANDARD_52_GAMEDEF;

const STANDARD_52: &str = "std52";

fn standard_52_definition() -> String {
    std::str::from_utf8(STANDARD_52_GAMEDEF).unwrap().replacen(r#""id": "standard_52""#, r#""id": "std52""#, 1)
}

/// Creates a room for `game_id`, seats the `others`, and starts it. Returns the room id.
async fn start_room(owner: &mut TestClient, others: &mut [&mut TestClient], game_id: &str) -> String {
    let created = owner
        .request("roomCreate", object([("gameId", game_id.into()), ("maxPlayers", (others.len() + 1).into())]))
        .await;
    assert!(created.ok(), "{:?}", created.0);
    let room_id = created.get("room").and_then(|room| room.get("id")).and_then(JsonValue::as_str).unwrap().to_string();
    for other in others.iter_mut() {
        assert!(other.request("roomJoin", object([("roomId", room_id.as_str().into())])).await.ok());
    }
    assert!(owner.request("roomStart", empty()).await.ok());
    room_id
}

#[tokio::test(flavor = "multi_thread")]
async fn deal_draw_filter_spectate_and_previews() {
    let server = start_server().await;
    server.install_game(STANDARD_52, &standard_52_definition());
    let (mut a, registered_a) = TestClient::register(&server.address, "a@x.com", "A").await;
    let (mut b, _) = TestClient::register(&server.address, "b@x.com", "B").await;
    let (mut s, _) = TestClient::register(&server.address, "s@x.com", "S").await;
    let a_id = registered_a.user_text("id");
    let room_id = start_room(&mut a, &mut [&mut b], STANDARD_52).await;
    a.expect("gameData").await;
    assert_eq!(parse_full_state(&a.expect("fullState").await).cards.len(), 52);

    a.send("requestDrawFromZone", object([("zoneId", "deck".into()), ("count", 1.into())])).await;
    let a_view = parse_full_state(&a.latest("fullState").await.unwrap());
    let b_view = parse_full_state(&b.latest("fullState").await.unwrap());
    let mine = a_view.cards.iter().find(|card| card.zone == "hand").expect("the drawn card");
    let theirs = b_view.cards.iter().find(|card| card.zone == "hand").expect("the opponent's card");
    assert_eq!(mine.owner_id, a_id);
    assert!(!mine.definition_id.is_empty() && mine.definition_id != "__hidden__");
    assert_eq!((theirs.instance_id.as_str(), theirs.definition_id.as_str()), (mine.instance_id.as_str(), "__hidden__"));

    // Spectators get a filtered state with every hand hidden.
    assert!(s.request("roomSpectate", object([("roomId", room_id.as_str().into())])).await.ok());
    let spectator_view = parse_full_state(&s.expect("fullState").await);
    assert!(
        spectator_view.cards.iter().filter(|card| card.zone == "hand").all(|card| card.definition_id == "__hidden__")
    );

    // Preview relays go to everyone else, not back to the sender.
    let preview = object([
        ("instanceIds", JsonValue::Array(vec![mine.instance_id.as_str().into()])),
        ("fx", 0.1.into()),
        ("fy", 0.2.into()),
    ]);
    a.send("cardDragPreview", preview).await;
    b.expect("cardDragPreview").await;
    assert!(a.latest("cardDragPreview").await.is_none(), "the sender got its own preview back");
}

#[tokio::test(flavor = "multi_thread")]
async fn two_rooms_shuffle_differently() {
    let server = start_server().await;
    server.install_game(STANDARD_52, &standard_52_definition());
    let mut orders = Vec::new();
    for email in ["a@x.com", "b@x.com"] {
        let (mut player, _) = TestClient::register(&server.address, email, &email[..1].to_uppercase()).await;
        start_room(&mut player, &mut [], STANDARD_52).await;
        let view = parse_full_state(&player.expect("fullState").await);
        orders.push(view.cards.iter().map(|card| card.definition_id.clone()).collect::<Vec<_>>().join(","));
    }
    assert_ne!(orders[0], orders[1], "two rooms dealt identical shuffles");
}

#[tokio::test(flavor = "multi_thread")]
async fn large_game() {
    let server = start_server().await;
    let cards: Vec<String> = (0..3000)
        .map(|index| {
            format!(
                r#"{{"id":"c{index}","cardTitle":"Card number {index} with a long title","imagePath":"c{index}.webp","types":["Unit","Rare","Blue"],"extraFields":{{"cost":"{}","text":"Some rules text for the card goes here."}}}}"#,
                index % 9
            )
        })
        .collect();
    let definition = format!(
        r#"{{"id":"big","name":"Big","sets":[{{"id":"s","name":"S","cards":[{}]}}],"zones":[{{"id":"deck","name":"Deck","dealsBuiltDeck":true}},{{"id":"pile","name":"Pile","shared":true}}]}}"#,
        cards.join(",")
    );
    server.install_game("big", &definition);
    let (mut a, _) = TestClient::register(&server.address, "a@x.com", "A").await;
    let (mut b, _) = TestClient::register(&server.address, "b@x.com", "B").await;
    start_room(&mut a, &mut [&mut b], "big").await;
    let game_data = a.expect("gameData").await;
    assert!(game_data.line.len() >= 300_000, "gameData is only {} bytes", game_data.line.len());
    assert_eq!(parse_full_state(&b.expect("fullState").await).cards.len(), 3000, "the shared pile holds every card");
}

#[tokio::test(flavor = "multi_thread")]
async fn game_data_keeps_same_orientation_for_all() {
    let server = start_server().await;
    server.install_game(
        "oriented",
        r#"{"id":"oriented","name":"G","cards":[{"id":"c1","cardTitle":"C1"}],"zones":[{"id":"deck","name":"Deck"}],"sameOrientationForAll":true}"#,
    );
    let (mut a, _) = TestClient::register(&server.address, "a@x.com", "A").await;
    start_room(&mut a, &mut [], "oriented").await;
    let game_data = a.expect("gameData").await;
    assert!(game_data.contains(r#""sameOrientationForAll":true"#), "{}", game_data.line);
}

#[tokio::test(flavor = "multi_thread")]
async fn deeply_nested_lines_are_ignored() {
    let server = start_server().await;
    server.install_game(STANDARD_52, &standard_52_definition());
    let (mut a, _) = TestClient::register(&server.address, "a@x.com", "A").await;
    start_room(&mut a, &mut [], STANDARD_52).await;
    a.expect("fullState").await;
    let depth = 1000;
    a.send_raw(&format!(r#"{{"type":"requestDrawFromZone","payload":{}1{}}}"#, "[".repeat(depth), "]".repeat(depth)))
        .await;
    // The connection and the room keep working.
    a.send("requestDrawFromZone", object([("zoneId", "deck".into()), ("count", 1.into())])).await;
    let view = parse_full_state(&a.latest("fullState").await.expect("the room still answers"));
    assert_eq!(view.cards.iter().filter(|card| card.zone == "hand").count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_owner_loads_scenarios() {
    let server = start_server().await;
    server.install_game(
        "scen",
        r#"{"id":"scen","name":"G","cards":[{"id":"c1","cardTitle":"C1"}],
		"zones":[{"id":"deck","name":"Deck"}],
		"hasScenarios":true,
		"scenarios":[{"id":"s1","name":"Scenario One","zones":[
			{"id":"shared1","name":"Shared","shared":true,"entries":[{"definitionId":"c1","quantity":1}]}]}]}"#,
    );
    let (mut a, _) = TestClient::register(&server.address, "a@x.com", "A").await;
    let (mut b, _) = TestClient::register(&server.address, "b@x.com", "B").await;
    start_room(&mut a, &mut [&mut b], "scen").await;
    a.expect("fullState").await;
    b.expect("fullState").await;
    let load = || object([("scenarioId", "s1".into())]);
    let has_shared_card = |message: Option<Message>| {
        message.is_some_and(|message| parse_full_state(&message).cards.iter().any(|card| card.zone_id == "shared1"))
    };

    b.send("requestLoadScenario", load()).await;
    assert!(!has_shared_card(a.latest("fullState").await), "a non-owner loaded the scenario");
    a.send("requestLoadScenario", load()).await;
    assert!(has_shared_card(b.latest("fullState").await), "the owner's scenario was not dealt");
}
