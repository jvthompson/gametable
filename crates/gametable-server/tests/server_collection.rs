//! Decks, collection mode, packs and starter decks. Ports of `TestDecks`, `TestCollectionMode`,
//! `TestStarterDecks` and `TestBuiltInDecksPrefersUnderscoreFolder` from the Go server.

mod common;

use common::*;
use gametable_core::json::JsonValue;

fn number(value: Option<&JsonValue>) -> f64 {
    value.and_then(JsonValue::as_f64).unwrap_or(f64::NAN)
}

fn deck_of(entries: &[(&str, i64)]) -> JsonValue {
    let entries: Vec<JsonValue> = entries
        .iter()
        .map(|(id, quantity)| object([("definitionId", (*id).into()), ("quantity", (*quantity).into())]))
        .collect();
    object([
        ("gameId", "mini".into()),
        (
            "subdecks",
            JsonValue::Array(vec![object([("name", "main_deck".into()), ("entries", JsonValue::Array(entries))])]),
        ),
    ])
}

#[tokio::test(flavor = "multi_thread")]
async fn decks() {
    let server = start_server().await;
    let (mut a, _) = TestClient::register(&server.address, "a@x.com", "A").await;
    let deck = deck_of(&[("c1", 2)]);
    let save = |name: &str, overwrite: bool| {
        object([
            ("gameId", "mini".into()),
            ("name", name.into()),
            ("deck", deck.clone()),
            ("overwrite", overwrite.into()),
        ])
    };
    assert!(a.request("deckSave", save("Aggro", false)).await.ok());
    assert_eq!(a.request("deckSave", save("aggro", false)).await.error(), "exists");
    assert!(a.request("deckSave", save("aggro", true)).await.ok());
    let bad_name = a.request("deckSave", save("a/b", false)).await;
    assert_eq!(bad_name.error(), "badName");
    let no_subdecks = a
        .request(
            "deckSave",
            object([("gameId", "mini".into()), ("name", "X".into()), ("deck", object([("gameId", "mini".into())]))]),
        )
        .await;
    assert_eq!(no_subdecks.error(), "badDeck");

    let list = a.request("deckList", object([("gameId", "mini".into())])).await;
    let decks = list.get("decks").and_then(JsonValue::as_array).unwrap();
    assert_eq!(decks.len(), 1);
    assert_eq!(decks[0].get("name").and_then(JsonValue::as_str), Some("aggro"));
    assert!(decks[0].get("deck").is_none(), "deck JSON only with full");

    let got = a.request("deckGet", object([("gameId", "mini".into()), ("name", "AGGRO".into())])).await;
    assert!(got.ok(), "{:?}", got.0);
    assert_eq!(got.get("deck"), Some(&deck));

    a.request("deckDelete", object([("gameId", "mini".into()), ("name", "aggro".into())])).await;
    let list = a.request("deckList", object([("gameId", "mini".into())])).await;
    assert_eq!(list.get("decks").and_then(JsonValue::as_array).map(Vec::len), Some(0), "deck not deleted");
}

