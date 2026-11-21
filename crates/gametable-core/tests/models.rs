//! Port of CardTable's `tests/models_test.lua` (model behaviour beyond plain round-tripping,
//! which `models_golden.rs` covers against the original's real output).

use gametable_core::json::{self, JsonValue};
use gametable_core::models::*;

fn compact(value: &JsonValue) -> String {
    json::encode_compact(value).unwrap()
}

fn decode(text: &str) -> JsonValue {
    json::decode(text).unwrap()
}

#[test]
fn card_definition_round_trips_all_fields_and_omits_defaults() {
    let card = CardDefinition {
        color_hex: Some("#FFFFFF".into()),
        suit: Some("s".into()),
        rank: Some("r".into()),
        image_path: Some("x.png".into()),
        extra_fields: json::JsonObject::new().with("k", "v"),
        types: vec!["A".into(), "B".into()],
        orientation: CardOrientation::Left,
        set_id: Some("s1".into()),
        unownable: true,
        is_free_card: true,
        card_back_id: Some(UNIQUE_CARD_BACK_ID.into()),
        unique_back_orientation: CardOrientation::Right,
        is_scenario_card: true,
        ..CardDefinition::new("x", "X")
    };
    assert_eq!(CardDefinition::from_json(&card.to_json(false)).unwrap(), card);
    assert_eq!(compact(&CardDefinition::new("y", "Y").to_json(false)), r#"{"id":"y","cardTitle":"Y"}"#);
}

#[test]
fn card_instance_writes_face_up_false_and_minimal_form() {
    let card = CardInstance {
        x: 0.25,
        y: 1.0,
        z_index: 3,
        face_up: false,
        zone_id: Some("deck".into()),
        owner_id: Some("p1".into()),
        stack_parent_id: Some("s".into()),
        rotation_turns: -2.0,
        unownable: true,
        ..CardInstance::new("i", "d", CardZone::Zone)
    };
    let encoded = compact(&card.to_json());
    assert_eq!(CardInstance::from_json(&decode(&encoded)).unwrap(), card);
    assert!(encoded.contains(r#""faceUp":false"#));
    let minimal = CardInstance { x: 0.0, y: 0.0, ..CardInstance::new("i", "d", CardZone::Table) };
    assert_eq!(
        compact(&minimal.to_json()),
        r#"{"instanceId":"i","definitionId":"d","x":0,"y":0,"zIndex":0,"faceUp":true,"zone":"table"}"#
    );
}

#[test]
fn board_widget_omits_defaults_and_round_trips_bag_fields() {
    let token = BoardWidgetInstance::new("w", BoardWidgetKind::Token, 0.0, 0.0, 0);
    assert_eq!(compact(&token.to_json()), r#"{"instanceId":"w","kind":"token","x":0,"y":0,"zIndex":0}"#);
    let flipped = BoardWidgetInstance { face_up: false, ..token.clone() };
    assert!(compact(&flipped.to_json()).contains(r#""faceUp":false"#));
    assert_eq!(BoardWidgetInstance::from_json(&flipped.to_json()).unwrap(), flipped);

    let bag = BoardWidgetInstance {
        size_override: Some(WidgetSizeOverride::NativeImageSize),
        bag_remaining: vec!["a.png".into(), "a.png".into(), "b.png".into()],
        bag_revealed: vec![RevealedToken::new("s1", "c.png")],
        ..BoardWidgetInstance::new("w", BoardWidgetKind::TokenBag, 0.0, 0.0, 0)
    };
    assert_eq!(BoardWidgetInstance::from_json(&bag.to_json()).unwrap(), bag);
    let empty_bag = BoardWidgetInstance::new("w", BoardWidgetKind::TokenBag, 0.0, 0.0, 0);
    assert!(!compact(&empty_bag.to_json()).contains("bagR"));
}

#[test]
fn zone_definition_defaults_and_field_order() {
    let zone = ZoneDefinition::from_json(&decode(r#"{"id":"deck","name":"Deck"}"#)).unwrap();
    assert!(!zone.shared && zone.shuffleable);
    assert_eq!(zone.deck_type, "main_deck");
    assert_eq!(zone.counter_starting_color, 0xFF455A64);
    assert_eq!(compact(&zone.to_json()), r#"{"id":"deck","name":"Deck"}"#);

    let changed = ZoneDefinition {
        shuffleable: false,
        shared: true,
        side: SharedZoneSide::Right,
        entries: vec![DeckEntry::new("c1", 2)],
        ..zone.clone()
    };
    assert_eq!(
        compact(&changed.to_json()),
        r#"{"id":"deck","name":"Deck","shared":true,"entries":[{"definitionId":"c1","quantity":2}],"shuffleable":false,"side":"right"}"#
    );
    let turned = ZoneDefinition { side: SharedZoneSide::Right, orientation: CardOrientation::Left, ..zone };
    assert_eq!(compact(&turned.to_json()), r#"{"id":"deck","name":"Deck","side":"right","orientation":"left"}"#);
}

#[test]
fn playmat_zone_placements_round_trip_and_are_omitted_when_empty() {
    let plain = GameBoardDefinition::from_json(&decode(r#"{"imagePath":"mat.png"}"#)).unwrap();
    assert!(plain.zone_placements.is_empty());
    assert_eq!(compact(&plain.to_json()), r#"{"imagePath":"mat.png"}"#);

    let text = r#"{"imagePath":"mat.png","scale":0.5,"zonePlacements":[{"zoneId":"deck","x":0.25,"y":0.75}]}"#;
    let placed = GameBoardDefinition::from_json(&decode(text)).unwrap();
    assert_eq!(placed.zone_placements, vec![BoardZonePlacement::new("deck", 0.25, 0.75)]);
    assert_eq!(placed.placement_for("deck").map(|placement| placement.x), Some(0.25));
    assert!(placed.placement_for("discard").is_none());
    assert_eq!(compact(&placed.to_json()), text);
}

#[test]
fn zones_fit_board_kinds_by_ownership() {
    let owned = ZoneDefinition::new("deck", "Deck");
    let shared = ZoneDefinition { shared: true, ..owned.clone() };
    assert!(zone_fits_board_kind(&owned, BoardKind::Player));
    assert!(!zone_fits_board_kind(&owned, BoardKind::Shared));
    assert!(zone_fits_board_kind(&shared, BoardKind::Shared));
    assert!(!zone_fits_board_kind(&shared, BoardKind::Player));
}

#[test]
fn game_boards_kind_and_table_placement_round_trip_in_field_order() {
    let text = r#"{"imagePath":"board.png","scale":0.5,"kind":"shared","zonePlacements":[{"zoneId":"deck","x":0.25,"y":0.75}],"tablePlacement":{"x":1280,"y":300,"rotation":90,"toggleable":true,"unlockedFromCenter":true}}"#;
    let board = GameBoardDefinition::from_json(&decode(text)).unwrap();
    assert_eq!(board.kind, BoardKind::Shared);
    assert_eq!(
        board.table_placement,
        Some(BoardTablePlacement { x: 1280.0, y: 300.0, rotation: 90, toggleable: true, unlocked_from_center: true })
    );
    assert!(board.is_toggleable_on_table());
    assert_eq!(compact(&board.to_json()), text);

    // Defaults are left out: a player board, unrotated and always shown.
    let fixed = GameBoardDefinition {
        table_placement: Some(BoardTablePlacement::new(0.0, -200.0)),
        ..GameBoardDefinition::new("mat.png")
    };
    assert!(!fixed.is_toggleable_on_table());
    assert_eq!(compact(&fixed.to_json()), r#"{"imagePath":"mat.png","tablePlacement":{"x":0,"y":-200}}"#);
}

#[test]
fn player_placements_round_trip_and_replace_the_every_seat_copy_for_their_player() {
    let text = r#"{"imagePath":"mat.png","tablePlacement":{"x":0,"y":200},"playerPlacements":[{"player":2,"x":400,"y":300,"rotation":90,"toggleable":true},{"player":3,"x":2000,"y":1100}]}"#;
    let board = GameBoardDefinition::from_json(&decode(text)).unwrap();
    assert_eq!(
        board.player_placements,
        vec![
            BoardPlayerPlacement { rotation: 90, toggleable: true, ..BoardPlayerPlacement::new(2, 400.0, 300.0) },
            BoardPlayerPlacement::new(3, 2000.0, 1100.0),
        ]
    );
    assert_eq!(compact(&board.to_json()), text);
    assert!(board.is_on_table() && board.is_toggleable_on_table());
    // Player 1 gets the every-seat copy; players 2 and 3 their own placements.
    assert_eq!(board.toggleable_for_player(1), Some(false));
    assert_eq!(board.toggleable_for_player(2), Some(true));
    assert_eq!(board.toggleable_for_player(3), Some(false));

    // Placed for one player only: nobody else gets it.
    let only_two = GameBoardDefinition {
        player_placements: vec![BoardPlayerPlacement::new(2, 400.0, 300.0)],
        ..GameBoardDefinition::new("mat.png")
    };
    assert_eq!(only_two.toggleable_for_player(1), None);
    assert!(only_two.is_on_table());
}

#[test]
fn board_rotation_snaps_to_quarter_turns() {
    assert_eq!(normalized_rotation(0), 0);
    assert_eq!(normalized_rotation(-90), 270);
    assert_eq!(normalized_rotation(450), 90);
    assert_eq!(normalized_rotation(100), 90);
    let mut placement = BoardTablePlacement::new(0.0, 0.0);
    for expected in [90, 180, 270, 0] {
        placement.rotate_clockwise();
        assert_eq!(placement.rotation, expected);
    }
}

#[test]
fn old_playmats_load_as_game_boards_and_save_as_game_boards() {
    let old = r#"{"id":"g","name":"G","cards":[],"playmatType":"shared","playmats":[{"imagePath":"a.png"},{"imagePath":"b.png","scale":2}]}"#;
    let game = GameDefinition::from_json(&decode(old)).unwrap();
    assert_eq!(game.game_boards.len(), 2);
    assert!(game.game_boards.iter().all(|board| board.kind == BoardKind::Shared));
    assert_eq!(
        compact(&game.to_json()),
        r#"{"id":"g","name":"G","cards":[],"gameBoards":[{"imagePath":"a.png","kind":"shared"},{"imagePath":"b.png","scale":2,"kind":"shared"}]}"#
    );

    // Individual mats were player boards (the default kind, so it isn't written).
    let individual = r#"{"id":"g","name":"G","cards":[],"playmats":[{"imagePath":"a.png"}]}"#;
    let game = GameDefinition::from_json(&decode(individual)).unwrap();
    assert_eq!(game.game_boards[0].kind, BoardKind::Player);
    assert_eq!(compact(&game.to_json()), r#"{"id":"g","name":"G","cards":[],"gameBoards":[{"imagePath":"a.png"}]}"#);

    // gameBoards wins over a leftover playmats list.
    let both = r#"{"id":"g","name":"G","cards":[],"playmats":[{"imagePath":"old.png"}],"gameBoards":[{"imagePath":"new.png"}]}"#;
    let game = GameDefinition::from_json(&decode(both)).unwrap();
    assert_eq!(game.game_boards.len(), 1);
    assert_eq!(game.game_boards[0].image_path, "new.png");
}

#[test]
fn scenario_placements_omit_defaults() {
    let placement = ScenarioPlacement::new("c1", 0.25, 0.75);
    assert_eq!(compact(&placement.to_json()), r#"{"definitionId":"c1","x":0.25,"y":0.75}"#);
    let face_down = ScenarioPlacement { face_up: false, ..placement };
    assert_eq!(compact(&face_down.to_json()), r#"{"definitionId":"c1","x":0.25,"y":0.75,"faceUp":false}"#);
    let token = ScenarioTokenPlacement::new("clue.png", 0.3, 0.4);
    assert_eq!(compact(&token.to_json()), r#"{"imagePath":"clue.png","x":0.3,"y":0.4}"#);
}

#[test]
fn scenario_zones_set_on_the_table_round_trip() {
    let placement = ScenarioZonePlacement::new("boss_deck_zone", 0.5, 0.25);
    assert_eq!(compact(&placement.to_json()), r#"{"zoneId":"boss_deck_zone","x":0.5,"y":0.25}"#);
    let scenario = Scenario { zone_placements: vec![placement.clone()], ..Scenario::new("scen1", "Opening") };
    let json = compact(&scenario.to_json());
    assert!(json.contains(r#""zonePlacements":[{"zoneId":"boss_deck_zone","x":0.5,"y":0.25}]"#), "{json}");
    assert_eq!(Scenario::from_json(&scenario.to_json()).unwrap(), scenario);
    assert_eq!(scenario.zone_placement_for("boss_deck_zone"), Some(&placement));
    // Without any, the key is left out (files stay as the original writes them).
    assert!(!compact(&Scenario::new("scen1", "Opening").to_json()).contains("zonePlacements"));
}

#[test]
fn scenario_subdeck_lookup() {
    let deck = SubDeck::new("boss_deck", vec![DeckEntry::new("c3", 2)]);
    let scenario = Scenario { decks: vec![deck.clone()], ..Scenario::new("scen1", "Opening") };
    assert_eq!(scenario.subdeck("boss_deck"), Some(&deck));
    assert_eq!(scenario.entries_for("boss_deck"), deck.entries.as_slice());
    assert!(scenario.entries_for("missing").is_empty());
}

#[test]
fn token_names_split_into_display_name_and_type() {
    let token = |name: &str| TokenDefinition { id: name.into(), name: name.into(), image_path: None };
    assert_eq!(token("Daisy Walker|Investigator").display_name(), "Daisy Walker");
    assert_eq!(token("Daisy Walker|Investigator").token_type(), Some("Investigator"));
    assert_eq!(token("Clue").display_name(), "Clue");
    assert_eq!(token("Clue").token_type(), None);
    assert_eq!(token("Resource | Supply ").display_name(), "Resource");
    assert_eq!(token("Resource | Supply ").token_type(), Some("Supply"));
    assert_eq!(token("Doom|").token_type(), None);

    let mut game = GameDefinition::new("g", "G");
    game.tokens = vec![
        token("Daisy Walker|Investigator"),
        token("Clue"),
        token("Roland Banks|Investigator"),
        token("Doom|Treachery"),
    ];
    assert_eq!(game.token_types(), vec!["Investigator", "Treachery"]);
}

#[test]
fn game_definition_nested_sets_round_trip_exactly() {
    let source = concat!(
        r#"{"id":"g","name":"G","sets":[{"id":"s1","name":"S1","cards":[{"id":"a","cardTitle":"A"}]},"#,
        r#"{"id":"s2","name":"S2","packSettings":[{"tag":"Rare","count":1}],"cards":[{"id":"b","cardTitle":"B"}]}]}"#
    );
    let game = GameDefinition::from_json(&decode(source)).unwrap();
    assert_eq!(game.cards()[0].set_id.as_deref(), Some("s1"));
    assert_eq!(game.cards()[1].set_id.as_deref(), Some("s2"));
    assert_eq!(game.sets[1].pack_settings[0].tag, "Rare");
    assert_eq!(compact(&game.to_json()), source);
}

#[test]
fn game_definition_legacy_card_back_and_deck_types() {
    let game = GameDefinition::from_json(&decode(concat!(
        r#"{"id":"g","name":"G","cards":[],"cardBackImagePath":"b.png","zones":["#,
        r#"{"id":"z1","name":"Z1","dealsBuiltDeck":true},{"id":"z2","name":"Z2","dealsBuiltDeck":true,"deckType":"side"},"#,
        r#"{"id":"z3","name":"Z3","dealsBuiltDeck":true},{"id":"z4","name":"Z4","shared":true,"dealsBuiltDeck":true,"deckType":"x"}]}"#
    )))
    .unwrap();
    assert_eq!(game.card_backs[0].id, "default");
    assert_eq!(game.card_backs[0].image_path.as_deref(), Some("b.png"));
    assert_eq!(game.deck_types(), vec!["main_deck", "side"]);
    assert!(game.needs_deck_building());
    let encoded = compact(&game.to_json());
    assert!(encoded.contains(r#""cards":[]"#));
    assert!(encoded.contains(r#""cardBacks":[{"id":"default","name":"Default","imagePath":"b.png"}]"#));
    assert!(!encoded.contains("cardBackImagePath"));
}

#[test]
fn game_definition_card_lookup_follows_edits() {
    let mut game = GameDefinition::new("g", "G")
        .with_cards(vec![CardDefinition::new("a", "First A"), CardDefinition::new("a", "Second A")]);
    assert_eq!(game.card_by_id("a").unwrap().card_title, "First A");
    game.cards_mut().remove(0);
    assert_eq!(game.card_by_id("a").unwrap().card_title, "Second A");
    assert!(game.card_by_id("missing").is_none());
}

#[test]
fn bundled_standard_52_is_a_byte_stable_fixed_point() {
    let raw =
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/games/standard_52/standard_52.json")).unwrap();
    let game = GameDefinition::from_json(&json::decode_bytes(&raw).unwrap()).unwrap();
    let written = json::encode_indented(&game.to_json()).unwrap();
    let reread = GameDefinition::from_json(&decode(&written)).unwrap();
    assert_eq!(reread, game);
    assert_eq!(json::encode_indented(&reread.to_json()).unwrap(), written);
}

#[test]
fn deck_config_round_trip_legacy_error_and_helpers() {
    let deck = DeckConfig::new("g", vec![SubDeck::new("main_deck", vec![DeckEntry::new("a", 2)])]);
    assert_eq!(DeckConfig::from_json(&deck.to_json()).unwrap(), deck);
    assert_eq!(DeckConfig::from_json(&decode(r#"{"gameId":"g","entries":[]}"#)), Err(ModelError::LegacyDeckFormat));
    assert_eq!(deck.card_count_for("main_deck"), 2);
    assert_eq!(deck.card_count_for("side"), 0);
    let game = GameDefinition::new("g", "G").with_cards(vec![CardDefinition::new("a", "A")]);
    let full = DeckConfig::full(&game);
    assert_eq!(full.subdecks[0].name, "main_deck");
    assert_eq!(full.subdecks[0].entries[0], DeckEntry::new("a", 1));
}

#[test]
fn deck_config_is_starter_is_optional_and_written_only_when_true() {
    let plain = r#"{"gameId":"g","subdecks":[{"name":"main_deck","entries":[{"definitionId":"a","quantity":2}]}]}"#;
    let deck = DeckConfig::from_json(&decode(plain)).unwrap();
    assert!(!deck.is_starter);
    assert_eq!(compact(&deck.to_json()), plain);
    let starter = DeckConfig::from_json(&decode(r#"{"gameId":"g","subdecks":[],"isStarter":true}"#)).unwrap();
    assert_eq!(compact(&starter.to_json()), r#"{"gameId":"g","subdecks":[],"isStarter":true}"#);
    assert!(DeckConfig::from_json(&decode(r#"{"gameId":"g","subdecks":[],"IsStarter":true}"#)).unwrap().is_starter);
}

#[test]
fn table_state_round_trip_and_minimal_form() {
    let mut player = PlayerInfo::new("p", "P", PlayerRole::Host, 0xFFD32F2F);
    player.connected = false;
    let mut state = TableState::new("g");
    state.revision = 7;
    state.players = vec![player];
    state.searches = vec![ActiveSearch {
        searcher_id: "p".into(),
        target_type: SearchTargetType::Pile,
        target_id: "c".into(),
        target_owner_id: None,
    }];
    state.log = vec![LogEntry::new(1500, "P drew a card.")];
    state.current_player_id = Some("p".into());
    let state = state
        .with_cards(vec![CardInstance { owner_id: Some("p".into()), ..CardInstance::new("c", "d", CardZone::Hand) }])
        .with_widgets(vec![BoardWidgetInstance::new("w", BoardWidgetKind::SimpleCounter, 0.0, 0.0, 0)]);
    assert_eq!(TableState::from_json(&state.to_json()).unwrap(), state);
    let saved = SavedGame { saved_at: "2026-01-02T03:04:05.678".into(), state };
    assert_eq!(SavedGame::from_json(&saved.to_json()).unwrap(), saved);
    assert_eq!(compact(&TableState::new("g").to_json()), r#"{"gameId":"g","players":[],"cards":[],"revision":0}"#);
}

#[test]
fn table_state_lookups_follow_mutation() {
    let mut state = TableState::new("g")
        .with_cards(vec![CardInstance::new("c1", "d", CardZone::Table), CardInstance::new("c2", "d", CardZone::Table)]);
    assert_eq!(state.card_by_id("c1").unwrap().x, 0.5);
    state.cards_mut()[0].x = 99.0;
    assert_eq!(state.card_by_id("c1").unwrap().x, 99.0);
    state.cards_mut().remove(0);
    assert!(state.card_by_id("c1").is_none());
    assert_eq!(state.card_by_id("c2").unwrap().instance_id, "c2");
}

#[test]
fn standard_deck_cards() {
    let cards = build_standard_deck_cards();
    assert_eq!(cards.len(), 52);
    assert_eq!(cards[0].id, "__standard52_hearts_A");
    assert_eq!(cards[51].types, vec!["Spades"]);
    assert_eq!(cards[26].color_hex.as_deref(), Some("#212121"));
}
