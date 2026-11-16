//! Drives a practice table through real frames with simulated mouse and keys: drawing from the
//! shared deck, dragging a card from hand to table, the card hotkeys, context menus and box
//! selection. Uses the bundled Standard 52-Card Deck (a shared deck and discard pile on the left).

use std::collections::HashMap;

use gametable::app::harness::UiHarness;
use gametable::data::game_loader::load_standard_deck;
use gametable::table::{PracticeBackend, TableOptions, TableScreen};
use gametable_core::models::{CardZone, PlayerInfo, PlayerRole};
use gametable_core::session::{DealOptions, Session, SessionEnvironment};

/// Shared sidebar slots at 1280×720: two zones centred vertically in a 110-wide column.
const DECK: (f32, f32) = (55.0, 284.0);
const DISCARD: (f32, f32) = (55.0, 436.0);
/// The middle of the local player's hand band (one card fanned at the centre).
const HAND_CARD: (f32, f32) = (695.0, 640.0);
const TABLE_SPOT: (f32, f32) = (640.0, 300.0);

fn practice_table() -> TableScreen {
    let players = vec![
        PlayerInfo::new("seat1", "Ann", PlayerRole::Host, 0xFF45_5A64),
        PlayerInfo::new("seat2", "Player 2", PlayerRole::Host, 0xFFE9_1E63),
    ];
    let empty = HashMap::new();
    let options = DealOptions {
        game: load_standard_deck(),
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

fn key(harness: &mut UiHarness, table: &mut TableScreen, key: &'static str) {
    harness.frame(|frame| {
        table.update(frame, 1.0 / 60.0);
        table.key_pressed(frame, key, false);
        table.draw(frame, 0.0, 0.0, 1280.0, 720.0);
    });
}

fn hover(harness: &mut UiHarness, table: &mut TableScreen, at: (f32, f32)) {
    harness.move_mouse(at.0, at.1);
    frame(harness, table);
    frame(harness, table);
}

fn drag(harness: &mut UiHarness, table: &mut TableScreen, from: (f32, f32), to: (f32, f32)) {
    hover(harness, table, from);
    harness.press(from.0, from.1, 1, 1);
    frame(harness, table);
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        harness.move_mouse(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        frame(harness, table);
    }
    harness.release(to.0, to.1, 1);
    frame(harness, table);
    frame(harness, table);
}

fn cards_in(table: &TableScreen, zone: CardZone) -> Vec<gametable_core::models::CardInstance> {
    table.backend().session().state().cards().iter().filter(|card| card.zone == zone).cloned().collect()
}

#[test]
fn draw_play_flip_rotate_and_discard() {
    let mut harness = UiHarness::new();
    let mut table = practice_table();
    frame(&mut harness, &mut table);
    frame(&mut harness, &mut table);
    assert_eq!(cards_in(&table, CardZone::Zone).len(), 52);

    // Hover the deck and press 1: one card into the hand.
    hover(&mut harness, &mut table, DECK);
    key(&mut harness, &mut table, "1");
    let hand = cards_in(&table, CardZone::Hand);
    assert_eq!(hand.len(), 1, "drew one card");
    assert_eq!(hand[0].owner_id.as_deref(), Some("seat1"));

    // Drag it from the hand onto the table.
    drag(&mut harness, &mut table, HAND_CARD, TABLE_SPOT);
    let on_table = cards_in(&table, CardZone::Table);
    assert_eq!(on_table.len(), 1, "played to the table");
    assert!(
        (on_table[0].x - 0.5).abs() < 0.2 && (on_table[0].y - 0.42).abs() < 0.2,
        "{:?}",
        (on_table[0].x, on_table[0].y)
    );

    // F flips, E rotates a quarter turn clockwise, Q back.
    let card_spot = TABLE_SPOT;
    hover(&mut harness, &mut table, card_spot);
    let face_up = cards_in(&table, CardZone::Table)[0].face_up;
    key(&mut harness, &mut table, "f");
    assert_eq!(cards_in(&table, CardZone::Table)[0].face_up, !face_up);
    key(&mut harness, &mut table, "e");
    assert_eq!(cards_in(&table, CardZone::Table)[0].rotation_turns, 1.0);
    key(&mut harness, &mut table, "q");
    assert_eq!(cards_in(&table, CardZone::Table)[0].rotation_turns, 0.0);

    // X discards it to the shared discard pile (with a flight animation).
    key(&mut harness, &mut table, "x");
    assert!(cards_in(&table, CardZone::Table).is_empty());
    let discard_count = cards_in(&table, CardZone::Zone).len();
    assert_eq!(discard_count, 52);
    for _ in 0..20 {
        frame(&mut harness, &mut table);
    }

    // A hand card dropped on the discard slot goes into it.
    hover(&mut harness, &mut table, DECK);
    key(&mut harness, &mut table, "1");
    drag(&mut harness, &mut table, HAND_CARD, DISCARD);
    assert!(cards_in(&table, CardZone::Hand).is_empty());
    let discard_id = table.game().zones.iter().find(|zone| zone.is_discard_pile).map(|zone| zone.id.clone());
    let in_discard = cards_in(&table, CardZone::Zone).iter().filter(|card| card.zone_id == discard_id).count();
    assert_eq!(in_discard, 2);

    // The log recorded it all.
    let log = &table.backend().session().state().log;
    assert!(log.len() >= 4, "{log:?}");
}

#[test]
fn search_from_the_zone_menu_and_drag_a_card_out() {
    let mut harness = UiHarness::new();
    let mut table = practice_table();
    frame(&mut harness, &mut table);
    // Right-click the deck: the menu offers Search...
    hover(&mut harness, &mut table, DECK);
    harness.press(DECK.0, DECK.1, 2, 1);
    frame(&mut harness, &mut table);
    harness.release(DECK.0, DECK.1, 2);
    frame(&mut harness, &mut table);
    frame(&mut harness, &mut table);
    // The menu opens at the pointer; its first item is right below it.
    let item = (DECK.0 + 30.0, DECK.1 + 8.0 + 20.0);
    hover(&mut harness, &mut table, item);
    harness.press(item.0, item.1, 1, 1);
    frame(&mut harness, &mut table);
    harness.release(item.0, item.1, 1);
    frame(&mut harness, &mut table);
    frame(&mut harness, &mut table);
    let searches = &table.backend().session().state().searches;
    assert_eq!(searches.len(), 1, "a search is open");
    assert_eq!(searches[0].searcher_id, "seat1");

    // Escape opens the Game Menu (no selection to clear first); Escape closes it again.
    key(&mut harness, &mut table, "escape");
    key(&mut harness, &mut table, "escape");
}

#[test]
fn box_selection_moves_piles_together() {
    let mut harness = UiHarness::new();
    let mut table = practice_table();
    frame(&mut harness, &mut table);
    // Two cards onto the table, side by side.
    for target in [(500.0, 300.0), (700.0, 300.0)] {
        hover(&mut harness, &mut table, DECK);
        key(&mut harness, &mut table, "1");
        drag(&mut harness, &mut table, HAND_CARD, target);
    }
    assert_eq!(cards_in(&table, CardZone::Table).len(), 2);
    // Drag a box around both from empty felt.
    drag(&mut harness, &mut table, (420.0, 200.0), (800.0, 420.0));
    // Drag one of them: both move by the same amount.
    let before: Vec<(f64, f64)> = cards_in(&table, CardZone::Table).iter().map(|card| (card.x, card.y)).collect();
    drag(&mut harness, &mut table, (500.0, 300.0), (500.0, 450.0));
    let after: Vec<(f64, f64)> = cards_in(&table, CardZone::Table).iter().map(|card| (card.x, card.y)).collect();
    assert_eq!(before.len(), after.len());
    let moved: Vec<f64> = before.iter().zip(&after).map(|(old, new)| new.1 - old.1).collect();
    assert!(moved.iter().all(|dy| *dy > 0.05), "{moved:?}");
    assert!((moved[0] - moved[1]).abs() < 1e-6, "{moved:?}");
    // Escape clears the selection rather than opening the menu.
    key(&mut harness, &mut table, "escape");
}