#[tokio::test(flavor = "multi_thread")]
async fn collection_mode() {
    let server = start_server().await;
    let (mut a, registered_a) = TestClient::register(&server.address, "a@x.com", "A").await;
    let a_id = registered_a.user_text("id");
    let buy = || object([("gameId", "mini".into()), ("setId", "s1".into())]);
    assert_eq!(a.request("packBuy", buy()).await.error(), "notCollection");

    let settings = object([("mode", "collection".into()), ("packCost", 2.into()), ("startingTokens", 3.into())]);
    let set = a.request("adminSettingsSet", object([("settings", settings)])).await;
    assert!(set.ok(), "{:?}", set.0);
    a.expect("serverInfo").await;
    assert!(a.request("adminUserUpdate", object([("id", a_id.as_str().into()), ("tokensSet", 5.into())])).await.ok());

    let pack = a.request("packBuy", buy()).await;
    assert!(pack.ok(), "{:?}", pack.0);
    let cards = texts_of(pack.get("cards"));
    assert_eq!(cards.len(), 3, "{cards:?}");
    assert_eq!(cards[2], "r1");
    assert_eq!(number(pack.get("tokens")), 3.0);
    let second = a.request("packBuy", buy()).await;
    for card in cards.iter().chain(texts_of(second.get("cards")).iter()) {
        assert!(card != "t1" && card != "l1", "unownable or free card in pack: {card}");
    }
    assert_eq!(a.request("packBuy", buy()).await.error(), "notEnoughTokens");

    let collection = a.request("collectionGet", object([("gameId", "mini".into())])).await;
    let owned = collection.get("cards").unwrap();
    assert_eq!(number(owned.get("r1")), 2.0);
    assert!(owned.get("l1").is_none());

    let game_list = a.request("gameList", empty()).await;
    let set_info = &game_list.get("games").unwrap().as_array().unwrap()[0].get("sets").unwrap().as_array().unwrap()[0];
    assert_eq!(number(set_info.get("cardCount")), 4.0);

    // New accounts get the starting balance.
    let (_b, registered_b) = TestClient::register(&server.address, "b@x.com", "B").await;
    assert_eq!(number(registered_b.get("user").and_then(|user| user.get("tokens"))), 3.0);

    // In a collection game, decks are checked and pack/library actions refused.
    a.request("roomCreate", object([("gameId", "mini".into()), ("maxPlayers", 1.into())])).await;
    a.request("roomStart", empty()).await;
    a.expect("fullState").await;
    let too_many = deck_of(&[("r1", 3), ("t1", 4)]);
    a.send("requestLoadDeckIntoZone", object([("zoneId", "mine".into()), ("deck", too_many)])).await;
    let toast = a.expect("toast").await;
    assert!(toast.contains("Rare One (2/3)"), "{}", toast.line);

    let ok_deck = deck_of(&[("r1", 2), ("t1", 4), ("l1", 10)]);
    a.send("requestLoadDeckIntoZone", object([("zoneId", "mine".into()), ("deck", ok_deck)])).await;
    let state = a.latest("fullState").await.expect("a fullState after loading the deck");
    let owned_cards = parse_full_state(&state).cards.iter().filter(|card| card.owner_id == a_id).count();
    assert_eq!(owned_cards, 16);

    a.send("requestGeneratePack", object([("setId", "s1".into())])).await;
    a.expect("toast").await;

    // The Card Library stays available in collection mode.
    a.send(
        "requestCreateCardFromLibrary",
        object([("definitionId", "c1".into()), ("x", 0.5.into()), ("y", 0.5.into())]),
    )
    .await;
    let state = a.latest("fullState").await.expect("a fullState after creating a card");
    assert!(parse_full_state(&state).cards.iter().any(|card| card.definition_id == "c1"));
}

