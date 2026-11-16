//! Accounts, chat, rooms and shutdown, driven over real connections. Ports of the matching tests
//! in the Go server's `server_test.go`.

mod common;

use std::time::Duration;

use common::*;
use gametable_core::json::JsonValue;
use gametable_server::PROTOCOL_VERSION;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn version_checks() {
    let server = start_server().await;
    let mut old_client = TestClient::dial(&server.address).await;
    old_client.send("hello", object([("name", "Old".into())])).await;
    let message = old_client.expect("disconnect").await;
    assert!(message.contains("serverRequiresUpdate"), "old client got {}", message.line);

    let mut future_client = TestClient::dial(&server.address).await;
    future_client.send("hello", object([("serverProtocol", (PROTOCOL_VERSION + 1).into())])).await;
    let message = future_client.expect("disconnect").await;
    assert!(message.contains("versionMismatch"), "future client got {}", message.line);
    assert!(message.contains(&format!("\"required\":{PROTOCOL_VERSION}")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accounts() {
    let server = start_server().await;
    let address = &server.address;
    let (mut alice, alice_response) = TestClient::register(address, "Alice@Example.com", "Alice").await;
    assert!(alice_response.user_is_admin(), "first account should be admin");
    let (_bob, bob_response) = TestClient::register(address, "bob@example.com", "Bob").await;
    assert!(!bob_response.user_is_admin(), "second account should not be admin");

    let mut client = TestClient::connect(address).await;
    let duplicate_email = client
        .request(
            "register",
            object([
                ("email", "alice@example.com".into()),
                ("passwordHash", password_hash("x", "y").into()),
                ("name", "Other".into()),
            ]),
        )
        .await;
    assert_eq!(duplicate_email.error(), "emailTaken");
    let duplicate_name = client
        .request(
            "register",
            object([
                ("email", "new@example.com".into()),
                ("passwordHash", password_hash("x", "y").into()),
                ("name", "alice".into()),
            ]),
        )
        .await;
    assert_eq!(duplicate_name.error(), "nameTaken", "names are unique ignoring case");
    let wrong_password = client
        .request(
            "login",
            object([
                ("email", "bob@example.com".into()),
                ("passwordHash", password_hash("bob@example.com", "wrong").into()),
            ]),
        )
        .await;
    assert_eq!(wrong_password.error(), "badCredentials");
    assert_eq!(client.request("adminUsers", empty()).await.error(), "notLoggedIn");

    // Token resume on a fresh connection replaces the older connection for the same account.
    let mut resumed = TestClient::connect(address).await;
    let response = resumed.request("resume", object([("token", alice_response.text("token").into())])).await;
    assert!(response.ok());
    assert_eq!(response.user_text("name"), "Alice");
    let message = alice.expect("disconnect").await;
    assert!(message.contains("loggedInElsewhere"), "old session got {}", message.line);

    // Non-admins can't use admin calls.
    let mut bob_again = TestClient::connect(address).await;
    let login = bob_again
        .request(
            "login",
            object([
                ("email", "bob@example.com".into()),
                ("passwordHash", password_hash("bob@example.com", "pw").into()),
            ]),
        )
        .await;
    assert!(login.ok());
    assert_eq!(bob_again.request("adminUsers", empty()).await.error(), "notAdmin");

    // Login is rate-limited after repeated failures.
    let mut guesser = TestClient::connect(address).await;
    let guess = || object([("email", "nobody@example.com".into()), ("passwordHash", password_hash("n", "n").into())]);
    for _ in 0..5 {
        guesser.request("login", guess()).await;
    }
    let blocked = guesser.request("login", guess()).await;
    assert_eq!(blocked.error(), "rateLimited");
    assert!(blocked.text("message").starts_with("Too many failed attempts. Try again in "));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn profile_update_and_avatars() {
    let server = start_server().await;
    let address = &server.address;
    let (mut alice, registered) = TestClient::register(address, "a@x.com", "A").await;
    let avatar_png = b"\x89PNG\r\n\x1a\nimage";
    let avatar_base64 = "iVBORw0KGgppbWFnZQ==";
    let response = alice
        .request(
            "profileUpdate",
            object([("name", "Alicia".into()), ("color", 7.into()), ("avatar", avatar_base64.into())]),
        )
        .await;
    assert!(response.ok(), "{:?}", response.0);
    assert_eq!(response.user_text("name"), "Alicia");
    let avatar_hash = response.user_text("avatarHash");
    assert_eq!(avatar_hash, gametable_server::store::avatar_hash_of(avatar_png));
    assert!(alice.expect("self").await.contains("\"name\":\"Alicia\""));

    let avatar = alice.request("avatarGet", object([("hash", avatar_hash.as_str().into())])).await;
    assert_eq!(avatar.text("data"), avatar_base64);
    assert_eq!(alice.request("avatarGet", object([("hash", "nope".into())])).await.error(), "notFound");

    // Changing the password needs the old one and issues a new token.
    let wrong_old = alice
        .request(
            "profileUpdate",
            object([
                ("passwordHash", password_hash("a@x.com", "new").into()),
                ("oldPasswordHash", password_hash("a@x.com", "bad").into()),
            ]),
        )
        .await;
    assert_eq!(wrong_old.error(), "badCredentials");
    let changed = alice
        .request(
            "profileUpdate",
            object([
                ("passwordHash", password_hash("a@x.com", "new").into()),
                ("oldPasswordHash", password_hash("a@x.com", "pw").into()),
            ]),
        )
        .await;
    assert!(changed.ok());
    alice.expect("newToken").await;
    // The old saved sign-in no longer works.
    let mut other = TestClient::connect(address).await;
    let stale = other.request("resume", object([("token", registered.text("token").into())])).await;
    assert_eq!(stale.error(), "badToken");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat() {
    let server = start_server().await;
    let address = &server.address;
    let (mut a, _) = TestClient::register(address, "a@x.com", "A").await;
    let (mut b, _) = TestClient::register(address, "b@x.com", "B").await;
    a.send("chatSend", object([("text", "  hello there  ".into())])).await;
    let message = b.expect("chat").await;
    assert!(message.contains("\"text\":\"hello there\""), "chat: {}", message.line);
    // History is replayed to newcomers.
    let (mut c, _) = (TestClient::connect(address).await, ());
    c.request(
        "register",
        object([
            ("email", "c@x.com".into()),
            ("passwordHash", password_hash("c@x.com", "pw").into()),
            ("name", "C".into()),
        ]),
    )
    .await;
    assert!(c.expect("lobbyState").await.contains("hello there"), "chat history missing from lobbyState");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_commands() {
    let server = start_server().await;
    let address = &server.address;
    let (mut a, _) = TestClient::register(address, "a@x.com", "A").await; // the first account is the admin
    let (mut b, _) = TestClient::register(address, "b@x.com", "B").await;

    b.send("chatSend", object([("text", "/flip".into())])).await;
    let message = a.expect("chat").await;
    assert!(message.contains("\"text\":\"flipped: "), "flip: {}", message.line);

    b.send("chatSend", object([("text", "/roll d6".into())])).await;
    let message = a.expect("chat").await;
    assert!(message.contains("\"text\":\"rolled d6: "), "roll: {}", message.line);

    // An invalid roll or unknown command toasts the sender only and broadcasts nothing.
    b.send("chatSend", object([("text", "/roll dabc".into())])).await;
    assert!(b.expect("toast").await.contains("Usage"));
    b.send("chatSend", object([("text", "/nope".into())])).await;
    assert!(b.expect("toast").await.contains("Unknown command"));
    b.send("chatSend", object([("text", "/clear".into())])).await;
    assert!(b.expect("toast").await.contains("Only admins"));

    // The admin's /clear empties the lobby chat for everyone, silently.
    a.send("chatSend", object([("text", "/clear".into())])).await;
    a.expect("chatCleared").await;
    b.expect("chatCleared").await;
    let mut c = TestClient::connect(address).await;
    c.request(
        "register",
        object([
            ("email", "c@x.com".into()),
            ("passwordHash", password_hash("c@x.com", "pw").into()),
            ("name", "C".into()),
        ]),
    )
    .await;
    let lobby = c.expect("lobbyState").await;
    assert!(!lobby.contains("flipped") && !lobby.contains("rolled"), "chat history should be empty after /clear");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn room_lifecycle() {
    let server = start_server().await;
    let address = &server.address;
    let (mut a, registered_a) = TestClient::register(address, "a@x.com", "A").await;
    let (mut b, registered_b) = TestClient::register(address, "b@x.com", "B").await;
    let (mut s, _) = TestClient::register(address, "s@x.com", "S").await;
    let (a_id, b_id) = (registered_a.user_text("id"), registered_b.user_text("id"));

    let created = a
        .request("roomCreate", object([("gameId", "mini".into()), ("name", "Test".into()), ("maxPlayers", 2.into())]))
        .await;
    assert!(created.ok(), "{:?}", created.0);
    let room_id = created.get("room").and_then(|room| room.get("id")).and_then(JsonValue::as_str).unwrap().to_string();
    let in_room = || object([("roomId", room_id.as_str().into())]);
    assert_eq!(b.request("roomStart", empty()).await.error(), "noRoom");
    assert!(b.request("roomJoin", in_room()).await.ok());
    assert_eq!(s.request("roomJoin", in_room()).await.error(), "full");
    assert_eq!(b.request("roomStart", empty()).await.error(), "notOwner");
    assert!(a.request("roomSeats", object([("order", texts(&[&b_id, &a_id]))])).await.ok());
    assert!(a.request("roomStart", empty()).await.ok());
    a.expect("gameData").await;
    b.expect("gameData").await;
    let table = parse_full_state(&a.expect("fullState").await);
    assert_eq!(table.players.len(), 2);
    assert_eq!(table.players[0].0, b_id, "seat order not applied");
    assert_eq!(table.cards.len(), 6);

    // A draws; B sees the card hidden.
    a.send("requestDrawFromZone", object([("zoneId", "deck".into()), ("count", 1.into())])).await;
    let a_view = parse_full_state(&a.latest("fullState").await.unwrap());
    let b_view = parse_full_state(&b.latest("fullState").await.unwrap());
    let drawn = a_view.cards.iter().find(|card| card.zone == "hand" && card.owner_id == a_id).unwrap();
    assert_ne!(drawn.definition_id, "__hidden__", "the drawer can't see their card");
    assert!(b_view.cards.iter().filter(|card| card.zone == "hand").all(|card| card.definition_id == "__hidden__"));

    // Spectating: the table with every hand hidden; actions do nothing.
    assert_eq!(s.request("roomJoin", in_room()).await.error(), "started");
    assert!(s.request("roomSpectate", in_room()).await.ok());
    s.expect("gameData").await;
    let spectator_view = parse_full_state(&s.expect("fullState").await);
    assert!(
        spectator_view.cards.iter().filter(|card| card.zone == "hand").all(|card| card.definition_id == "__hidden__")
    );
    s.send("requestDrawFromZone", object([("zoneId", "deck".into()), ("count", 1.into())])).await;
    if let Some(message) = s.latest("fullState").await {
        assert_eq!(parse_full_state(&message).revision, spectator_view.revision, "a spectator changed the game");
    }

    // A seated player who chooses to watch gets the neutral view too.
    b.request("roomLeave", empty()).await;
    assert!(b.request("roomSpectate", in_room()).await.ok());
    b.expect("gameData").await;
    a.send("requestDrawFromZone", object([("zoneId", "deck".into()), ("count", 1.into())])).await;
    let watched = parse_full_state(&b.latest("fullState").await.expect("the seated spectator got no state"));
    assert!(watched.cards.iter().filter(|card| card.zone == "hand").all(|card| card.definition_id == "__hidden__"));
    b.request("roomLeave", empty()).await;
    assert!(b.request("roomJoin", in_room()).await.ok(), "back to the seat");
    b.expect("fullState").await;

    // B disconnects: the seat is kept, marked disconnected; B rejoins on a new connection.
    b.disconnect().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let after_drop = parse_full_state(&a.latest("fullState").await.unwrap());
    let b_seat = after_drop.players.iter().find(|(id, _)| *id == b_id).unwrap();
    assert_eq!(b_seat.1, Some(false), "B should be disconnected");
    let mut b2 = TestClient::connect(address).await;
    assert!(b2.request("resume", object([("token", registered_b.text("token").into())])).await.ok());
    assert!(b2.request("roomJoin", in_room()).await.ok(), "rejoin");
    b2.expect("gameData").await;
    b2.expect("fullState").await;

    // Only the owner (or an admin) can close it; everyone returns to the lobby.
    assert_eq!(b2.request("roomClose", in_room()).await.error(), "notOwner");
    assert!(a.request("roomClose", in_room()).await.ok());
    b2.expect("roomClosed").await;
    s.expect("roomClosed").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_rooms_of_the_same_game_keep_separate_state() {
    let server = start_server().await;
    let address = &server.address;
    let (mut a, _) = TestClient::register(address, "a@x.com", "A").await;
    let (mut c, _) = TestClient::register(address, "c@x.com", "C").await;
    for (client, name) in [(&mut a, "Room A"), (&mut c, "Room C")] {
        let created = client
            .request("roomCreate", object([("gameId", "mini".into()), ("name", name.into()), ("maxPlayers", 1.into())]))
            .await;
        assert!(created.ok());
    }
    assert!(a.request("roomStart", empty()).await.ok());
    assert!(c.request("roomStart", empty()).await.ok());
    for client in [&mut a, &mut c] {
        client.expect("gameData").await;
        client.expect("fullState").await;
    }
    a.send("requestDrawFromZone", object([("zoneId", "deck".into()), ("count", 1.into())])).await;
    c.send("requestDrawFromZone", object([("zoneId", "deck".into()), ("count", 1.into())])).await;
    for client in [&mut a, &mut c] {
        let view = parse_full_state(&client.latest("fullState").await.expect("no fullState after the draw"));
        assert_eq!(view.cards.iter().filter(|card| card.zone_id == "deck").count(), 5);
        assert_eq!(view.cards.iter().filter(|card| card.zone == "hand").count(), 1);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn waiting_room_owner_leaves() {
    let server = start_server().await;
    let address = &server.address;
    let (mut a, _) = TestClient::register(address, "a@x.com", "A").await;
    let (mut b, registered_b) = TestClient::register(address, "b@x.com", "B").await;
    let created = a.request("roomCreate", object([("gameId", "mini".into()), ("maxPlayers", 3.into())])).await;
    let room_id = created.get("room").and_then(|room| room.get("id")).and_then(JsonValue::as_str).unwrap().to_string();
    assert!(created.get("room").unwrap().get("name").and_then(JsonValue::as_str) == Some("A's game"));
    b.request("roomJoin", object([("roomId", room_id.as_str().into())])).await;
    a.request("roomLeave", empty()).await;
    let rooms = b.latest("rooms").await.unwrap();
    assert!(rooms.contains(&format!("\"ownerId\":\"{}\"", registered_b.user_text("id"))), "ownership: {}", rooms.line);
    b.request("roomLeave", empty()).await;
    let rooms = a.latest("rooms").await.unwrap();
    assert!(rooms.contains("\"rooms\":[]"), "empty room not removed: {}", rooms.line);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_when_players_leave() {
    let server = start_server().await;
    let address = &server.address;
    let (mut a, _) = TestClient::register(address, "a@x.com", "A").await;
    let (mut b, _) = TestClient::register(address, "b@x.com", "B").await;
    assert_eq!(b.request("adminShutdown", empty()).await.error(), "notAdmin");
    let started = a.request("adminShutdown", empty()).await;
    assert!(started.ok());
    assert_eq!(started.get("seconds").and_then(JsonValue::as_f64), Some(300.0));
    assert!(b.expect("serverShutdown").await.contains("\"seconds\":300"));
    assert_eq!(a.request("adminShutdown", empty()).await.error(), "alreadyShuttingDown");
    // No new connections while the countdown runs.
    let mut late = TestClient::dial(address).await;
    late.send("hello", object([("serverProtocol", PROTOCOL_VERSION.into())])).await;
    assert!(late.expect("disconnect").await.contains("serverShuttingDown"));
    // The last other player leaving closes the server right away.
    b.disconnect().await;
    assert!(a.expect("disconnect").await.contains("serverShutdown"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_countdown() {
    let server = start_server().await;
    server.server.set_shutdown_delay(Duration::from_millis(300));
    let address = &server.address;
    let (mut a, _) = TestClient::register(address, "a@x.com", "A").await;
    let (mut b, _) = TestClient::register(address, "b@x.com", "B").await;
    let mut pending = TestClient::connect(address).await; // connected before the shutdown, not signed in
    assert!(a.request("adminShutdown", empty()).await.ok());
    let login = pending
        .request(
            "login",
            object([("email", "b@x.com".into()), ("passwordHash", password_hash("b@x.com", "pw").into())]),
        )
        .await;
    assert_eq!(login.error(), "shuttingDown");
    b.expect("serverShutdown").await;
    assert!(b.expect("disconnect").await.contains("serverShutdown"));
}
