//! Game boards through real frames, with the Standard 52-Card Deck's shared discard pile placed
//! in the middle of a shared board. With the board shown, the discard pile leaves the left sidebar
//! and a hand card dropped on the middle of the board goes into it; with it hidden, the pile is
//! back in the sidebar. Boards placed on the table are there from the start, and F12 only shows
//! and hides the toggleable ones.

use std::collections::HashMap;

use gametable::app::harness::UiHarness;
use gametable::data::game_loader::load_standard_deck;
use gametable::table::{PracticeBackend, TableOptions, TableScreen};
use gametable_core::models::{
    BoardKind, BoardPlayerPlacement, BoardTablePlacement, BoardZonePlacement, CardZone, GameBoardDefinition,
    GameDefinition, MovedBoard, PlayerInfo, PlayerRole,
};
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

/// The standard deck with one shared board (its image missing, so 1280 x 720) holding the
/// discard pile in its middle; `table_placement` puts it on the table.
fn game_with_discard_on_a_shared_board(table_placement: Option<BoardTablePlacement>) -> GameDefinition {
    let mut game = load_standard_deck();
    let discard_id = game.zones.iter().find(|zone| zone.is_discard_pile).unwrap().id.clone();
    let mut board = GameBoardDefinition { kind: BoardKind::Shared, ..GameBoardDefinition::new("missing-image.png") };
    board.zone_placements.push(BoardZonePlacement::new(discard_id, 0.5, 0.5));
    board.table_placement = table_placement;
    game.game_boards.push(board);
    game
}

/// Nothing placed: the shared board is the original's centred shared playmat, shown here.
fn practice_table_with_discard_on_the_mat() -> TableScreen {
    let mut table = practice_table(game_with_discard_on_a_shared_board(None));
    table.backend_mut().session_mut().set_shared_playmat(true, 1);
    table
}