fn texts_of(value: Option<&JsonValue>) -> Vec<String> {
    value
        .and_then(JsonValue::as_array)
        .map(|items| items.iter().filter_map(JsonValue::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn starter_decks() {
    let server = start_server().await;
    let decks_folder = server.data_folder.join("games").join("mini").join("decks");
    std::fs::create_dir_all(&decks_folder).unwrap();
    std::fs::write(
        decks_folder.join("Starter.json"),
        r#"{"gameId":"mini","subdecks":[
		{"name":"main_deck","entries":[{"definitionId":"c1","quantity":2},{"definitionId":"t1","quantity":3},{"definitionId":"l1","quantity":2}]},
		{"name":"side","entries":[{"definitionId":"c1","quantity":1},{"definitionId":"r1","quantity":1}]}],"isStarter":true}"#,
    )
    .unwrap();
    std::fs::write(decks_folder.join("Plain.json"), r#"{"gameId":"mini","subdecks":[]}"#).unwrap();
    std::fs::write(decks_folder.join("Other.json"), r#"{"gameId":"other","subdecks":[],"IsStarter":true}"#).unwrap();
    std::fs::write(decks_folder.join("Caps.json"), r#"{"gameId":"mini","subdecks":[],"IsStarter":true}"#).unwrap();
    server.server.library.write().unwrap().scan().unwrap();
    {
        let library = server.server.library.read().unwrap();
        let game = library.game("mini").unwrap();
        let names: Vec<&str> = game.starter_decks.iter().map(|deck| deck.name.as_str()).collect();
        assert_eq!(names, ["Caps", "Starter"]);
        let cards = &game.starter_deck("Starter").unwrap().card_quantities;
        assert_eq!((cards["c1"], cards["r1"], cards["t1"], cards["l1"]), (3, 1, 3, 2));
    }

    let (mut a, registered) = TestClient::register(&server.address, "a@x.com", "A").await;
    let a_id = registered.user_text("id");
    let game_list = a.request("gameList", empty()).await;
    let starters =
        game_list.get("games").unwrap().as_array().unwrap()[0].get("starterDecks").unwrap().as_array().unwrap().clone();
    assert_eq!(starters.len(), 2);
    assert_eq!(number(starters[1].get("cardCount")), 9.0);

    let buy = || object([("gameId", "mini".into()), ("name", "Starter".into())]);
    assert_eq!(a.request("starterBuy", buy()).await.error(), "notCollection");
    let settings = a.request("adminSettingsGet", empty()).await;
    assert_eq!(number(settings.get("settings").and_then(|settings| settings.get("starterDeckCost"))), 3.0);
    let new_settings = object([("mode", "collection".into()), ("starterDeckCost", 4.into())]);
    assert!(a.request("adminSettingsSet", object([("settings", new_settings)])).await.ok());
    let info = a.expect("serverInfo").await;
    assert!(info.contains(r#""starterDeckCost":4"#), "{}", info.line);
    a.request("adminUserUpdate", object([("id", a_id.as_str().into()), ("tokensSet", 10.into())])).await;
    let unknown = a.request("starterBuy", object([("gameId", "mini".into()), ("name", "Nope".into())])).await;
    assert_eq!(unknown.error(), "noStarter");

    let bought = a.request("starterBuy", buy()).await;
    assert!(bought.ok(), "{:?}", bought.0);
    assert_eq!(bought.text("savedName"), "Starter");
    assert_eq!(number(bought.get("tokens")), 6.0);
    let granted = bought.get("cards").unwrap();
    assert_eq!((number(granted.get("c1")), number(granted.get("r1"))), (3.0, 1.0));
    assert!(granted.get("t1").is_none() && granted.get("l1").is_none());
    let second = a.request("starterBuy", buy()).await;
    assert_eq!((second.text("savedName").as_str(), number(second.get("tokens"))), ("Starter 2", 2.0));
    assert_eq!(a.request("starterBuy", buy()).await.error(), "notEnoughTokens");

    let collection = a.request("collectionGet", object([("gameId", "mini".into())])).await;
    let owned = collection.get("cards").unwrap();
    assert_eq!((number(owned.get("c1")), number(owned.get("r1"))), (6.0, 2.0));
    assert!(owned.get("t1").is_none() && owned.get("l1").is_none());

    let list = a.request("deckList", object([("gameId", "mini".into()), ("full", true.into())])).await;
    let decks = list.get("decks").unwrap().as_array().unwrap();
    assert_eq!(decks.len(), 2);
    let saved = gametable_core::json::encode_compact(decks[0].get("deck").unwrap()).unwrap();
    assert!(!saved.to_lowercase().contains("isstarter") && saved.contains(r#""subdecks""#), "{saved}");
}

#[tokio::test(flavor = "multi_thread")]
async fn built_in_decks_prefer_underscore_folder() {
    let server = start_server().await;
    let folder = server.data_folder.join("games").join("mini");
    std::fs::create_dir_all(folder.join("decks")).unwrap();
    std::fs::write(folder.join("decks").join("Old.json"), r#"{"gameId":"mini","subdecks":[],"isStarter":true}"#)
        .unwrap();
    let starter_names = || {
        let mut library = server.server.library.write().unwrap();
        library.scan().unwrap();
        let game = library.game("mini").unwrap();
        let names: Vec<String> = game.starter_decks.iter().map(|deck| deck.name.clone()).collect();
        (names, game.built_in_deck("Old").is_some(), game.built_in_deck("New").is_some())
    };
    assert_eq!(starter_names(), (vec!["Old".to_string()], true, false));

    std::fs::create_dir_all(folder.join("_decks")).unwrap();
    std::fs::write(folder.join("_decks").join("New.json"), r#"{"gameId":"mini","subdecks":[],"isStarter":true}"#)
        .unwrap();
    assert_eq!(starter_names(), (vec!["New".to_string()], false, true));
}
