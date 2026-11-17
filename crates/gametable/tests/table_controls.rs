//! Phase 12 parity walk through `CONTROLS.md`'s game-table section, driven through real frames
//! with simulated mouse and keys on a two-seat Standard 52 practice table: piles (click to draw,
//! drag to peel, Alt+drag to move or deal into the hand), the board menu's widgets (Simple
//! Counter double-click halves and menu), Ctrl+click and Shift+box selection with Create Pile
//! Here, Tab arrows, Z, and Give to.

use std::collections::HashMap;

use gametable::app::harness::UiHarness;
use gametable::data::game_loader::load_standard_deck;
use gametable::table::{PracticeBackend, TableAction, TableOptions, TableScreen};
use gametable_core::models::{BoardWidgetInstance, BoardWidgetKind, CardInstance, CardZone, PlayerInfo, PlayerRole};
use gametable_core::session::{DealOptions, Session, SessionEnvironment};

/// The shared deck in the left sidebar at 1280×720.
const DECK: (f32, f32) = (55.0, 284.0);
/// The middle of the local hand band (a single card fans at the centre).
const HAND_CARD: (f32, f32) = (695.0, 640.0);
const PILE_SPOT: (f32, f32) = (640.0, 300.0);

fn practice_table() -> TableScreen {
    let players = vec![
        PlayerInfo::new("seat1", "Ann", PlayerRole::Host, 0xFF45_5A64),
        PlayerInfo::new("seat2", "Bea", PlayerRole::Host, 0xFFE9_1E63),
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

fn click(harness: &mut UiHarness, table: &mut TableScreen, at: (f32, f32), button: u8, clicks: u8) {
    hover(harness, table, at);
    harness.press(at.0, at.1, button, clicks);
    frame(harness, table);
    harness.release(at.0, at.1, button);
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

/// Right-clicks `at`, then clicks each label in turn (submenus open on hover).
fn menu(harness: &mut UiHarness, table: &mut TableScreen, at: (f32, f32), labels: &[&str]) {
    click(harness, table, at, 2, 1);
    for label in labels {
        let rect = table.menu_item_rect(label).unwrap_or_else(|| panic!("menu item {label:?} is showing"));
        click(harness, table, (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0), 1, 1);
    }
}

fn cards_in(table: &TableScreen, zone: CardZone) -> Vec<CardInstance> {
    table.backend().session().state().cards().iter().filter(|card| card.zone == zone).cloned().collect()
}

fn widgets(table: &TableScreen, kind: BoardWidgetKind) -> Vec<BoardWidgetInstance> {
    table.backend().session().state().widgets().iter().filter(|widget| widget.kind == kind).cloned().collect()
}

/// Draws one card from the shared deck and drops it at `at` (Alt held: stacks onto a card there).
fn play_card(harness: &mut UiHarness, table: &mut TableScreen, at: (f32, f32), with_alt: bool) {
    hover(harness, table, DECK);
    key(harness, table, "1");
    if with_alt {
        harness.hold("lalt");
    }
    drag(harness, table, HAND_CARD, at);
    harness.let_go("lalt");
}

/// Plays three cards stacked into one pile at [`PILE_SPOT`].
fn build_pile(harness: &mut UiHarness, table: &mut TableScreen) {
    play_card(harness, table, PILE_SPOT, false);
    play_card(harness, table, PILE_SPOT, true);
    play_card(harness, table, PILE_SPOT, true);
    let on_table = cards_in(table, CardZone::Table);
    assert_eq!(on_table.len(), 3);
    assert_eq!(on_table.iter().filter(|card| card.stack_parent_id.is_some()).count(), 2, "Alt+drop stacks: {on_table:?}");
}

#[test]
fn clicking_a_pile_draws_its_top_card() {
    let mut harness = UiHarness::new();
    let mut table = practice_table();
    frame(&mut harness, &mut table);
    build_pile(&mut harness, &mut table);
    let top = cards_in(&table, CardZone::Table).into_iter().max_by_key(|card| card.z_index).unwrap();
    click(&mut harness, &mut table, PILE_SPOT, 1, 1);
    let hand = cards_in(&table, CardZone::Hand);
    assert_eq!(hand.len(), 1, "a click draws one card");
    assert_eq!(hand[0].instance_id, top.instance_id, "the top card");
    assert_eq!(cards_in(&table, CardZone::Table).len(), 2);
}

#[test]
fn dragging_a_pile_peels_the_top_card_and_alt_moves_it_whole() {
    let mut harness = UiHarness::new();
    let mut table = practice_table();
    frame(&mut harness, &mut table);
    build_pile(&mut harness, &mut table);

    // A plain drag peels off just the top card.
    drag(&mut harness, &mut table, PILE_SPOT, (400.0, 300.0));
    let loose: Vec<CardInstance> =
        cards_in(&table, CardZone::Table).into_iter().filter(|card| card.x < 0.45).collect();
    assert_eq!(loose.len(), 1, "one card peeled off");
    assert!(loose[0].stack_parent_id.is_none());

    // Alt + drag moves the remaining two-card pile together.
    harness.hold("lalt");
    drag(&mut harness, &mut table, PILE_SPOT, (850.0, 300.0));
    harness.let_go("lalt");
    let moved: Vec<CardInstance> = cards_in(&table, CardZone::Table).into_iter().filter(|card| card.x > 0.55).collect();
    assert_eq!(moved.len(), 2, "the whole pile moved");

    // Alt + drag the pile onto the hand deals every card into it.
    harness.hold("lalt");
    drag(&mut harness, &mut table, (850.0, 300.0), HAND_CARD);
    harness.let_go("lalt");
    assert_eq!(cards_in(&table, CardZone::Hand).len(), 2, "the pile went into the hand");
    assert_eq!(cards_in(&table, CardZone::Table).len(), 1);
}

#[test]
fn simple_counter_from_the_board_menu() {
    let mut harness = UiHarness::new();
    let mut table = practice_table();
    frame(&mut harness, &mut table);
    let spot = (900.0, 250.0);
    menu(&mut harness, &mut table, spot, &["Widgets", "Simple Counter"]);
    let counters = widgets(&table, BoardWidgetKind::SimpleCounter);
    assert_eq!(counters.len(), 1, "a counter was created");
    assert_eq!(counters[0].value, 0.0);
    let value = |table: &TableScreen| widgets(table, BoardWidgetKind::SimpleCounter)[0].value;

    // Double-click the right half adds one, the left half takes one away (never below 0).
    click(&mut harness, &mut table, (spot.0 + 12.0, spot.1), 1, 2);
    click(&mut harness, &mut table, (spot.0 + 12.0, spot.1), 1, 2);
    assert_eq!(value(&table), 2.0);
    click(&mut harness, &mut table, (spot.0 - 12.0, spot.1), 1, 2);
    assert_eq!(value(&table), 1.0);
    click(&mut harness, &mut table, (spot.0 - 12.0, spot.1), 1, 2);
    click(&mut harness, &mut table, (spot.0 - 12.0, spot.1), 1, 2);
    assert_eq!(value(&table), 0.0, "clamped at the minimum");

    // Its menu: Increment, then Delete.
    menu(&mut harness, &mut table, spot, &["Increment"]);
    assert_eq!(value(&table), 1.0);
    menu(&mut harness, &mut table, spot, &["Delete"]);
    assert!(widgets(&table, BoardWidgetKind::SimpleCounter).is_empty(), "deleted");
}

#[test]
fn pass_turn_marks_the_current_player() {
    let mut harness = UiHarness::new();
    let mut table = practice_table();
    frame(&mut harness, &mut table);
    menu(&mut harness, &mut table, (900.0, 250.0), &["Pass Turn To", "Bea"]);
    assert_eq!(table.backend().session().state().current_player_id.as_deref(), Some("seat2"));
}

#[test]
fn ctrl_click_and_shift_box_select_then_create_pile_here() {
    let mut harness = UiHarness::new();
    let mut table = practice_table();
    frame(&mut harness, &mut table);
    let spots = [(450.0, 250.0), (640.0, 250.0), (830.0, 250.0)];
    for spot in spots {
        play_card(&mut harness, &mut table, spot, false);
    }

    // Ctrl+click the first two, Shift+box the third.
    harness.hold("lctrl");
    click(&mut harness, &mut table, spots[0], 1, 1);
    click(&mut harness, &mut table, spots[1], 1, 1);
    harness.let_go("lctrl");
    harness.hold("lshift");
    drag(&mut harness, &mut table, (780.0, 150.0), (900.0, 350.0));
    harness.let_go("lshift");

    // The selection menu piles them all at the click.
    menu(&mut harness, &mut table, spots[1], &["Create Pile Here"]);
    let on_table = cards_in(&table, CardZone::Table);
    assert_eq!(on_table.len(), 3);
    assert_eq!(on_table.iter().filter(|card| card.stack_parent_id.is_some()).count(), 2, "{on_table:?}");

    // Esc clears the selection first; a plain click on empty table also clears it.
    key(&mut harness, &mut table, "escape");
}

#[test]
fn tab_drag_draws_an_arrow() {
    let mut harness = UiHarness::new();
    let mut table = practice_table();
    frame(&mut harness, &mut table);
    harness.hold("tab");
    drag(&mut harness, &mut table, (500.0, 200.0), (800.0, 350.0));
    harness.let_go("tab");
    frame(&mut harness, &mut table);
    assert_eq!(widgets(&table, BoardWidgetKind::Arrow).len(), 1, "an arrow everyone can see");
    assert!(cards_in(&table, CardZone::Table).is_empty(), "no box selection or card move");
}

#[test]
fn z_sends_a_stack_to_the_back() {
    let mut harness = UiHarness::new();
    let mut table = practice_table();
    frame(&mut harness, &mut table);
    play_card(&mut harness, &mut table, (600.0, 300.0), false);
    play_card(&mut harness, &mut table, (640.0, 300.0), false);
    let newest = cards_in(&table, CardZone::Table).into_iter().max_by_key(|card| card.z_index).unwrap();
    hover(&mut harness, &mut table, (660.0, 300.0));
    key(&mut harness, &mut table, "z");
    let lowest = cards_in(&table, CardZone::Table).into_iter().min_by_key(|card| card.z_index).unwrap();
    assert_eq!(lowest.instance_id, newest.instance_id, "the front card went to the back");
}

#[test]
fn give_to_asks_first() {
    let mut harness = UiHarness::new();
    let mut table = practice_table();
    frame(&mut harness, &mut table);
    // Standard 52's shared deck deals unownable cards; a Card Library card belongs to its creator.
    // Played to the spot first to learn its table position, then sent back.
    play_card(&mut harness, &mut table, PILE_SPOT, false);
    let played = cards_in(&table, CardZone::Table)[0].clone();
    table.backend_mut().perform(TableAction::MoveToHand { instance_id: played.instance_id });
    let definition_id = table.game().cards()[0].id.clone();
    table.backend_mut().perform(TableAction::CreateCardFromLibrary { definition_id, x: played.x, y: played.y });
    frame(&mut harness, &mut table);
    let owned = |table: &TableScreen| cards_in(table, CardZone::Table)[0].owner_id.clone();
    assert_eq!(owned(&table).as_deref(), Some("seat1"));
    menu(&mut harness, &mut table, PILE_SPOT, &["Give to Bea"]);
    assert!(table.is_dialog_open(), "Give to asks first");
    assert_eq!(owned(&table).as_deref(), Some("seat1"));
    // As in the original, a confirmation has no Enter action; Escape cancels it.
    harness.key("return");
    frame(&mut harness, &mut table);
    assert!(table.is_dialog_open(), "Enter doesn't confirm");
    harness.key("escape");
    frame(&mut harness, &mut table);
    frame(&mut harness, &mut table);
    assert!(!table.is_dialog_open(), "Escape cancels");
    assert_eq!(owned(&table).as_deref(), Some("seat1"), "still ours");
}
