//! Drives the lobby-server client against a running CardTable server (the Go server or
//! `gametable-server`), headless, to check protocol interop:
//!
//! ```text
//! cargo run -p gametable --example server_interop -- 127.0.0.1:51234 <game id> <upload file>
//! ```
//!
//! Two accounts are created (the first account on a fresh server is its admin). The run covers
//! sign-in and resume, lobby state, chat, game download over the asset channel, server decks,
//! creating and starting a room with the game channel, collection mode (packs, starter decks), a
//! complete trade between the two accounts, admin settings, an admin file upload, listing and
//! deletion, and finally draws every server screen against the live data to catch panics.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gametable::app::harness::UiHarness;
use gametable::app::{Frame, Screen};
use gametable::data::deck_store::{DeckStore, ServerDeckStore};
use gametable::net::admin_files::{FileUpload, UploadProgress};
use gametable::net::asset_sync::{DownloadProgress, GameDownload};
use gametable::net::game_client::{ClientStatus, GameClient};
use gametable::net::server_client::{ServerClient, ServerEvent, SharedServer, password_hash, response_ok};
use gametable::net::server_models::{ServerRoom, ServerUser, quantities};
use gametable::screens::server::admin::AdminScreen;
use gametable::screens::server::collection::CollectionScreen;
use gametable::screens::server::lobby::ServerLobby;
use gametable::screens::server::room::RoomScreen;
use gametable::screens::server::shop::ShopScreen;
use gametable::screens::server::trade::TradeScreen;
use gametable_core::json::{JsonObject, JsonValue};
use gametable_core::models::{DeckConfig, GameDefinition};

const PASSWORD: &str = "interop-password";

fn check(condition: bool, what: &str) {
    if condition {
        println!("ok   {what}");
    } else {
        println!("FAIL {what}");
        std::process::exit(1);
    }
}

