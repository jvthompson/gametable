//! Zones placed on a playmat, through real frames: the Standard 52-Card Deck's shared discard
//! pile is placed in the middle of a shared playmat. With the mat on, the discard pile leaves the
//! left sidebar and a hand card dropped on the middle of the mat goes into it; with the mat off,
//! the pile is back in the sidebar.

use std::collections::HashMap;

use gametable::app::harness::UiHarness;
use gametable::data::game_loader::load_standard_deck;
use gametable::table::{PracticeBackend, TableOptions, TableScreen};
use gametable_core::models::{CardZone, PlayerInfo, PlayerRole, PlaymatDefinition, PlaymatType, PlaymatZonePlacement};
use gametable_core::session::{DealOptions, Session, SessionEnvironment};

/// The deck alone in the left sidebar at 1280×720 (centred vertically in a 110-wide column).
const DECK_ALONE: (f32, f32) = (55.0, 360.0);
/// The deck and discard pile together in the sidebar (as in `table.rs`).
const DECK_WITH_DISCARD: (f32, f32) = (55.0, 284.0);
const DISCARD_IN_SIDEBAR: (f32, f32) = (55.0, 436.0);
/// The middle of the local player's hand band (one card fanned at the centre).
const HAND_CARD: (f32, f32) = (695.0, 640.0);
/// The middle of the table (and of the shared mat) with only the left sidebar showing.
const TABLE_MIDDLE: (f32, f32) = (695.0, 360.0);

fn practice_table_with_discard_on_the_mat() -> TableScreen {
    let mut game = load_standard_deck();
    let discard_id = game.zones.iter().find(|zone| zone.is_discard_pile).unwrap().id.clone();
    game.playmat_type = PlaymatType::Shared;
    let mut mat = PlaymatDefinition::new("missing-image.png");
    mat.zone_placements.push(PlaymatZonePlacement::new(discard_id, 0.5, 0.5));
    game.playmats.push(mat);
    let players = vec![
        PlayerInfo::new("seat1", "Ann", PlayerRole::Host, 0xFF45_5A64),
        PlayerInfo::new("seat2", "Player 2", PlayerRole::Host, 0xFFE9_1E63),
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
    let mut session = Session::deal_from_zones(options, SessionEnvironment::system());
    session.set_shared_playmat(true, 1);
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

fn discard_count(table: &TableScreen) -> usize {
    let discard_id = table.game().zones.iter().find(|zone| zone.is_discard_pile).map(|zone| zone.id.clone());
    cards_in(table, CardZone::Zone).iter().filter(|card| card.zone_id == discard_id).count()
}

/// Draws one card from the deck at `deck` into the hand.
fn draw_one(harness: &mut UiHarness, table: &mut TableScreen, deck: (f32, f32)) {
    hover(harness, table, deck);
    key(harness, table, "1");
    assert_eq!(cards_in(table, CardZone::Hand).len(), 1, "drew one card from {deck:?}");
}

#[test]
fn a_zone_on_the_shared_mat_leaves_the_sidebar_and_takes_drops() {
    let mut harness = UiHarness::new();
    let mut table = practice_table_with_discard_on_the_mat();
    frame(&mut harness, &mut table);
    frame(&mut harness, &mut table);

    // Only the deck is left in the sidebar, centred on its own.
    draw_one(&mut harness, &mut table, DECK_ALONE);

    // A hand card dropped on the middle of the mat goes into the discard pile there.
    drag(&mut harness, &mut table, HAND_CARD, TABLE_MIDDLE);
    assert!(cards_in(&table, CardZone::Hand).is_empty());
    assert!(cards_in(&table, CardZone::Table).is_empty(), "went into the zone, not onto the table");
    assert_eq!(discard_count(&table), 1);

    // F12 goes past the only mat to off: the discard pile is back in the sidebar under the deck.
    key(&mut harness, &mut table, "f12");
    frame(&mut harness, &mut table);
    assert!(!table.backend().session().state().shared_playmat_on);
    draw_one(&mut harness, &mut table, DECK_WITH_DISCARD);
    drag(&mut harness, &mut table, HAND_CARD, DISCARD_IN_SIDEBAR);
    assert_eq!(discard_count(&table), 2);

    // And a card dropped in the middle of the table now stays on the table.
    draw_one(&mut harness, &mut table, DECK_WITH_DISCARD);
    drag(&mut harness, &mut table, HAND_CARD, TABLE_MIDDLE);
    assert_eq!(cards_in(&table, CardZone::Table).len(), 1);
    assert_eq!(discard_count(&table), 2);
}

#[test]
fn the_host_ctrl_drags_a_zone_to_a_new_spot_on_the_shared_mat() {
    let mut harness = UiHarness::new();
    let mut table = practice_table_with_discard_on_the_mat();
    frame(&mut harness, &mut table);
    frame(&mut harness, &mut table);

    // Ctrl + drag the discard pile 150 pixels right (zoom 1, so 150 world units of the mat).
    let moved_to = (TABLE_MIDDLE.0 + 150.0, TABLE_MIDDLE.1);
    hover(&mut harness, &mut table, TABLE_MIDDLE);
    harness.hold("lctrl");
    harness.press(TABLE_MIDDLE.0, TABLE_MIDDLE.1, 1, 1);
    frame(&mut harness, &mut table);
    for step in 1..=6 {
        let t = step as f32 / 6.0;
        harness.move_mouse(TABLE_MIDDLE.0 + 150.0 * t, TABLE_MIDDLE.1);
        frame(&mut harness, &mut table);
    }
    harness.release(moved_to.0, moved_to.1, 1);
    frame(&mut harness, &mut table);
    harness.let_go("lctrl");
    frame(&mut harness, &mut table);

    let positions = table.backend().session().state().shared_playmat_zone_positions.clone();
    assert_eq!(positions.len(), 1, "{positions:?}");
    // The shared mat is 1280 wide while its image is missing: 150 to the right is 0.5 + 150/1280.
    assert!((positions[0].x - (0.5 + 150.0 / 1280.0)).abs() < 0.01, "{positions:?}");
    assert!((positions[0].y - 0.5).abs() < 0.01, "{positions:?}");
    assert!(cards_in(&table, CardZone::Table).is_empty(), "moving the zone moved no cards");

    // Cards now drop into it at its new spot, and not at the old one.
    draw_one(&mut harness, &mut table, DECK_ALONE);
    drag(&mut harness, &mut table, HAND_CARD, moved_to);
    assert_eq!(discard_count(&table), 1);
    draw_one(&mut harness, &mut table, DECK_ALONE);
    drag(&mut harness, &mut table, HAND_CARD, TABLE_MIDDLE);
    assert_eq!(discard_count(&table), 1);
    assert_eq!(cards_in(&table, CardZone::Table).len(), 1);
}

#[test]
fn f12_cycles_from_the_mat_to_off_and_back() {
    let mut harness = UiHarness::new();
    let mut table = practice_table_with_discard_on_the_mat();
    frame(&mut harness, &mut table);
    let shared_mat = |table: &TableScreen| {
        let state = table.backend().session().state();
        (state.shared_playmat_on, state.shared_playmat_index)
    };
    assert_eq!(shared_mat(&table), (true, 1));
    key(&mut harness, &mut table, "f12");
    assert_eq!(shared_mat(&table), (false, 0), "past the only mat: off");
    key(&mut harness, &mut table, "f12");
    assert_eq!(shared_mat(&table), (true, 1), "then the first mat again");
    key(&mut harness, &mut table, "f11");
    assert_eq!(shared_mat(&table), (true, 1), "F11 no longer does anything");
}
