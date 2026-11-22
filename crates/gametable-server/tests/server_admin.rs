//! Admin tools, the asset channel and trading. Ports of `TestAdmin`, `TestAssets`,
//! `TestAdminFiles`, `TestAdminGames` and `TestTrading` from the Go server.

mod common;

use std::io::Write;

use base64::Engine;
use common::*;
use gametable_core::json::JsonValue;

fn base64_text(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn entry_names(response: &Response) -> Vec<String> {
    response
        .get("entries")
        .and_then(JsonValue::as_array)
        .map(|entries| entries.iter().filter_map(|e| e.get("name")?.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// Opens an asset channel signed in with a session token.
async fn asset_channel(address: &str, token: &str) -> TestClient {
    let mut channel = TestClient::dial(address).await;
    channel.send("assetHello", object([("token", token.into())])).await;
    channel.expect("assetReady").await;
    channel
}

#[tokio::test(flavor = "multi_thread")]
async fn admin() {
    let server = start_server().await;
    let (mut a, registered_a) = TestClient::register(&server.address, "a@x.com", "A").await;
    let (mut b, registered_b) = TestClient::register(&server.address, "b@x.com", "B").await;
    let (a_id, b_id) = (registered_a.user_text("id"), registered_b.user_text("id"));
    let update = |id: &str, entries: Vec<(&str, JsonValue)>| {
        let mut payload = object([("id", id.into())]);
        if let JsonValue::Object(fields) = &mut payload {
            for (key, value) in entries {
                fields.insert(key, value);
            }
        }
        payload
    };
    let demote = a.request("adminUserUpdate", update(&a_id, vec![("isAdmin", false.into())])).await;
    assert_eq!(demote.error(), "lastAdmin");
    let taken = a.request("adminUserUpdate", update(&b_id, vec![("name", "a".into())])).await;
    assert_eq!(taken.error(), "nameTaken");
    let renamed =
        a.request("adminUserUpdate", update(&b_id, vec![("name", "Bobby".into()), ("isAdmin", true.into())])).await;
    assert!(renamed.ok(), "{:?}", renamed.0);
    let own_view = b.expect("self").await;
    assert!(own_view.contains(r#""name":"Bobby""#), "{}", own_view.line);

    // Password reset by an admin; the old password stops working.
    let new_hash = password_hash("b@x.com", "new");
    a.request(
        "adminUserUpdate",
        update(&b_id, vec![("newPasswordHash", new_hash.as_str().into()), ("disabled", true.into())]),
    )
    .await;
    b.expect("disconnect").await;
    let mut c = TestClient::connect(&server.address).await;
    let login = || object([("email", "b@x.com".into()), ("passwordHash", new_hash.as_str().into())]);
    assert_eq!(c.request("login", login()).await.error(), "disabled");
    a.request("adminUserUpdate", update(&b_id, vec![("disabled", false.into())])).await;
    let logged_in = c.request("login", login()).await;
    assert!(logged_in.ok(), "login with reset password: {:?}", logged_in.0);

    assert_eq!(a.request("adminUserDelete", object([("id", a_id.as_str().into())])).await.error(), "self");
    // Everyone who has signed in has a last-seen time.
    let users = a.request("adminUsers", empty()).await;
    let last_seen: Vec<f64> = users
        .get("users")
        .and_then(JsonValue::as_array)
        .unwrap()
        .iter()
        .map(|user| user.get("lastSeenAt").and_then(JsonValue::as_f64).unwrap_or(0.0))
        .collect();
    assert!(last_seen.iter().all(|seen| *seen > 0.0), "{last_seen:?}");

    assert!(a.request("adminUserDelete", object([("id", b_id.as_str().into())])).await.ok());
    let users = a.request("adminUsers", empty()).await;
    assert_eq!(users.get("users").and_then(JsonValue::as_array).map(Vec::len), Some(1), "user not deleted");
}

#[tokio::test(flavor = "multi_thread")]
async fn assets() {
    let server = start_server().await;
    let (_a, registered) = TestClient::register(&server.address, "a@x.com", "A").await;
    let token = registered.text("token");
    let mut lobby = TestClient::connect(&server.address).await;
    lobby.request("resume", object([("token", token.as_str().into())])).await;
    let manifest = lobby.request("gameManifest", object([("gameId", "mini".into())])).await;
    let paths: Vec<String> = manifest
        .get("files")
        .and_then(JsonValue::as_array)
        .unwrap()
        .iter()
        .filter_map(|file| file.get("path")?.as_str().map(str::to_string))
        .collect();
    assert_eq!(paths.join(","), "gamedef.json,s1/c1.png");

    let mut channel = asset_channel(&server.address, &token).await;
    channel.send("assetGet", object([("gameId", "mini".into()), ("path", "s1/c1.png".into())])).await;
    let data = channel.expect("assetData").await;
    assert_eq!(data.payload.get("done").and_then(JsonValue::as_bool), Some(true));
    let encoded = data.payload.get("data").and_then(JsonValue::as_str).unwrap();
    assert_eq!(base64::engine::general_purpose::STANDARD.decode(encoded).unwrap(), b"\x89PNGfake-image-bytes");
    channel.send("assetGet", object([("gameId", "mini".into()), ("path", "../../test.db".into())])).await;
    channel.expect("assetError").await;

    let mut bad = TestClient::dial(&server.address).await;
    bad.send("assetHello", object([("token", "nope".into())])).await;
    bad.expect("disconnect").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn large_assets_stream_in_chunks() {
    let server = start_server().await;
    let big_file: Vec<u8> = (0..(192 << 10) * 2).map(|index| (index % 251) as u8).collect();
    std::fs::write(server.data_folder.join("games").join("mini").join("s1").join("big.pdf"), &big_file).unwrap();
    server.server.library.write().unwrap().scan().unwrap();
    let (_a, registered) = TestClient::register(&server.address, "a@x.com", "A").await;
    let mut channel = asset_channel(&server.address, &registered.text("token")).await;
    channel.send("assetGet", object([("gameId", "mini".into()), ("path", "s1/big.pdf".into())])).await;
    let mut received = Vec::new();
    let mut chunks = 0;
    loop {
        let data = channel.expect("assetData").await;
        chunks += 1;
        assert_eq!(data.payload.get("offset").and_then(JsonValue::as_f64), Some(received.len() as f64));
        let encoded = data.payload.get("data").and_then(JsonValue::as_str).unwrap();
        received.extend(base64::engine::general_purpose::STANDARD.decode(encoded).unwrap());
        if data.payload.get("done").and_then(JsonValue::as_bool) == Some(true) {
            break;
        }
    }
    // An exact multiple of the chunk size ends with an empty `done` chunk, as in Go.
    assert_eq!(chunks, 3);
    assert_eq!(received, big_file);
}

#[tokio::test(flavor = "multi_thread")]
async fn admin_files() {
    let server = start_server().await;
    let (mut a, registered_a) = TestClient::register(&server.address, "a@x.com", "A").await;
    let (mut b, registered_b) = TestClient::register(&server.address, "b@x.com", "B").await;
    let path = |path: &str| object([("gameId", "mini".into()), ("path", path.into())]);

    assert_eq!(b.request("adminFileList", path("")).await.error(), "notAdmin");
    assert_eq!(b.request("adminFileDelete", path("s1/c1.png")).await.error(), "notAdmin");

    let root = a.request("adminFileList", path("")).await;
    assert!(root.ok(), "{:?}", root.0);
    let names = entry_names(&root);
    // Folders first, then files, case-insensitively.
    assert_eq!(names, ["s1", "savegames", "gamedef.json"]);
    assert_eq!(entry_names(&a.request("adminFileList", path("s1")).await).len(), 1);

    // A path that tries to escape the game folder is clamped inside it.
    assert!(!a.request("adminFileList", path("../../test.db")).await.ok());
    assert!(!a.request("adminFileDelete", path("../../test.db")).await.ok());
    assert!(server.data_folder.join("test.db").exists());
    assert_eq!(a.request("adminFileDelete", path("/")).await.error(), "badPath");
    assert_eq!(a.request("adminFileDelete", path("s1")).await.error(), "notEmpty");

    // Upload (chunked, over the asset channel), as a non-admin then as the admin.
    let mut b_channel = asset_channel(&server.address, &registered_b.text("token")).await;
    let put = |path: &str, offset: usize, data: &[u8], done: bool| {
        object([
            ("gameId", "mini".into()),
            ("path", path.into()),
            ("offset", offset.into()),
            ("data", base64_text(data).into()),
            ("done", done.into()),
        ])
    };
    b_channel.send("assetPut", put("s1/new.png", 0, b"\0\0\0", true)).await;
    b_channel.expect("assetPutError").await;

    let mut a_channel = asset_channel(&server.address, &registered_a.text("token")).await;
    let first_half = b"\x89PNGfirst-half-";
    a_channel.send("assetPut", put("s1/new.png", 0, first_half, false)).await;
    a_channel.expect("assetPutAck").await;
    a_channel.send("assetPut", put("s1/new.png", 3, b"wrong", false)).await;
    let wrong_offset = a_channel.expect("assetPutError").await;
    assert!(wrong_offset.contains("badOffset"), "{}", wrong_offset.line);
    a_channel.send("assetPut", put("s1/new.png", 0, first_half, false)).await;
    a_channel.expect("assetPutAck").await;
    a_channel.send("assetPut", put("s1/new.png", first_half.len(), b"second-half", true)).await;
    let ack = a_channel.expect("assetPutAck").await;
    assert_eq!(ack.payload.get("offset").and_then(JsonValue::as_f64), Some((first_half.len() + 11) as f64));

    let written = std::fs::read(server.data_folder.join("games").join("mini").join("s1").join("new.png")).unwrap();
    assert_eq!(written, b"\x89PNGfirst-half-second-half");
    assert_eq!(entry_names(&a.request("adminFileList", path("s1")).await).len(), 2);
    let manifest = a.request("gameManifest", object([("gameId", "mini".into())])).await;
    assert!(
        manifest
            .0
            .get("files")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .any(|file| { file.get("path").and_then(JsonValue::as_str) == Some("s1/new.png") })
    );
    assert!(a.request("adminFileDelete", path("s1/new.png")).await.ok());
}

fn build_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, content) in files {
        writer.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        writer.write_all(content).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// Chunks data to the asset channel's flat staging area (`gameId: ""`).
async fn upload_staged(address: &str, token: &str, name: &str, data: &[u8]) {
    let mut channel = asset_channel(address, token).await;
    const CHUNK: usize = 64 * 1024;
    let mut offset = 0;
    loop {
        let end = (offset + CHUNK).min(data.len());
        let done = end >= data.len();
        channel
            .send(
                "assetPut",
                object([
                    ("gameId", "".into()),
                    ("path", name.into()),
                    ("offset", offset.into()),
                    ("data", base64_text(&data[offset..end]).into()),
                    ("done", done.into()),
                ]),
            )
            .await;
        channel.expect("assetPutAck").await;
        offset = end;
        if done {
            break;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn admin_games() {
    let server = start_server().await;
    let (mut a, registered) = TestClient::register(&server.address, "a@x.com", "A").await;
    let (mut b, _) = TestClient::register(&server.address, "b@x.com", "B").await;
    let token = registered.text("token");
    let install = |upload: &str| object([("upload", upload.into())]);

    let zip_data = build_zip(&[
        ("newgame/gamedef.json", br#"{"id": "newgame", "name": "New Game"}"#),
        ("newgame/s1/c1.png", b"fake-image-bytes"),
    ]);
    upload_staged(&server.address, &token, "install1.zip", &zip_data).await;
    let installed = a.request("adminGameInstall", install("install1.zip")).await;
    assert!(installed.ok() && installed.text("gameId") == "newgame", "{:?}", installed.0);
    assert!(a.request("gameManifest", object([("gameId", "newgame".into())])).await.ok());
    assert!(!server.data_folder.join("games").join(".uploads").join("install1.zip").exists());

    let mismatched = build_zip(&[("wrongname/gamedef.json", br#"{"id": "realname"}"#)]);
    upload_staged(&server.address, &token, "install2.zip", &mismatched).await;
    assert_eq!(a.request("adminGameInstall", install("install2.zip")).await.error(), "idMismatch");

    let slip = build_zip(&[("slip/gamedef.json", br#"{"id": "slip"}"#), ("slip/../../evil.txt", b"pwned")]);
    upload_staged(&server.address, &token, "install3.zip", &slip).await;
    assert_eq!(a.request("adminGameInstall", install("install3.zip")).await.error(), "badZip");
    assert!(!server.data_folder.join("evil.txt").exists());

    // A .gamedef upload is kept as a package, not unzipped, and is served like any game.
    let package = build_zip(&[
        ("packed/gamedef.json", br#"{"id": "packed", "name": "Packed Game"}"#),
        ("packed/s1/c1.png", b"packed-image-bytes"),
    ]);
    upload_staged(&server.address, &token, "install6.gamedef", &package).await;
    let installed_package = a.request("adminGameInstall", install("install6.gamedef")).await;
    assert!(installed_package.ok() && installed_package.text("gameId") == "packed", "{:?}", installed_package.0);
    let games_folder = server.data_folder.join("games");
    assert!(games_folder.join("packed.gamedef").is_file());
    assert!(!games_folder.join("packed").exists(), "a package is not unzipped");
    assert!(a.request("gameManifest", object([("gameId", "packed".into())])).await.ok());
    upload_staged(&server.address, &token, "install7.gamedef", &package).await;
    assert_eq!(a.request("adminGameInstall", install("install7.gamedef")).await.error(), "exists");
    let files = a.request("adminFileList", object([("gameId", "packed".into()), ("path", "".into())])).await;
    assert_eq!(files.error(), "packaged");
    assert!(a.request("adminGameDelete", object([("gameId", "packed".into())])).await.ok());
    assert!(!games_folder.join("packed.gamedef").exists());

    let two_roots = build_zip(&[("one/gamedef.json", br#"{"id": "one"}"#), ("two/x.png", b"x")]);
    upload_staged(&server.address, &token, "install5.zip", &two_roots).await;
    assert_eq!(a.request("adminGameInstall", install("install5.zip")).await.error(), "badShape");
    assert_eq!(a.request("adminGameInstall", install("../x.zip")).await.error(), "badUpload");
    assert_eq!(a.request("adminGameInstall", install("missing.zip")).await.error(), "badZip");

    let settings = a.request("adminSettingsGet", empty()).await;
    assert_eq!(settings.get("settings").and_then(|s| s.get("maxUploadMB")).and_then(JsonValue::as_f64), Some(2048.0));
    // Go sends `"gameErrors": null` with no scan errors (the test decoder drops nulls).
    assert!(settings.get("gameErrors").is_none());
    let limit = object([("settings", object([("maxUploadMB", 1.into())]))]);
    assert!(a.request("adminSettingsSet", limit).await.ok());
    let big_content = vec![b'x'; 2 << 20];
    let big = build_zip(&[("big/gamedef.json", br#"{"id": "big"}"#), ("big/s1/c1.png", &big_content)]);
    upload_staged(&server.address, &token, "install4.zip", &big).await;
    assert_eq!(a.request("adminGameInstall", install("install4.zip")).await.error(), "tooLarge");

    assert_eq!(b.request("adminGameDelete", object([("gameId", "newgame".into())])).await.error(), "notAdmin");
    assert!(a.request("adminGameDelete", object([("gameId", "newgame".into())])).await.ok());
    assert_eq!(a.request("gameManifest", object([("gameId", "newgame".into())])).await.error(), "noGame");
}

/// `offers.<user id>` of a `trade` message: (wanted cards, confirmed).
fn offer_of(message: &Message, user_id: &str) -> (Option<f64>, bool) {
    let offer = message.payload.get("offers").and_then(|offers| offers.get(user_id));
    let wanted =
        offer.and_then(|offer| offer.get("cards")).and_then(|cards| cards.get("r1")).and_then(JsonValue::as_f64);
    let confirmed = offer.and_then(|offer| offer.get("confirmed")).and_then(JsonValue::as_bool).unwrap_or(false);
    (wanted, confirmed)
}

#[tokio::test(flavor = "multi_thread")]
async fn trading() {
    let server = start_server().await;
    let (mut a, registered_a) = TestClient::register(&server.address, "a@x.com", "A").await;
    let (mut b, registered_b) = TestClient::register(&server.address, "b@x.com", "B").await;
    let (a_id, b_id) = (registered_a.user_text("id"), registered_b.user_text("id"));
    let settings = object([("settings", object([("mode", "collection".into()), ("packCost", 1.into())]))]);
    assert!(a.request("adminSettingsSet", settings).await.ok());
    a.expect("serverInfo").await;
    b.expect("serverInfo").await;
    for id in [&a_id, &b_id] {
        a.request("adminUserUpdate", object([("id", id.as_str().into()), ("tokensSet", 5.into())])).await;
    }
    // Every pack from this set includes exactly one "r1".
    let buy = || object([("gameId", "mini".into()), ("setId", "s1".into())]);
    assert!(a.request("packBuy", buy()).await.ok());
    assert!(b.request("packBuy", buy()).await.ok());

    let request_trade = || object([("targetId", b_id.as_str().into()), ("gameId", "mini".into())]);
    // Trading while seated in a room is refused.
    a.request("roomCreate", object([("gameId", "mini".into()), ("maxPlayers", 2.into())])).await;
    assert_eq!(a.request("tradeRequest", request_trade()).await.error(), "inRoom");
    a.request("roomLeave", empty()).await;
    let self_trade =
        a.request("tradeRequest", object([("targetId", a_id.as_str().into()), ("gameId", "mini".into())])).await;
    assert_eq!(self_trade.error(), "badTarget");

    // A declined request notifies the sender and leaves both free to trade again.
    let requested = a.request("tradeRequest", request_trade()).await;
    assert!(requested.ok(), "{:?}", requested.0);
    let trade_id = requested.text("tradeId");
    let invite = b.expect("tradeInvite").await;
    assert_eq!(invite.payload.get("fromName").and_then(JsonValue::as_str), Some("A"));
    assert_eq!(invite.payload.get("tradeId").and_then(JsonValue::as_str), Some(trade_id.as_str()));
    assert_eq!(a.request("tradeRequest", request_trade()).await.error(), "trading");
    let respond = |trade_id: &str, accept: bool| object([("tradeId", trade_id.into()), ("accept", accept.into())]);
    assert!(b.request("tradeRespond", respond(&trade_id, false)).await.ok());
    let closed = a.expect("tradeClosed").await;
    assert!(closed.contains(r#""reason":"declined""#), "{}", closed.line);

    // Accept, offer, and confirm from both sides swaps the cards.
    let trade_id = a.request("tradeRequest", request_trade()).await.text("tradeId");
    b.expect("tradeInvite").await;
    assert!(b.request("tradeRespond", respond(&trade_id, true)).await.ok());
    a.expect("trade").await;
    let started = b.expect("trade").await;
    assert!(started.contains(r#""chat":null"#), "{}", started.line);

    let offer =
        |delta: i64| object([("tradeId", trade_id.as_str().into()), ("cardId", "r1".into()), ("delta", delta.into())]);
    assert!(b.request("tradeOffer", offer(1)).await.ok());
    a.expect("trade").await;
    assert_eq!(offer_of(&b.expect("trade").await, &b_id).0, Some(1.0));
    // Asking for more than the giver owns clamps at what they have.
    assert!(b.request("tradeOffer", offer(10)).await.ok());
    a.expect("trade").await;
    assert_eq!(offer_of(&b.expect("trade").await, &b_id).0, Some(1.0));

    let confirm = || object([("tradeId", trade_id.as_str().into()), ("confirmed", true.into())]);
    assert!(a.request("tradeConfirm", confirm()).await.ok());
    a.expect("trade").await;
    let view = b.expect("trade").await;
    assert!(offer_of(&view, &a_id).1 && !offer_of(&view, &b_id).1, "{}", view.line);
    assert!(a.request("tradeOffer", offer(0)).await.ok());

    let r1_of =
        |response: &Response| response.get("cards").and_then(|cards| cards.get("r1")).and_then(JsonValue::as_f64);
    let collection = || object([("gameId", "mini".into())]);
    assert_eq!(r1_of(&a.request("collectionGet", collection()).await), Some(1.0));
    assert_eq!(r1_of(&b.request("collectionGet", collection()).await), Some(1.0));
    let partner = a.request("tradeCollectionGet", object([("tradeId", trade_id.as_str().into())])).await;
    assert_eq!(r1_of(&partner), Some(1.0));

    // Trade chat reaches both sides.
    a.send("chatSend", object([("text", "  deal?  ".into()), ("tradeId", trade_id.as_str().into())])).await;
    let chat = b.expect("chat").await;
    assert!(chat.contains(r#""text":"deal?""#) && chat.contains(&trade_id), "{}", chat.line);
    a.expect("chat").await;

    assert!(a.request("tradeConfirm", confirm()).await.ok());
    a.expect("trade").await;
    b.expect("trade").await;
    assert!(b.request("tradeConfirm", confirm()).await.ok());
    let (closed_a, closed_b) = (a.expect("tradeClosed").await, b.expect("tradeClosed").await);
    assert!(closed_a.contains(r#""reason":"completed""#) && closed_b.contains(r#""reason":"completed""#));
    assert!(closed_b.contains(r#""gaveA":{"r1":1}"#), "{}", closed_b.line);
    assert_eq!(r1_of(&a.request("collectionGet", collection()).await), None, "A should have given up r1");
    assert_eq!(r1_of(&b.request("collectionGet", collection()).await), Some(2.0), "B should have received r1");

    // Voluntarily leaving an active trade notifies both sides, including the one who left.
    let trade_id = a.request("tradeRequest", request_trade()).await.text("tradeId");
    b.expect("tradeInvite").await;
    b.request("tradeRespond", respond(&trade_id, true)).await;
    a.expect("trade").await;
    b.expect("trade").await;
    assert!(a.request("tradeLeave", object([("tradeId", trade_id.as_str().into())])).await.ok());
    let (left_a, left_b) = (a.expect("tradeClosed").await, b.expect("tradeClosed").await);
    assert!(left_a.contains(r#""reason":"left""#) && left_b.contains(r#""reason":"left""#));

    // Leaving outside of any trade, or responding to a stale invite, fails cleanly.
    assert!(a.request("tradeLeave", object([("tradeId", "nope".into())])).await.ok());
    assert_eq!(a.request("tradeRespond", respond("nope", true)).await.error(), "noTrade");

    // Disconnecting ends a pending invite for the other side.
    let trade_id = a.request("tradeRequest", request_trade()).await.text("tradeId");
    b.expect("tradeInvite").await;
    b.disconnect().await;
    let left = a.expect("tradeClosed").await;
    assert!(left.contains(r#""reason":"left""#) && left.contains(&trade_id), "{}", left.line);
}
