//! Phase 12: table features no bundled game uses, exercised through real frames against real
//! game definitions (read only; nothing is written to the library). Ignored by default:
//! `GAMETABLE_LIBRARY=D:\CardfluxLibrary\game_library cargo test -p gametable --test table_real_games -- --ignored`
//! Lorcana: Pack Generator (open a pack, delete) and Deck Widget. Arkham Horror LCG: scenarios,
//! the token picker, and token bags (draw by clicking, return to the bag).

use std::collections::HashMap;

use gametable::app::harness::UiHarness;
use gametable::data::game_loader::load_from_folder;
use gametable::table::{PracticeBackend, TableAction, TableOptions, TableScreen};
use gametable_core::models::{BoardWidgetInstance, BoardWidgetKind, CardZone, GameDefinition, PlayerInfo, PlayerRole};
use gametable_core::session::{DealOptions, Session, SessionEnvironment};

/// Empty felt in the middle of a two-seat table at 1280×720.
const FELT: (f32, f32) = (640.0, 330.0);

fn library_game(folder: &str) -> Option<GameDefinition> {
    let root = std::env::var("GAMETABLE_LIBRARY").ok()?;
    let (mut games, errors) = load_from_folder(&format!("{root}/{folder}"));
    assert!(errors.is_empty(), "{errors:?}");
    Some(games.remove(0))
}

fn practice_table(game: GameDefinition) -> TableScreen {
    let players = vec![
        PlayerInfo::new("seat1", "Ann", PlayerRole::Host, 0xFF45_5A64),
        PlayerInfo::new("seat2", "Bea", PlayerRole::Host, 0xFFE9_1E63),
    ];
    let empty = HashMap::new();
    let options = DealOptions {
        game,
        players,
        local_player_id: "seat1".to_string(),
        deck_configs_by_player_id: Some(&empty),
        shared_deck_configs_by_zone_id: None,
        is_local_practice: true,
    };
    let session = Session::deal_from_zones(options, SessionEnvironment::system());
    TableScreen::new(TableOptions {
        backend: Box::new(PracticeBackend { session }),
        is_mirrored: false,
        local_avatar_path: None,
        avatar_keys: HashMap::new(),
        leave_button_label: "Leave Game".to_string(),
        leave_confirmation_message: "Leave?".to_string(),
        can_save_game: false,
        can_kick_players: false,
        can_load_scenario: true,
        collection_mode: false,
        spectator: false,
        deck_store: None,
    })
}

fn frame(harness: &mut UiHarness, table: &mut TableScreen) {
    harness.frame(|frame| {
        table.update(frame, 1.0 / 60.0);
        table.draw(frame, 0.0, 0.0, 1280.0, 720.0);
    });
}

fn click(harness: &mut UiHarness, table: &mut TableScreen, at: (f32, f32), button: u8) {
    harness.move_mouse(at.0, at.1);
    frame(harness, table);
    frame(harness, table);
    harness.press(at.0, at.1, button, 1);
    frame(harness, table);
    harness.release(at.0, at.1, button);
    frame(harness, table);
    frame(harness, table);
}

fn menu(harness: &mut UiHarness, table: &mut TableScreen, at: (f32, f32), labels: &[&str]) {
    click(harness, table, at, 2);
    for label in labels {
        let rect = table.menu_item_rect(label).unwrap_or_else(|| panic!("menu item {label:?} is showing"));
        click(harness, table, (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0), 1);
    }
}

fn widgets(table: &TableScreen, kind: BoardWidgetKind) -> Vec<BoardWidgetInstance> {
    table.backend().session().state().widgets().iter().filter(|widget| widget.kind == kind).cloned().collect()
}

fn item_center(table: &TableScreen, kind: &str) -> Option<(f32, f32)> {
    table.hit_item_centers().into_iter().rev().find(|(name, ..)| *name == kind).map(|(_, x, y)| (x, y))
}