fn practice_table(game: GameDefinition) -> TableScreen {
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

    // F12 hides the board: the discard pile is back in the sidebar under the deck.
    key(&mut harness, &mut table, "f12");
    frame(&mut harness, &mut table);
    assert!(!table.backend().session().state().shared_boards_shown);
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

    let positions = table.backend().session().state().shared_board_zone_positions.clone();
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
fn f12_hides_and_shows_the_board_and_keeps_the_choice() {
    let mut harness = UiHarness::new();
    let mut table = practice_table_with_discard_on_the_mat();
    frame(&mut harness, &mut table);
    let shared_board = |table: &TableScreen| {
        let state = table.backend().session().state();
        (state.shared_boards_shown, state.chosen_shared_board)
    };
    assert_eq!(shared_board(&table), (true, 1));
    key(&mut harness, &mut table, "f12");
    assert_eq!(shared_board(&table), (false, 1), "hidden, still the same board");
    key(&mut harness, &mut table, "f12");
    assert_eq!(shared_board(&table), (true, 1), "shown again");
    key(&mut harness, &mut table, "f11");
    assert_eq!(shared_board(&table), (true, 1), "F11 does nothing");
}

/// The table with the seeding done, as when it is first shown.
fn entered(harness: &mut UiHarness, table: &mut TableScreen) {
    harness.frame(|frame| table.enter(frame));
    frame(harness, table);
    frame(harness, table);
}

#[test]
fn a_placed_board_is_there_from_the_start_and_f12_leaves_it() {
    let mut harness = UiHarness::new();
    let placement = BoardTablePlacement::new(1280.0, 720.0);
    let mut table = practice_table(game_with_discard_on_a_shared_board(Some(placement)));
    entered(&mut harness, &mut table);

    // Never toggled on, yet the board holds the discard pile in the middle of the table.
    assert!(!table.backend().session().state().shared_boards_shown);
    draw_one(&mut harness, &mut table, DECK_ALONE);
    drag(&mut harness, &mut table, HAND_CARD, TABLE_MIDDLE);
    assert_eq!(discard_count(&table), 1);

    // It can't be toggled, so F12 changes nothing and the pile stays on the board.
    key(&mut harness, &mut table, "f12");
    frame(&mut harness, &mut table);
    draw_one(&mut harness, &mut table, DECK_ALONE);
    drag(&mut harness, &mut table, HAND_CARD, TABLE_MIDDLE);
    assert_eq!(discard_count(&table), 2);
}

#[test]
fn a_toggleable_placed_board_starts_shown_and_f12_hides_it() {
    let mut harness = UiHarness::new();
    let placement = BoardTablePlacement { toggleable: true, ..BoardTablePlacement::new(1280.0, 720.0) };
    let mut table = practice_table(game_with_discard_on_a_shared_board(Some(placement)));
    entered(&mut harness, &mut table);
    assert!(table.backend().session().state().shared_boards_shown, "starts shown");
    draw_one(&mut harness, &mut table, DECK_ALONE);

    key(&mut harness, &mut table, "f12");
    frame(&mut harness, &mut table);
    assert!(!table.backend().session().state().shared_boards_shown);
    // Hidden: the discard pile is back in the sidebar, and the middle of the table is just table.
    drag(&mut harness, &mut table, HAND_CARD, TABLE_MIDDLE);
    assert_eq!(cards_in(&table, CardZone::Table).len(), 1);
    assert_eq!(discard_count(&table), 0);

    key(&mut harness, &mut table, "f12");
    assert!(table.backend().session().state().shared_boards_shown);
}

fn right_click_menu_item(harness: &mut UiHarness, table: &mut TableScreen, at: (f32, f32), item: &str) {
    hover(harness, table, at);
    harness.press(at.0, at.1, 2, 1);
    frame(harness, table);
    harness.release(at.0, at.1, 2);
    frame(harness, table);
    frame(harness, table);
    harness.click_region(&format!("table:menu:menu1:{item}"), &mut |frame| {
        table.update(frame, 1.0 / 60.0);
        table.draw(frame, 0.0, 0.0, 1280.0, 720.0);
    });
    frame(harness, table);
}

fn my_moved_boards(table: &TableScreen) -> Vec<MovedBoard> {
    table.backend().session().state().players[0].moved_boards.clone()
}

#[test]
fn unlock_a_player_board_then_drag_and_turn_it_by_its_handle() {
    let mut harness = UiHarness::new();
    let mut game = load_standard_deck();
    // Cards that players can own, so the other player's card can lie on my board.
    for card in game.cards_mut() {
        card.unownable = false;
    }
    // Placed for the first player only, in the middle of the table.
    game.game_boards.push(GameBoardDefinition {
        player_placements: vec![BoardPlayerPlacement::new(1, 1280.0, 720.0)],
        ..GameBoardDefinition::new("missing-image.png")
    });
    let mut table = practice_table(game);
    // One of my cards lies on the board (right of its middle), one of the other player's lies on
    // it too, and another of mine lies off it.
    let zone_card_ids: Vec<String> =
        cards_in(&table, CardZone::Zone).into_iter().take(3).map(|card| card.instance_id).collect();
    let [mine_on_board, mine_off_board, theirs_on_board] = <[String; 3]>::try_from(zone_card_ids).unwrap();
    let session = table.backend_mut().session_mut();
    for (id, x, y, owner) in [
        (&mine_on_board, 0.55, 0.5, "seat1"),
        (&mine_off_board, 0.1, 0.1, "seat1"),
        (&theirs_on_board, 0.45, 0.5, "seat2"),
    ] {
        session.move_card(id, x, y, "seat1").unwrap();
        session.give_card(id, Some(owner), "seat1").unwrap();
    }
    let card_by_id = |table: &TableScreen, id: &str| table.backend().session().state().card_by_id(id).unwrap().clone();
    let theirs_before = card_by_id(&table, &theirs_on_board);
    let off_board_before = card_by_id(&table, &mine_off_board);
    entered(&mut harness, &mut table);
    assert_eq!(table.board_handle_center(1), None, "locked: no handle");

    // Tokens, Widgets, Card Library, Pass Turn To, a divider, then the board's own entries.
    right_click_menu_item(&mut harness, &mut table, TABLE_MIDDLE, "6:Unlock Board");
    let handle = table.board_handle_center(1).expect("the handle shows");
    assert!((handle.0 - TABLE_MIDDLE.0).abs() < 1.0 && handle.1 > TABLE_MIDDLE.1, "below the board: {handle:?}");

    // Dragging the handle moves the board for everyone.
    drag(&mut harness, &mut table, handle, (handle.0 + 100.0, handle.1));
    let moved = my_moved_boards(&table);
    assert_eq!(moved.len(), 1);
    assert!(moved[0].x > 0.5 && (moved[0].y - 0.5).abs() < 1e-3 && moved[0].rotation == 0, "{moved:?}");
    // My card on the board went with it; the other player's card and my card off the board stayed.
    let mine = card_by_id(&table, &mine_on_board);
    assert!((mine.x - (0.55 + moved[0].x - 0.5)).abs() < 1e-3 && (mine.y - 0.5).abs() < 1e-3, "{mine:?}");
    assert_eq!(card_by_id(&table, &theirs_on_board), theirs_before);
    assert_eq!(card_by_id(&table, &mine_off_board), off_board_before);

    // Q while dragging turns it a quarter turn counterclockwise.
    let handle = table.board_handle_center(1).unwrap();
    hover(&mut harness, &mut table, handle);
    harness.press(handle.0, handle.1, 1, 1);
    frame(&mut harness, &mut table);
    harness.move_mouse(handle.0 + 10.0, handle.1);
    frame(&mut harness, &mut table);
    key(&mut harness, &mut table, "q");
    harness.release(handle.0 + 10.0, handle.1, 1);
    frame(&mut harness, &mut table);
    frame(&mut harness, &mut table);
    assert_eq!(my_moved_boards(&table)[0].rotation, 270);
    // The card turned with the board, and now lies above the board's middle (a tenth of the
    // table's width right of it became the same distance up).
    let turned = my_moved_boards(&table)[0].clone();
    let mine = card_by_id(&table, &mine_on_board);
    assert_eq!(mine.rotation_turns, -1.0);
    assert!((mine.x - turned.x).abs() < 1e-3 && mine.y < turned.y, "{mine:?} on {turned:?}");

    // Reset Board Position puts it back; Lock Board hides the handle.
    let handle = table.board_handle_center(1).unwrap();
    right_click_menu_item(&mut harness, &mut table, handle, "7:Reset Board Position");
    assert!(my_moved_boards(&table).is_empty());
    // ...with my card back where it started on it.
    let mine = card_by_id(&table, &mine_on_board);
    assert!((mine.x - 0.55).abs() < 1e-3 && (mine.y - 0.5).abs() < 1e-3 && mine.rotation_turns == 0.0, "{mine:?}");
    right_click_menu_item(&mut harness, &mut table, TABLE_MIDDLE, "6:Lock Board");
    assert_eq!(table.board_handle_center(1), None);
}

#[test]
fn placed_player_boards_toggle_for_the_player_only() {
    let mut harness = UiHarness::new();
    let mut game = load_standard_deck();
    let player_board = GameBoardDefinition {
        table_placement: Some(BoardTablePlacement { toggleable: true, ..BoardTablePlacement::new(0.0, -200.0) }),
        ..GameBoardDefinition::new("missing-image.png")
    };
    game.game_boards.push(player_board);
    let mut table = practice_table(game);
    entered(&mut harness, &mut table);
    let shown = |table: &TableScreen| {
        let state = table.backend().session().state();
        state.players.iter().map(|player| player.player_boards_shown).collect::<Vec<_>>()
    };
    assert_eq!(shown(&table), [true, true], "every practice seat's boards start shown");
    key(&mut harness, &mut table, "f12");
    assert_eq!(shown(&table), [false, true], "F12 toggles only the acting player's");
    assert!(!table.backend().session().state().shared_boards_shown, "no shared boards to toggle");
}

/// The standard deck with a scenario whose shared "Boss Deck" zone (two cards) it sets in the
/// middle of the table.
fn game_with_a_scenario_zone_set_on_the_table() -> GameDefinition {
    let mut game = load_standard_deck();
    let card_id = game.cards()[0].id.clone();
    let boss_deck = gametable_core::models::ZoneDefinition {
        shared: true,
        entries: vec![gametable_core::models::DeckEntry::new(card_id, 2)],
        ..gametable_core::models::ZoneDefinition::new("boss", "Boss Deck")
    };
    let mut scenario = gametable_core::models::Scenario::new("raid", "Raid");
    scenario.zones.push(boss_deck);
    scenario.zone_placements.push(gametable_core::models::ScenarioZonePlacement::new("boss", 0.5, 0.5));
    game.scenarios.push(scenario);
    game
}

fn boss_deck_count(table: &TableScreen) -> usize {
    cards_in(table, CardZone::Zone).iter().filter(|card| card.zone_id.as_deref() == Some("boss")).count()
}

#[test]
fn a_scenario_zone_set_on_the_table_sits_there_and_cannot_be_moved() {
    let mut harness = UiHarness::new();
    let mut table = practice_table(game_with_a_scenario_zone_set_on_the_table());
    table.backend_mut().session_mut().load_scenario("raid", "seat1");
    frame(&mut harness, &mut table);
    frame(&mut harness, &mut table);
    assert_eq!(boss_deck_count(&table), 2);

    // It isn't docked in a sidebar: the deck and discard pile keep their usual two slots.
    draw_one(&mut harness, &mut table, DECK_WITH_DISCARD);
    drag(&mut harness, &mut table, HAND_CARD, DISCARD_IN_SIDEBAR);
    assert_eq!(discard_count(&table), 1);

    // It sits in the middle of the table: 1 draws from it, and a card dropped there goes into it.
    hover(&mut harness, &mut table, TABLE_MIDDLE);
    key(&mut harness, &mut table, "1");
    assert_eq!(boss_deck_count(&table), 1);
    assert_eq!(cards_in(&table, CardZone::Hand).len(), 1);
    drag(&mut harness, &mut table, HAND_CARD, TABLE_MIDDLE);
    assert_eq!(boss_deck_count(&table), 2);
    assert!(cards_in(&table, CardZone::Table).is_empty());

    // Ctrl + drag doesn't move it (it sits on no board): like a sidebar zone, the drag just takes
    // its top card onto the table, and the zone is still in the middle afterwards.
    hover(&mut harness, &mut table, TABLE_MIDDLE);
    harness.hold("lctrl");
    harness.press(TABLE_MIDDLE.0, TABLE_MIDDLE.1, 1, 1);
    frame(&mut harness, &mut table);
    for step in 1..=6 {
        harness.move_mouse(TABLE_MIDDLE.0 + 25.0 * step as f32, TABLE_MIDDLE.1);
        frame(&mut harness, &mut table);
    }
    harness.release(TABLE_MIDDLE.0 + 150.0, TABLE_MIDDLE.1, 1);
    frame(&mut harness, &mut table);
    harness.let_go("lctrl");
    frame(&mut harness, &mut table);
    assert!(table.backend().session().state().shared_board_zone_positions.is_empty());
    assert_eq!(boss_deck_count(&table), 1);
    assert_eq!(cards_in(&table, CardZone::Table).len(), 1, "the top card was dragged out");
    hover(&mut harness, &mut table, TABLE_MIDDLE);
    key(&mut harness, &mut table, "1");
    assert_eq!(boss_deck_count(&table), 0, "still drawn from in the middle of the table");
    assert_eq!(cards_in(&table, CardZone::Hand).len(), 1);
}