/// Pumps every client (one frame each) until `done` holds, or fails after `seconds`.
fn pump_until(
    harness: &mut UiHarness,
    clients: &[&SharedServer],
    what: &str,
    seconds: u64,
    mut done: impl FnMut() -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        harness.frame(|frame| {
            for client in clients {
                client.borrow_mut().update(frame);
            }
        });
        if done() {
            return;
        }
        if Instant::now() > deadline {
            check(false, &format!("{what} (timed out)"));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Sends a request and waits for its answer.
fn ask(harness: &mut UiHarness, server: &SharedServer, message_type: &str, payload: JsonObject) -> JsonValue {
    let rid = server.borrow_mut().request(message_type, payload);
    let mut answer = None;
    pump_until(harness, &[server], &format!("reply to {message_type}"), 20, || {
        answer = server.borrow_mut().take_response(rid);
        answer.is_some()
    });
    answer.unwrap_or(JsonValue::Null)
}

/// `Join` up to the hand-over: connects and adopts the connection once the server says hello.
fn connect(harness: &mut UiHarness, host: &str, port: u16) -> SharedServer {
    let mut client = GameClient::new();
    client.connect(host, port, "Interop", 0xFF21_96F3, None, None);
    let deadline = Instant::now() + Duration::from_secs(10);
    while client.status != ClientStatus::Server {
        client.update();
        if let Some(message) = &client.error_message {
            check(false, &format!("connect to the server: {message}"));
        }
        assert!(Instant::now() < deadline, "timed out connecting (status {:?})", client.status);
        std::thread::sleep(Duration::from_millis(5));
    }
    let connection = client.connection.take().expect("the connection is handed over");
    let info = client.server_info.clone().unwrap_or(JsonValue::Null);
    let server = Rc::new(RefCell::new(ServerClient::adopt(connection, host, port, &info)));
    harness.frame(|_| {});
    server
}

/// Registers (or signs in, when the account exists) and stores the token and user.
fn sign_in(harness: &mut UiHarness, server: &SharedServer, email: &str, name: &str) -> ServerUser {
    let hash = password_hash(email, PASSWORD);
    let payload = JsonObject::new()
        .with("email", email)
        .with("passwordHash", hash.as_str())
        .with("name", name)
        .with("color", f64::from(0xFFE9_1E63u32));
    let mut response = ask(harness, server, "register", payload);
    if !response_ok(&response) {
        println!("     register: {}", gametable_core::json::encode_compact(&response).unwrap_or_default());
        response = ask(harness, server, "login", JsonObject::new().with("email", email).with("passwordHash", hash));
    }
    let problem = if response_ok(&response) {
        String::new()
    } else {
        format!(": {}", gametable_core::json::encode_compact(&response).unwrap_or_default())
    };
    check(response_ok(&response), &format!("{name} signs in{problem}"));
    let user = response.get("user").map(ServerUser::parse).unwrap_or_default();
    let token = response.get("token").and_then(JsonValue::as_str).unwrap_or("").to_string();
    let mut client = server.borrow_mut();
    client.token = Some(token);
    client.me = Some(user.clone());
    user
}

fn download(harness: &mut UiHarness, server: &SharedServer, game_id: &str) -> GameDefinition {
    let save_folder = harness.paths.save_folder.clone();
    let games_folder = save_folder.join("no-local-library").to_string_lossy().into_owned();
    let mut job = GameDownload::start(&mut server.borrow_mut(), &save_folder, &games_folder, None, game_id);
    let mut result = None;
    pump_until(harness, &[server], &format!("download {game_id}"), 120, || {
        match job.poll(&mut server.borrow_mut()) {
            DownloadProgress::Ready(game) => result = Some(Ok(*game)),
            DownloadProgress::Failed(message) => result = Some(Err(message)),
            _ => {}
        }
        result.is_some()
    });
    match result {
        Some(Ok(game)) => game,
        other => {
            check(false, &format!("download {game_id}: {other:?}", other = other.map(|result| result.err())));
            unreachable!()
        }
    }
}

/// Draws a screen for a few frames while pumping the clients (catches panics, settles requests).
fn draw_screen(harness: &mut UiHarness, clients: &[&SharedServer], screen: &mut dyn Screen, frames: usize) {
    for _ in 0..frames {
        harness.frame(|frame: &mut Frame| {
            for client in clients {
                client.borrow_mut().update(frame);
            }
            screen.background_update(frame, 1.0 / 60.0);
            screen.update(frame, 1.0 / 60.0);
            screen.draw(frame, 0.0, 48.0, 1280.0, 672.0);
        });
        harness.commands.clear();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let address = arguments.first().cloned().unwrap_or_else(|| "127.0.0.1:51234".to_string());
    let game_id = arguments.get(1).cloned().unwrap_or_else(|| "standard_52".to_string());
    let upload_path = arguments.get(2).cloned();
    let (host, port) = address.rsplit_once(':').expect("host:port");
    let port: u16 = port.parse().expect("a port number");
    let run_id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |time| time.as_secs());
    let mut harness = UiHarness::new();

    // `GAMETABLE_INTEROP_SESSION=<name>`: only sign that account in, leave a chat line and a
    // waiting room, and print the token (for screenshots of a resumed lobby).
    if let Ok(name) = std::env::var("GAMETABLE_INTEROP_SESSION") {
        let server = connect(&mut harness, host, port);
        let email = format!("{}@example.com", name.to_lowercase());
        sign_in(&mut harness, &server, &email, &name);
        pump_until(&mut harness, &[&server], "lobby state", 10, || !server.borrow().games.is_empty());
        server.borrow_mut().send("chatSend", JsonObject::new().with("text", "Anyone up for a game?").into());
        let payload =
            JsonObject::new().with("gameId", game_id.as_str()).with("name", "Friday Night").with("maxPlayers", 4.0);
        let response = ask(&mut harness, &server, "roomCreate", payload);
        check(response_ok(&response), "create a waiting room");
        pump_until(&mut harness, &[&server], "chat echo", 10, || !server.borrow().chat.is_empty());
        println!("token {} email {email}", server.borrow().token.clone().unwrap_or_default());
        return;
    }

    // -- Accounts and the lobby ----------------------------------------------------------------
    let alice = connect(&mut harness, host, port);
    let server_name = alice.borrow().info.display_name().to_string();
    check(!server_name.is_empty(), &format!("server info arrives ({server_name})"));
    let alice_user =
        sign_in(&mut harness, &alice, &format!("alice{run_id}@example.com"), &format!("Alice{}", run_id % 100000));
    pump_until(&mut harness, &[&alice], "lobby state", 10, || !alice.borrow().games.is_empty());
    check(alice.borrow().game_by_id(&game_id).is_some(), &format!("the server lists {game_id}"));
    pump_until(&mut harness, &[&alice], "online users", 10, || {
        alice.borrow().users.iter().any(|user| user.id == alice_user.id)
    });
    check(true, "the lobby lists the signed-in user");

    // Lobby chat comes back as an event.
    let text = format!("hello from the interop run {run_id}");
    alice.borrow_mut().send("chatSend", JsonObject::new().with("text", text.as_str()).into());
    pump_until(&mut harness, &[&alice], "lobby chat echo", 10, || {
        alice.borrow().chat.iter().any(|line| line.text == text)
    });
    check(true, "lobby chat round trip");

    // -- Game download -------------------------------------------------------------------------
    let started = Instant::now();
    let game = download(&mut harness, &alice, &game_id);
    check(
        !game.cards().is_empty(),
        &format!("downloaded {} ({} cards, {:?})", game.name, game.cards().len(), started.elapsed()),
    );
    let again = download(&mut harness, &alice, &game_id);
    check(again.id == game.id, "a second download is served from the cache");

    // -- Server decks --------------------------------------------------------------------------
    let deck_name = format!("Interop {run_id}");
    let mut store =
        ServerDeckStore::new(alice.clone(), harness.paths.save_folder.join("decks").to_string_lossy().into_owned());
    let saved = Rc::new(RefCell::new(None));
    let saved_result = saved.clone();
    let deck = DeckConfig::full(&game);
    store.write(
        &game,
        &deck_name,
        &deck,
        Some(Box::new(move |ok, message, _| *saved_result.borrow_mut() = Some((ok, message)))),
    );
    pump_until(&mut harness, &[&alice], "deckSave", 10, || saved.borrow().is_some());
    check(saved.borrow().as_ref().is_some_and(|(ok, _)| *ok), "save a deck on the server");
    let listed = Rc::new(RefCell::new(None));
    let listed_result = listed.clone();
    store.list(&game, Box::new(move |scan, error| *listed_result.borrow_mut() = Some((scan, error))));
    pump_until(&mut harness, &[&alice], "deckList", 10, || listed.borrow().is_some());
    let found = listed.borrow().as_ref().and_then(|(scan, _)| {
        scan.entries
            .iter()
            .find(|entry| entry.display_name == deck_name)
            .map(|entry| entry.deck.to_json() == deck.to_json())
    });
    check(found == Some(true), "the saved deck lists back unchanged");
    let removed = Rc::new(RefCell::new(None));
    let removed_result = removed.clone();
    store.remove(&game, &deck_name, Some(Box::new(move |ok, _| *removed_result.borrow_mut() = Some(ok))));
    pump_until(&mut harness, &[&alice], "deckDelete", 10, || removed.borrow().is_some());
    check(*removed.borrow() == Some(true), "delete the deck");

    // -- A room: create, chat, start, game channel ---------------------------------------------
    alice.borrow_mut().open_channel();
    let room_name = format!("Interop room {run_id}");
    let response = ask(
        &mut harness,
        &alice,
        "roomCreate",
        JsonObject::new().with("gameId", game_id.as_str()).with("name", room_name.as_str()).with("maxPlayers", 2.0),
    );
    check(response_ok(&response), "create a room");
    let room = response.get("room").map(ServerRoom::parse).unwrap_or_default();
    alice.borrow_mut().room_id = Some(room.id.clone());
    pump_until(&mut harness, &[&alice], "room listed", 10, || alice.borrow().current_room().is_some());
    check(true, "the room appears in the room list");
    let mut room_screen = RoomScreen::new(alice.clone(), game.clone());
    draw_screen(&mut harness, &[&alice], &mut room_screen, 10);
    let room_text = format!("room chat {run_id}");
    alice
        .borrow_mut()
        .send("chatSend", JsonObject::new().with("text", room_text.as_str()).with("roomId", room.id.as_str()).into());
    pump_until(&mut harness, &[&alice], "room chat", 10, || {
        alice.borrow().room_chat.iter().any(|line| line.text == room_text)
    });
    check(true, "room chat round trip");
    let response = ask(&mut harness, &alice, "roomStart", JsonObject::new());
    check(response_ok(&response), "start the game");
    pump_until(&mut harness, &[&alice], "gameData and fullState", 20, || alice.borrow().has_game_data());
    check(true, "the game channel receives gameData");
    draw_screen(&mut harness, &[&alice], &mut room_screen, 3);
    check(true, "the room screen hands over to the table");
    let response = ask(&mut harness, &alice, "roomLeave", JsonObject::new());
    check(response_ok(&response), "leave the room");
    {
        let mut client = alice.borrow_mut();
        client.room_id = None;
        client.close_channel();
    }
    let response = ask(&mut harness, &alice, "roomClose", JsonObject::new().with("roomId", room.id.as_str()));
    check(response_ok(&response) || response.get("error").is_some(), "end the room (or it already closed)");

    // -- Administration ------------------------------------------------------------------------
    let is_admin = alice.borrow().is_admin();
    if !is_admin {
        println!("skip admin, collection and trade checks: Alice isn't this server's admin");
    } else {
        let response = ask(&mut harness, &alice, "adminSettingsGet", JsonObject::new());
        check(response_ok(&response), "read the server settings");
        let mut settings = response.get("settings").cloned().unwrap_or(JsonValue::Null);
        let original_mode = settings.get("mode").and_then(JsonValue::as_str).unwrap_or("free").to_string();
        if let JsonValue::Object(object) = &mut settings {
            object.insert("mode", "collection");
            object.insert("startingTokens", 20.0);
            object.insert("packCost", 1.0);
        }
        let response =
            ask(&mut harness, &alice, "adminSettingsSet", JsonObject::new().with("settings", settings.clone()));
        check(response_ok(&response), "switch the server to collection mode");
        pump_until(&mut harness, &[&alice], "serverInfo update", 10, || alice.borrow().is_collection_mode());
        check(true, "the lobby sees collection mode");
        let response = ask(&mut harness, &alice, "adminUsers", JsonObject::new());
        let user_count = response.get("users").and_then(JsonValue::as_array).map_or(0, Vec::len);
        check(response_ok(&response) && user_count > 0, &format!("list users ({user_count})"));
        let response = ask(
            &mut harness,
            &alice,
            "adminUserUpdate",
            JsonObject::new().with("id", alice_user.id.as_str()).with("tokensAdd", 10.0),
        );
        check(response_ok(&response), "grant tokens");

        // Packs and the collection.
        let info = alice.borrow().game_by_id(&game_id).cloned().unwrap_or_default();
        if let Some(set) = info.sets.first() {
            let response = ask(
                &mut harness,
                &alice,
                "packBuy",
                JsonObject::new().with("gameId", game_id.as_str()).with("setId", set.id.as_str()),
            );
            let opened = response.get("cards").and_then(JsonValue::as_array).map_or(0, Vec::len);
            check(response_ok(&response) && opened > 0, &format!("open a {} pack ({opened} cards)", set.name));
        } else {
            println!("skip packBuy: {game_id} has no sets");
        }
        if let Some(starter) = info.starter_decks.first() {
            let response = ask(
                &mut harness,
                &alice,
                "starterBuy",
                JsonObject::new().with("gameId", game_id.as_str()).with("name", starter.name.as_str()),
            );
            check(response_ok(&response), &format!("buy the {} starter deck", starter.name));
        }
        let response = ask(&mut harness, &alice, "collectionGet", JsonObject::new().with("gameId", game_id.as_str()));
        let owned = quantities(response.get("cards"));
        check(response_ok(&response), &format!("read the collection ({} distinct cards)", owned.len()));

        // A trade: Bob asks, Alice accepts, both offer and confirm.
        let bob = connect(&mut harness, host, port);
        let bob_user =
            sign_in(&mut harness, &bob, &format!("bob{run_id}@example.com"), &format!("Bob{}", run_id % 100000));
        pump_until(&mut harness, &[&alice, &bob], "Bob's lobby", 10, || !bob.borrow().games.is_empty());
        if let Some(set) = info.sets.first() {
            let response = ask(
                &mut harness,
                &bob,
                "packBuy",
                JsonObject::new().with("gameId", game_id.as_str()).with("setId", set.id.as_str()),
            );
            check(response_ok(&response), "Bob opens a pack");
        }
        let response = ask(
            &mut harness,
            &bob,
            "tradeRequest",
            JsonObject::new().with("targetId", alice_user.id.as_str()).with("gameId", game_id.as_str()),
        );
        check(response_ok(&response), "Bob asks Alice to trade");
        let mut invite = None;
        pump_until(&mut harness, &[&alice, &bob], "trade invite", 10, || {
            for event in alice.borrow_mut().take_events() {
                if let ServerEvent::TradeInvite { trade_id, from_name } = event {
                    invite = Some((trade_id, from_name));
                }
            }
            invite.is_some()
        });
        let (trade_id, from_name) = invite.unwrap_or_default();
        check(from_name == bob_user.name, "Alice is invited by Bob");
        alice
            .borrow_mut()
            .request_ignored("tradeRespond", JsonObject::new().with("tradeId", trade_id.as_str()).with("accept", true));
        pump_until(&mut harness, &[&alice, &bob], "trade opens", 10, || {
            alice.borrow().trade.is_some() && bob.borrow().trade.is_some()
        });
        check(true, "both sides see the open trade");
        let response =
            ask(&mut harness, &bob, "tradeCollectionGet", JsonObject::new().with("tradeId", trade_id.as_str()));
        let alice_cards = quantities(response.get("cards"));
        check(response_ok(&response), &format!("Bob sees Alice's collection ({} cards)", alice_cards.len()));
        let mut trade_screen = TradeScreen::new(bob.clone(), game.clone());
        draw_screen(&mut harness, &[&alice, &bob], &mut trade_screen, 10);
        let wanted = alice_cards.iter().find(|(_, count)| **count > 0).map(|(id, _)| id.clone());
        if let Some(card_id) = &wanted {
            bob.borrow_mut().request_ignored(
                "tradeOffer",
                JsonObject::new()
                    .with("tradeId", trade_id.as_str())
                    .with("cardId", card_id.as_str())
                    .with("delta", 1.0),
            );
            pump_until(&mut harness, &[&alice, &bob], "offer arrives", 10, || {
                alice
                    .borrow()
                    .trade
                    .as_ref()
                    .is_some_and(|trade| trade.offer_of(&bob_user.id).cards.contains_key(card_id))
            });
            check(true, "Bob's want reaches Alice");
        }
        harness.frame(|frame| trade_screen.leave(frame));
        for client in [&alice, &bob] {
            client.borrow_mut().request_ignored(
                "tradeConfirm",
                JsonObject::new().with("tradeId", trade_id.as_str()).with("confirmed", true),
            );
        }
        let mut closed = None;
        pump_until(&mut harness, &[&alice, &bob], "trade completes", 10, || {
            for event in bob.borrow_mut().take_events() {
                if let ServerEvent::TradeClosed { reason, .. } = event {
                    closed = Some(reason);
                }
            }
            closed.is_some()
        });
        check(closed.as_deref() == Some("completed"), &format!("the trade completes ({closed:?})"));
        if let Some(card_id) = &wanted {
            let response = ask(&mut harness, &bob, "collectionGet", JsonObject::new().with("gameId", game_id.as_str()));
            let bob_cards = quantities(response.get("cards"));
            check(bob_cards.get(card_id).copied().unwrap_or(0) > 0, "Bob owns the traded card");
        }

        // Admin file upload, list and delete.
        if let Some(local_path) = &upload_path {
            let destination =
                format!("interop_{run_id}/{}", std::path::Path::new(local_path).file_name().unwrap().to_string_lossy());
            let token = alice.borrow().token.clone();
            let mut upload = FileUpload::start(host, port, token, &game_id, &destination, local_path);
            let mut finished = None;
            pump_until(&mut harness, &[&alice, &bob], "file upload", 60, || {
                match upload.poll() {
                    UploadProgress::Uploading { .. } => {}
                    other => finished = Some(other),
                }
                finished.is_some()
            });
            check(finished == Some(UploadProgress::Done), &format!("upload {destination} ({finished:?})"));
            let folder = destination.rsplit_once('/').map_or("", |(folder, _)| folder).to_string();
            let response = ask(
                &mut harness,
                &alice,
                "adminFileList",
                JsonObject::new().with("gameId", game_id.as_str()).with("path", folder.as_str()),
            );
            let listed = response.get("entries").and_then(JsonValue::as_array).map_or(0, Vec::len);
            check(response_ok(&response) && listed == 1, "the uploaded file is listed");
            // Folders must be emptied first.
            for path in [destination.as_str(), folder.as_str()] {
                let response = ask(
                    &mut harness,
                    &alice,
                    "adminFileDelete",
                    JsonObject::new().with("gameId", game_id.as_str()).with("path", path),
                );
                check(response_ok(&response), &format!("delete {path}"));
            }
        }

        // Every server screen against the live data.
        let mut lobby = harness_screen(&mut harness, |frame| ServerLobby::new(frame, alice.clone()));
        draw_screen(&mut harness, &[&alice, &bob], &mut lobby, 20);
        let mut shop = harness_screen(&mut harness, |frame| ShopScreen::new(frame, alice.clone(), info.clone()));
        draw_screen(&mut harness, &[&alice, &bob], &mut shop, 60);
        let mut collection = harness_screen(&mut harness, |frame| CollectionScreen::new(frame, alice.clone(), &info));
        draw_screen(&mut harness, &[&alice, &bob], &mut collection, 60);
        let mut admin = AdminScreen::new(alice.clone());
        draw_screen(&mut harness, &[&alice, &bob], &mut admin, 20);
        check(true, "the lobby, shop, collection and admin screens draw");

        // Put the mode back.
        if let JsonValue::Object(object) = &mut settings {
            object.insert("mode", original_mode.as_str());
        }
        let response = ask(&mut harness, &alice, "adminSettingsSet", JsonObject::new().with("settings", settings));
        check(response_ok(&response), "restore the server mode");
        bob.borrow_mut().disconnect();
    }
    // Reconnect and resume with the token (as Remember me does), then log out.
    let token = alice.borrow().token.clone().unwrap_or_default();
    alice.borrow_mut().disconnect();
    let resumed = connect(&mut harness, host, port);
    let response = ask(&mut harness, &resumed, "resume", JsonObject::new().with("token", token.as_str()));
    check(response_ok(&response), "resume with the saved token on a new connection");
    let response = ask(&mut harness, &resumed, "logout", JsonObject::new().with("token", token.as_str()));
    check(response_ok(&response) || response.get("error").is_some(), "log out");
    println!("all checks passed");
}

/// Builds a screen with the harness's frame.
fn harness_screen<S>(harness: &mut UiHarness, build: impl FnOnce(&mut Frame) -> S) -> S {
    let mut screen = None;
    harness.frame(|frame| screen = Some(build(frame)));
    screen.expect("built")
}