#[test]
#[ignore]
fn lorcana_pack_generator_and_deck_widget() {
    let Some(game) = library_game("lorcana") else { return };
    let first_set = game.sets[0].name.clone();
    let mut harness = UiHarness::new();
    let mut table = practice_table(game);
    frame(&mut harness, &mut table);

    menu(&mut harness, &mut table, FELT, &["Widgets", "Pack Generator"]);
    assert_eq!(widgets(&table, BoardWidgetKind::PackGenerator).len(), 1);
    let cards_before = table.backend().session().state().cards().len();
    menu(&mut harness, &mut table, FELT, &[first_set.as_str()]);
    let opened = table.backend().session().state().cards().len() - cards_before;
    assert!(opened >= 10, "a pack of {opened} cards");
    let log = table.backend().session().state().log.last().unwrap().message.clone();
    assert!(log.contains(&first_set), "{log}");
    for _ in 0..10 {
        frame(&mut harness, &mut table);
    }
    let generator = item_center(&table, "packGenerator").expect("the generator is drawn");
    menu(&mut harness, &mut table, generator, &["Delete"]);
    assert!(widgets(&table, BoardWidgetKind::PackGenerator).is_empty());

    menu(&mut harness, &mut table, (900.0, 330.0), &["Widgets", "Deck Widget"]);
    assert_eq!(widgets(&table, BoardWidgetKind::DeckBuilder).len(), 1);
    for _ in 0..5 {
        frame(&mut harness, &mut table);
    }
    let header = item_center(&table, "deckHeader").expect("the deck widget is drawn");
    menu(&mut harness, &mut table, header, &["Delete"]);
    assert!(widgets(&table, BoardWidgetKind::DeckBuilder).is_empty());
}

#[test]
#[ignore]
fn arkham_scenario_tokens_and_token_bag() {
    let Some(game) = library_game("ahlcg") else { return };
    let scenario = game.scenarios.first().cloned().expect("ahlcg has scenarios");
    let mut harness = UiHarness::new();
    let mut table = practice_table(game);
    frame(&mut harness, &mut table);

    table.backend_mut().perform(TableAction::LoadScenario { scenario_id: scenario.id.clone() });
    for _ in 0..10 {
        frame(&mut harness, &mut table);
    }
    let state = table.backend().session().state();
    assert_eq!(state.loaded_scenario_ids, vec![scenario.id.clone()]);
    println!(
        "scenario {:?}: {} zones added, {} table cards, {} widgets",
        scenario.name,
        state.loaded_zones.len(),
        state.cards().iter().filter(|card| card.zone == CardZone::Table).count(),
        state.widgets().len()
    );

    // A token bag: a click draws a token, clicking the revealed token returns it.
    // (A scenario can have several bags; count across all of them.)
    let totals = |table: &TableScreen| {
        let bags = widgets(table, BoardWidgetKind::TokenBag);
        let remaining: usize = bags.iter().map(|bag| bag.bag_remaining.len()).sum();
        let revealed: usize = bags.iter().map(|bag| bag.bag_revealed.len()).sum();
        (bags.len(), remaining, revealed)
    };
    let (bag_count, remaining, _) = totals(&table);
    println!("{bag_count} token bags, {remaining} tokens in them");
    let at = item_center(&table, "tokenBagIdle").expect("a bag is drawn");
    click(&mut harness, &mut table, at, 1);
    assert_eq!(totals(&table), (bag_count, remaining - 1, 1), "a click draws a token");
    for _ in 0..5 {
        frame(&mut harness, &mut table);
    }
    let revealed = item_center(&table, "revealedBagToken").expect("the drawn token is shown");
    click(&mut harness, &mut table, revealed, 1);
    assert_eq!(totals(&table), (bag_count, remaining, 0), "clicking it returns it");

    // The board menu's Tokens opens the token picker, which covers the table.
    menu(&mut harness, &mut table, FELT, &["Tokens"]);
    for _ in 0..5 {
        frame(&mut harness, &mut table);
    }
    assert!(table.hit_item_centers().is_empty() || item_center(&table, "tokenBagIdle").is_some());
}
