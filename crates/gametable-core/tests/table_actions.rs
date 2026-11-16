//! Port of CardTable's `tests/table_actions_test.lua`.
//!
//! The Lua tests check "no-op" by object identity; here a no-op means the returned state equals
//! the input (same revision, same contents).

mod common;

use common::*;
use gametable_core::models::*;
use gametable_core::table_actions::*;
use gametable_core::util::ParkMillerRandom;

fn assert_near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "expected {expected}, got {actual}");
}

fn card_in<'a>(state: &'a TableState, instance_id: &str) -> &'a CardInstance {
    state.card_by_id(instance_id).unwrap_or_else(|| panic!("card {instance_id} missing"))
}

fn widget_in<'a>(state: &'a TableState, instance_id: &str) -> &'a BoardWidgetInstance {
    state.widget_by_id(instance_id).unwrap_or_else(|| panic!("widget {instance_id} missing"))
}

fn widget_ids(state: &TableState) -> Vec<String> {
    state.widgets().iter().map(|widget| widget.instance_id.clone()).collect()
}

/// root (z0) <- a (z1), root <- b (z2)
fn pile() -> TableState {
    state(
        vec![],
        vec![
            card("root").definition("d1").build(),
            card("a").definition("d2").z(1).on("root").build(),
            card("b").definition("d3").z(2).on("root").build(),
        ],
    )
}

fn zone(zone_id: &str, owner_id: Option<&str>) -> TableState {
    let in_zone = |id: &str, definition: &str, z: i64| {
        let mut builder = card(id).definition(definition).z(z).in_zone(zone_id);
        if let Some(owner_id) = owner_id {
            builder = builder.owner(owner_id);
        }
        builder.build()
    };
    state(vec![], vec![in_zone("z1", "d1", 0), in_zone("z2", "d2", 1), in_zone("z3", "d3", 2)])
}

fn table_with_widgets(cards: Vec<CardInstance>, widgets: Vec<BoardWidgetInstance>) -> TableState {
    state(vec![], cards).with_widgets(widgets)
}

fn widget_at(id: &str, kind: BoardWidgetKind, x: f64, y: f64, z_index: i64) -> BoardWidgetInstance {
    BoardWidgetInstance::new(id, kind, x, y, z_index)
}

// ---------------------------------------------------------------------------
// moveCard / moveStack / moveGroup
// ---------------------------------------------------------------------------

#[test]
fn move_card_detaches_and_moves_to_table() {
    let original = state(vec![], vec![card("c").in_zone("deck").on("p").z(3).build(), card("p").z(5).build()]);
    let moved = move_card(&original, "c", 0.4, 0.6);
    let c = card_in(&moved, "c");
    assert_eq!((c.x, c.y, c.zone, c.z_index), (0.4, 0.6, CardZone::Table, 6));
    assert_eq!(c.stack_parent_id, None);
    assert_eq!(c.zone_id, None);
    assert_eq!(moved.revision, 1);
    assert_eq!(original.revision, 0);
    assert_eq!(card_in(&original, "c").zone, CardZone::Zone);
}

#[test]
fn move_card_drags_attached_widgets() {
    let mut rider = widget_at("w", BoardWidgetKind::Token, 0.15, 0.12, 0);
    rider.attached_card_id = Some("c".into());
    rider.attach_offset_x = 0.05;
    rider.attach_offset_y = 0.02;
    let original = table_with_widgets(vec![card("c").at(0.1, 0.1).build()], vec![rider]);
    let moved = move_card(&original, "c", 0.5, 0.5);
    assert_near(moved.widgets()[0].x, 0.55);
    assert_near(moved.widgets()[0].y, 0.52);
}

#[test]
fn move_card_clears_owner_only_for_unownable_cards() {
    let original = state(vec![], vec![card("u").owner("p1").unownable().build(), card("o").owner("p1").build()]);
    assert_eq!(card_in(&move_card(&original, "u", 0.0, 0.0), "u").owner_id, None);
    assert_eq!(card_in(&move_card(&original, "o", 0.0, 0.0), "o").owner_id.as_deref(), Some("p1"));
}

#[test]
fn move_stack_moves_every_member_and_raises_only_the_top() {
    let moved = move_stack(&pile(), "root", 0.3, 0.3);
    assert!(moved.cards().iter().all(|card| (card.x, card.y) == (0.3, 0.3)));
    assert_eq!(card_in(&moved, "b").z_index, 3);
    assert_eq!(card_in(&moved, "a").z_index, 1);
    assert_eq!(card_in(&moved, "a").stack_parent_id.as_deref(), Some("root"));
    assert_eq!(move_stack(&pile(), "nope", 0.0, 0.0), pile());
}

#[test]
fn move_group_translates_passengers_and_orders_z() {
    let original = state(
        vec![],
        vec![
            card("p").at(0.1, 0.1).build(),
            card("q").at(0.2, 0.3).z(1).build(),
            card("r").at(0.5, 0.5).z(2).build(),
            card("r2").at(0.5, 0.5).z(3).on("r").build(),
        ],
    );
    let moved = move_group(&original, "p", &["q".into(), "r".into()], 0.2, 0.2);
    let q = card_in(&moved, "q");
    assert_near(q.x, 0.3);
    assert_near(q.y, 0.4);
    assert_near(card_in(&moved, "r2").x, 0.6);
    assert_eq!(card_in(&moved, "p").z_index, 4);
    assert_eq!(q.z_index, 5);
    assert_eq!(card_in(&moved, "r2").z_index, 6);
    assert_eq!(card_in(&moved, "r").z_index, 2);
}

#[test]
fn move_group_ownership_and_unknown_primary() {
    let original =
        state(vec![], vec![card("p").owner("p1").unownable().build(), card("q").owner("p1").unownable().z(1).build()]);
    let moved = move_group(&original, "p", &["q".into()], 0.1, 0.1);
    assert_eq!(card_in(&moved, "p").owner_id, None);
    assert_eq!(card_in(&moved, "q").owner_id.as_deref(), Some("p1"));
    assert_eq!(move_group(&original, "zzz", &[], 0.0, 0.0), original);
}

// ---------------------------------------------------------------------------
// rotate / flip / give / sendToBack / stack
// ---------------------------------------------------------------------------

#[test]
fn rotate_stack_rotates_table_cards_and_raises_the_top() {
    let rotated = rotate_stack(&pile(), "root", true, 1.0);
    assert!(rotated.cards().iter().all(|card| card.rotation_turns == 1.0));
    assert_eq!(card_in(&rotated, "b").z_index, 3);
    let rotated = rotate_stack(&rotate_stack(&rotated, "root", false, 1.0), "root", false, 1.0);
    assert_eq!(card_in(&rotated, "a").rotation_turns, -1.0);

    let half = rotate_stack(&pile(), "root", true, 0.5);
    assert_eq!(card_in(&half, "a").rotation_turns, 0.5);
    assert_eq!(card_in(&rotate_stack(&half, "root", false, 0.5), "a").rotation_turns, 0.0);

    let in_hand = state(vec![], vec![card("h").in_hand("p1").build()]);
    assert_eq!(card_in(&rotate_stack(&in_hand, "h", true, 1.0), "h").rotation_turns, 0.0);
}

#[test]
fn flip_give_and_send_to_back() {
    let flipped = flip_card(&pile(), "a");
    assert!(card_in(&flipped, "a").face_up);
    assert_eq!(card_in(&flipped, "a").z_index, 3);

    let owned = state(vec![], vec![card("c").owner("p1").build(), card("u").unownable().build()]);
    assert_eq!(card_in(&give_card(&owned, "c", Some("p2")), "c").owner_id.as_deref(), Some("p2"));
    assert_eq!(card_in(&give_card(&owned, "c", None), "c").owner_id, None);
    assert_eq!(card_in(&give_card(&owned, "u", Some("p2")), "u").owner_id, None);

    let sent = send_to_back(&pile(), "root");
    assert_eq!(card_in(&sent, "b").z_index, -1);
    assert_eq!(card_in(&sent, "root").z_index, 0);
}

#[test]
fn stack_card_snaps_to_target_and_links_parent() {
    let original = state(
        vec![],
        vec![
            card("t").at(0.7, 0.2).z(1).build(),
            card("c").at(0.1, 0.0).in_zone("deck").owner("p1").unownable().build(),
        ],
    );
    let stacked = stack_card(&original, "c", "t");
    let c = card_in(&stacked, "c");
    assert_eq!((c.x, c.y, c.zone, c.stack_parent_id.as_deref(), c.z_index), (0.7, 0.2, CardZone::Table, Some("t"), 2));
    assert_eq!(c.zone_id, None);
    assert_eq!(c.owner_id, None);
    assert_eq!(stack_card(&pile(), "a", "a"), pile());
    assert_eq!(stack_card(&pile(), "a", "zzz"), pile());
}

// ---------------------------------------------------------------------------
// Hands
// ---------------------------------------------------------------------------

#[test]
fn move_to_hand_resets_card() {
    let original = state(vec![], vec![card("c").in_zone("deck").on("x").rotation(2.0).build()]);
    let in_hand = move_to_hand(&original, "c", "p1");
    let c = card_in(&in_hand, "c");
    assert_eq!((c.zone, c.owner_id.as_deref(), c.face_up, c.rotation_turns), (CardZone::Hand, Some("p1"), true, 0.0));
    assert_eq!(c.zone_id, None);
    assert_eq!(c.stack_parent_id, None);
}

fn hand_fixture() -> TableState {
    state(
        vec![],
        vec![
            card("h1").in_hand("p1").build(),
            card("h2").in_hand("p1").z(1).build(),
            card("h3").in_hand("p1").z(2).build(),
            card("o").in_hand("p2").z(3).build(),
        ],
    )
}

fn hand_order(state: &TableState, owner_id: &str) -> Vec<String> {
    ids_of_cards(hand_cards(state, Some(owner_id)))
}

#[test]
fn reorder_hand_moves_the_card_and_leaves_other_hands() {
    assert_eq!(hand_order(&reorder_hand(&hand_fixture(), "h3", Some("p1"), 0), "p1"), ["h3", "h1", "h2"]);
    assert_eq!(hand_order(&reorder_hand(&hand_fixture(), "h1", Some("p1"), 1), "p1"), ["h2", "h1", "h3"]);
    assert_eq!(hand_order(&reorder_hand(&hand_fixture(), "h1", Some("p1"), 99), "p1"), ["h2", "h3", "h1"]);
    assert_eq!(card_in(&reorder_hand(&hand_fixture(), "h1", Some("p1"), 1), "o").z_index, 3);
    assert_eq!(reorder_hand(&hand_fixture(), "o", Some("p1"), 0), hand_fixture());
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

#[test]
fn draw_card_takes_the_top_and_the_anchor_last() {
    let first = draw_card(&pile(), "root", "p1", 1).state;
    assert_eq!(card_in(&first, "b").zone, CardZone::Hand);
    let second = draw_card(&first, "root", "p1", 1).state;
    assert_eq!(card_in(&second, "a").zone, CardZone::Hand);
    let third = draw_card(&second, "root", "p1", 1).state;
    assert_eq!(card_in(&third, "root").zone, CardZone::Hand);
    assert_eq!(card_in(&third, "root").owner_id.as_deref(), Some("p1"));
}

#[test]
fn draw_card_counts_in_one_revision_and_clamps() {
    let result = draw_card(&pile(), "root", "p1", 3);
    assert_eq!(result.drawn_instance_ids, ["b", "a", "root"]);
    assert_eq!(result.state.revision, 1);
    assert_eq!(card_in(&result.state, "b").z_index, 3);
    assert_eq!(card_in(&result.state, "root").z_index, 5);
    assert_eq!(draw_card(&pile(), "root", "p1", 10).drawn_instance_ids.len(), 3);
    assert_eq!(draw_card(&pile(), "root", "p1", 0).state, pile());
    assert_eq!(draw_card(&pile(), "nope", "p1", 1).state, pile());

    let rotated = state(vec![], vec![card("solo").rotation(1.0).build()]);
    assert_eq!(draw_card(&rotated, "solo", "p1", 1).state.cards()[0].rotation_turns, 0.0);
}

#[test]
fn draw_from_zone_takes_top_cards() {
    let drawn = draw_from_zone(&zone("deck", Some("p1")), "deck", Some("p1"), "p1", 1).state;
    let z3 = card_in(&drawn, "z3");
    assert_eq!((z3.zone, z3.owner_id.as_deref(), z3.face_up), (CardZone::Hand, Some("p1"), true));
    assert_eq!(z3.zone_id, None);
    let drawn = draw_from_zone(&zone("deck", None), "deck", None, "p2", 1).state;
    assert_eq!(card_in(&drawn, "z3").owner_id.as_deref(), Some("p2"));

    let result = draw_from_zone(&zone("deck", None), "deck", None, "p1", 2);
    assert_eq!(result.drawn_instance_ids, ["z3", "z2"]);
    assert_eq!(result.state.revision, 1);
    let shared = zone("deck", None);
    assert_eq!(draw_from_zone(&shared, "deck", Some("p1"), "p1", 1).state, shared, "owner mismatch");
    assert_eq!(draw_from_zone(&shared, "deck", None, "p1", 0).state, shared);
}

// ---------------------------------------------------------------------------
// returnToZone
// ---------------------------------------------------------------------------

fn with_loose_card(base: TableState, loose: CardInstance) -> TableState {
    let mut cards = base.cards().to_vec();
    cards.push(loose);
    state(vec![], cards)
}

#[test]
fn return_to_zone_on_top_owned_by_zone_and_snapped_to_anchor() {
    let original = with_loose_card(
        state(vec![], vec![card("z1").at(0.3, 0.4).in_zone("deck").owner("p1").build()]),
        card("c").at(0.9, 0.9).in_hand("p1").z(5).face_up().rotation(1.0).build(),
    );
    let returned = return_to_zone(&original, "c", "deck", Some("p1"), false, false);
    let c = card_in(&returned, "c");
    assert_eq!(
        (c.x, c.y, c.zone, c.zone_id.as_deref(), c.owner_id.as_deref(), c.face_up, c.z_index, c.rotation_turns),
        (0.3, 0.4, CardZone::Zone, Some("deck"), Some("p1"), false, 6, 0.0)
    );
}

#[test]
fn return_to_zone_shared_bottom_empty_and_already_in_zone() {
    let original = with_loose_card(zone("deck", None), card("h").in_hand("p1").z(5).build());
    let returned = return_to_zone(&original, "h", "deck", None, true, false);
    assert_eq!(card_in(&returned, "h").owner_id, None);
    assert!(card_in(&returned, "h").face_up);

    let original = with_loose_card(zone("deck", None), card("h").z(5).build());
    assert_eq!(card_in(&return_to_zone(&original, "h", "deck", None, false, true), "h").z_index, -1);

    let lone = state(vec![], vec![card("c").at(0.2, 0.8).build()]);
    let returned = return_to_zone(&lone, "c", "deck", Some("p1"), false, false);
    assert_eq!((returned.cards()[0].x, returned.cards()[0].y), (0.2, 0.8));
    assert_eq!(return_to_zone(&zone("deck", None), "z1", "deck", None, false, false), zone("deck", None));
}

// ---------------------------------------------------------------------------
// Shuffling
// ---------------------------------------------------------------------------

#[test]
fn shuffle_zone_turns_cards_face_down_and_reassigns_z() {
    let face_up_zone = state(
        vec![],
        zone("deck", None).cards().iter().map(|existing| CardInstance { face_up: true, ..existing.clone() }).collect(),
    );
    let shuffled = shuffle_zone(&face_up_zone, "deck", None, &mut ParkMillerRandom::with_seed(7));
    assert!(shuffled.cards().iter().all(|card| !card.face_up));
    let mut z_indexes: Vec<i64> = shuffled.cards().iter().map(|card| card.z_index).collect();
    z_indexes.sort();
    assert_eq!(z_indexes, [3, 4, 5]);
    let single = state(vec![], vec![card("z").in_zone("deck").build()]);
    assert_eq!(shuffle_zone(&single, "deck", None, &mut ParkMillerRandom::with_seed(1)), single);
}

#[test]
fn shuffle_pile_raises_every_card() {
    let shuffled = shuffle_pile(&pile(), "root", &mut ParkMillerRandom::with_seed(3));
    assert_eq!(shuffled.cards().len(), 3);
    assert!(shuffled.cards().iter().all(|card| card.z_index >= 3));
    let single = state(vec![], vec![card("x").build()]);
    assert_eq!(shuffle_pile(&single, "x", &mut ParkMillerRandom::with_seed(1)), single);
}

// ---------------------------------------------------------------------------
// Searches
// ---------------------------------------------------------------------------

#[test]
fn searches_start_replaces_and_stop_removes() {
    let searching = start_search(&state(vec![], vec![]), "p1", SearchTargetType::Zone, "deck", Some("p1"));
    let searching = start_search(&searching, "p2", SearchTargetType::Pile, "root", None);
    let searching = start_search(&searching, "p1", SearchTargetType::Zone, "discard", None);
    assert_eq!(searching.searches.len(), 2);
    assert_eq!(searching.searches[1].target_id, "discard");
    assert_eq!(searching.revision, 3);
    let stopped = stop_search(&searching, "p1");
    assert_eq!(stopped.searches.len(), 1);
    assert_eq!(stop_search(&stopped, "p1"), stopped);
}

// ---------------------------------------------------------------------------
// Widgets
// ---------------------------------------------------------------------------

#[test]
fn widget_lifecycle() {
    let start = state(vec![], vec![card("c").at(0.5, 0.5).build()]);
    let s = create_widget(&start, "w1", BoardWidgetKind::SimpleCounter, 0.1, 0.2, None, None);
    let s = create_widget(&s, "w2", BoardWidgetKind::DeckBuilder, 0.1, 0.2, Some("p1"), None);
    assert_eq!(widget_ids(&s), ["w1", "w2"]);
    assert_eq!(s.widgets()[1].z_index, 1);
    assert_eq!(s.widgets()[1].owner_id.as_deref(), Some("p1"));
    assert_eq!(s.widgets()[0].owner_id, None);

    let s = create_arrow(&s, "ar", 0.0, 0.0, 1.0, 1.0, Some("p1"));
    let arrow = &s.widgets()[2];
    assert_eq!(
        (arrow.kind, arrow.x2, arrow.creator_id.as_deref(), arrow.z_index),
        (BoardWidgetKind::Arrow, Some(1.0), Some("p1"), 2)
    );

    let s = attach_widget_to_card(&s, "w1", "c", 0.52, 0.47);
    let w1 = widget_in(&s, "w1");
    assert_eq!((w1.x, w1.y, w1.attached_card_id.as_deref()), (0.52, 0.47, Some("c")));
    assert_near(w1.attach_offset_x, 0.02);
    assert_near(w1.attach_offset_y, -0.03);
    assert_eq!(attach_widget_to_card(&s, "w1", "missing", 0.0, 0.0), s);

    let s = move_widget(&s, "w1", 0.9, 0.9);
    assert_eq!(widget_in(&s, "w1").attached_card_id, None);
    assert_eq!(widget_in(&s, "w1").attach_offset_x, 0.0);
    assert_eq!(move_widget(&s, "nope", 0.0, 0.0).revision, s.revision + 1);

    assert_eq!(widget_in(&set_widget_value(&s, "w1", -5.0), "w1").value, 0.0);
    assert_eq!(widget_in(&set_widget_value(&s, "w1", 123456.0), "w1").value, 99999.0);
    assert_eq!(widget_in(&set_widget_value(&s, "w1", 42.0), "w1").value, 42.0);

    let s = set_widget_colors(&s, "w1", 0xFFD32F2F, 0xFF000000);
    assert_eq!(widget_in(&s, "w1").background_color, 0xFFD32F2F);
    let s = set_widget_text(&s, "w1", "Hi", 0xFF000000);
    assert_eq!((widget_in(&s, "w1").text.as_str(), widget_in(&s, "w1").text_color), ("Hi", 0xFF000000));
    let s = set_widget_size(&s, "w1", Some(WidgetSizeOverride::Multiplier(50.0)));
    assert_eq!(widget_in(&s, "w1").size_override, Some(WidgetSizeOverride::Multiplier(50.0)));
    let s = set_widget_size(&s, "w1", None);
    assert_eq!(widget_in(&s, "w1").size_override, None);

    let z_before = widget_in(&s, "w1").z_index;
    let s = rotate_widget(&s, "w1", true, 1.0);
    assert_eq!(widget_in(&s, "w1").rotation_turns, 1.0);
    assert!(widget_in(&s, "w1").z_index > z_before, "rotating brings the widget to the front");
    let s = rotate_widget(&s, "w1", false, 0.5);
    assert_eq!(widget_in(&s, "w1").rotation_turns, 0.5);
    assert!(widget_in(&s, "w1").face_up);
    let z_before = widget_in(&s, "w1").z_index;
    let s = flip_widget(&s, "w1");
    assert!(!widget_in(&s, "w1").face_up);
    assert!(widget_in(&s, "w1").z_index > z_before, "flipping brings the widget to the front");
    let s = flip_widget(&flip_widget(&s, "w1"), "w1");
    let s = set_widget_value(&s, "w1", 7.0);
    let s = set_widget_size(&s, "w1", Some(WidgetSizeOverride::Multiplier(50.0)));

    let s = duplicate_widget(&s, "w1", "w9", 0.3, 0.3);
    let w9 = widget_in(&s, "w9");
    assert_eq!(
        (
            w9.kind,
            w9.value,
            w9.background_color,
            w9.text_color,
            w9.text.as_str(),
            w9.x,
            w9.size_override,
            w9.rotation_turns,
            w9.face_up
        ),
        (
            BoardWidgetKind::SimpleCounter,
            7.0,
            0xFFD32F2F,
            0xFF000000,
            "Hi",
            0.3,
            Some(WidgetSizeOverride::Multiplier(50.0)),
            0.5,
            false
        )
    );
    assert_eq!(duplicate_widget(&s, "nope", "x", 0.0, 0.0), s);
}

#[test]
fn delete_widget_spills_only_that_deck_widgets_cards() {
    let mut deck_widget = widget_at("dw", BoardWidgetKind::DeckBuilder, 0.4, 0.6, 0);
    deck_widget.owner_id = Some("p1".into());
    let mut other_deck_widget = widget_at("dw2", BoardWidgetKind::DeckBuilder, 0.0, 0.0, 0);
    other_deck_widget.owner_id = Some("p1".into());
    let original = table_with_widgets(
        vec![
            card("in1").in_zone("deckWidget:dw:main_deck").owner("p1").on("x").build(),
            card("in2").in_zone("deckWidget:dw:side").owner("p1").unownable().z(1).build(),
            card("other").in_zone("deckWidget:dw2:main_deck").owner("p1").z(2).build(),
        ],
        vec![deck_widget, other_deck_widget],
    );
    let deleted = delete_widget(&original, "dw");
    let in1 = card_in(&deleted, "in1");
    assert_eq!(
        (in1.zone, in1.x, in1.y, in1.z_index, in1.owner_id.as_deref()),
        (CardZone::Table, 0.4, 0.6, 3, Some("p1"))
    );
    assert_eq!(in1.zone_id, None);
    assert_eq!(in1.stack_parent_id, None);
    assert_eq!(card_in(&deleted, "in2").z_index, 4);
    assert_eq!(card_in(&deleted, "in2").owner_id, None);
    assert_eq!(card_in(&deleted, "other").zone, CardZone::Zone);
    assert_eq!(widget_ids(&deleted), ["dw2"]);
    assert_eq!(delete_widget(&original, "nope").revision, 1);
}

#[test]
fn card_tokens_never_attach_but_can_be_anchors() {
    let original = table_with_widgets(
        vec![card("c").at(0.5, 0.5).build()],
        vec![widget_at("ct", BoardWidgetKind::CardToken, 0.0, 0.0, 0)],
    );
    assert_eq!(attach_widget_to_card(&original, "ct", "c", 0.5, 0.5), original);

    let original = table_with_widgets(
        vec![],
        vec![
            widget_at("ct", BoardWidgetKind::CardToken, 0.5, 0.5, 0),
            widget_at("w1", BoardWidgetKind::Token, 0.0, 0.0, 0),
        ],
    );
    let attached = attach_widget_to_card(&original, "w1", "ct", 0.52, 0.47);
    let w1 = widget_in(&attached, "w1");
    assert_eq!((w1.x, w1.y, w1.attached_card_id.as_deref()), (0.52, 0.47, Some("ct")));
    assert_near(w1.attach_offset_x, 0.02);
    assert_near(w1.attach_offset_y, -0.03);
    let moved = move_widget(&attached, "ct", 0.6, 0.6);
    assert_near(widget_in(&moved, "w1").x, 0.62);
    assert_near(widget_in(&moved, "w1").y, 0.57);
}

#[test]
fn deleting_a_card_token_anchor_detaches_riders() {
    let mut rider = widget_at("w1", BoardWidgetKind::Token, 0.0, 0.0, 0);
    rider.attached_card_id = Some("ct".into());
    rider.attach_offset_x = 0.01;
    rider.attach_offset_y = 0.02;
    let original = table_with_widgets(vec![], vec![widget_at("ct", BoardWidgetKind::CardToken, 0.0, 0.0, 0), rider]);
    let deleted = delete_widget(&original, "ct");
    let w1 = widget_in(&deleted, "w1");
    assert_eq!(w1.attached_card_id, None);
    assert_eq!((w1.attach_offset_x, w1.attach_offset_y), (0.0, 0.0));
}

#[test]
fn card_tokens_share_the_card_layer_z_order() {
    let original = table_with_widgets(
        vec![card("c1").z(3).build(), card("c2").z(7).build()],
        vec![widget_at("w1", BoardWidgetKind::Token, 0.0, 0.0, 50)],
    );
    let s = create_widget(&original, "ct", BoardWidgetKind::CardToken, 0.5, 0.5, None, None);
    assert_eq!(widget_in(&s, "ct").z_index, 8);
    let s = move_widget(&s, "ct", 0.6, 0.6);
    assert_eq!(widget_in(&s, "ct").z_index, 9);
    let s = rotate_widget(&s, "ct", true, 1.0);
    assert_eq!(widget_in(&s, "ct").z_index, 10);
    let s = flip_widget(&s, "ct");
    assert_eq!(widget_in(&s, "ct").z_index, 11);
    let s = duplicate_widget(&s, "ct", "ct2", 0.4, 0.4);
    assert_eq!(widget_in(&s, "ct2").z_index, 12);
    let s = duplicate_widget(&s, "w1", "w2", 0.2, 0.2);
    assert_eq!(widget_in(&s, "w2").z_index, 51);
}

#[test]
fn send_widget_to_back_goes_behind_the_card_layer() {
    let original = table_with_widgets(
        vec![card("c1").z(3).build(), card("c2").z(7).build()],
        vec![widget_at("ct", BoardWidgetKind::CardToken, 0.0, 0.0, 9)],
    );
    let sent = send_widget_to_back(&original, "ct");
    assert_eq!(widget_in(&sent, "ct").z_index, 2);
    assert_eq!(send_widget_to_back(&sent, "nope"), sent);
}

// ---------------------------------------------------------------------------
// Token bags
// ---------------------------------------------------------------------------

fn bag_state(remaining: &[&str], revealed: &[(&str, &str)]) -> TableState {
    let mut bag = widget_at("bag", BoardWidgetKind::TokenBag, 0.0, 0.0, 0);
    bag.image_path = Some("bag.png".into());
    bag.bag_remaining = remaining.iter().map(|path| path.to_string()).collect();
    bag.bag_revealed = revealed.iter().map(|(slot, path)| RevealedToken::new(*slot, *path)).collect();
    table_with_widgets(vec![], vec![bag])
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

#[test]
fn add_widgets_numbers_z_in_one_revision() {
    let original = table_with_widgets(vec![], vec![widget_at("w0", BoardWidgetKind::Token, 0.0, 0.0, 5)]);
    let added = add_widgets(
        &original,
        vec![
            widget_at("w1", BoardWidgetKind::Token, 0.0, 0.0, 0),
            widget_at("w2", BoardWidgetKind::Token, 0.0, 0.0, 0),
        ],
    );
    assert_eq!(widget_ids(&added), ["w0", "w1", "w2"]);
    assert_eq!((widget_in(&added, "w1").z_index, widget_in(&added, "w2").z_index), (6, 7));
    assert_eq!(added.revision, 1);
    assert_eq!(add_widgets(&original, vec![]), original);
}

#[test]
fn reveal_and_resolve_bag_tokens() {
    let original = bag_state(&["a.png", "a.png", "b.png"], &[]);
    let revealed = reveal_bag_token(&original, "bag", "a.png", "slot1");
    let bag = widget_in(&revealed, "bag");
    assert_eq!(bag.bag_remaining, strings(&["a.png", "b.png"]));
    assert_eq!(bag.bag_revealed, vec![RevealedToken::new("slot1", "a.png")]);
    assert_eq!(revealed.revision, 1);
    assert_eq!(original.revision, 0);

    let original = bag_state(&[], &[("s1", "a.png")]);
    let returned = resolve_bag_token(&original, "bag", "s1", true);
    assert_eq!(widget_in(&returned, "bag").bag_remaining, strings(&["a.png"]));
    assert!(widget_in(&returned, "bag").bag_revealed.is_empty());
    let destroyed = resolve_bag_token(&original, "bag", "s1", false);
    assert!(widget_in(&destroyed, "bag").bag_remaining.is_empty());
    assert!(widget_in(&destroyed, "bag").bag_revealed.is_empty());
    let missing = resolve_bag_token(&original, "bag", "nope", true);
    assert_eq!(widget_in(&missing, "bag").bag_revealed, vec![RevealedToken::new("s1", "a.png")]);
}

#[test]
fn place_bag_token_on_table_creates_an_image_token() {
    let original = bag_state(&[], &[("s1", "skull.png")]);
    let placed_state = place_bag_token_on_table(
        &original,
        "bag",
        "s1",
        "new1",
        0.4,
        0.6,
        None,
        Some(WidgetSizeOverride::Multiplier(100.0)),
    );
    assert!(widget_in(&placed_state, "bag").bag_revealed.is_empty());
    let placed = widget_in(&placed_state, "new1");
    assert_eq!(
        (
            placed.kind,
            placed.image_path.as_deref(),
            placed.x,
            placed.y,
            placed.size_override,
            placed.owner_id.as_deref()
        ),
        (BoardWidgetKind::ImageToken, Some("skull.png"), 0.4, 0.6, Some(WidgetSizeOverride::Multiplier(100.0)), None)
    );
    assert_eq!(place_bag_token_on_table(&original, "bag", "nope", "x", 0.0, 0.0, None, None), original);
}

#[test]
fn reset_and_set_bag_remaining() {
    let mut bag = widget_at("bag", BoardWidgetKind::TokenBag, 0.0, 0.0, 0);
    bag.bag_remaining = strings(&["a.png"]);
    bag.bag_revealed = vec![RevealedToken::new("s1", "b.png")];
    let mut placed = widget_at("placed", BoardWidgetKind::ImageToken, 0.0, 0.0, 0);
    placed.image_path = Some("b.png".into());
    let original = table_with_widgets(vec![], vec![bag, placed]);

    let reset = reset_bag(&original, "bag", &strings(&["a.png", "a.png", "b.png"]));
    assert_eq!(widget_in(&reset, "bag").bag_remaining, strings(&["a.png", "a.png", "b.png"]));
    assert!(widget_in(&reset, "bag").bag_revealed.is_empty());
    assert_eq!(widget_in(&reset, "placed"), widget_in(&original, "placed"));

    let replaced = set_bag_remaining(&original, "bag", &strings(&["c.png", "c.png"]));
    assert_eq!(widget_in(&replaced, "bag").bag_remaining, strings(&["c.png", "c.png"]));
    assert_eq!(widget_in(&replaced, "bag").bag_revealed, vec![RevealedToken::new("s1", "b.png")]);
}

#[test]
fn return_image_token_to_bag_optionally_refills() {
    let mut bag = widget_at("bag", BoardWidgetKind::TokenBag, 0.0, 0.0, 0);
    bag.bag_remaining = strings(&["a.png"]);
    let mut token = widget_at("tok", BoardWidgetKind::ImageToken, 0.0, 0.0, 0);
    token.image_path = Some("zzz.png".into());
    let original = table_with_widgets(vec![], vec![bag, token]);

    let returned = return_image_token_to_bag(&original, "bag", "tok", true);
    assert!(returned.widget_by_id("tok").is_none());
    assert_eq!(widget_in(&returned, "bag").bag_remaining, strings(&["a.png", "zzz.png"]));
    let destroyed = return_image_token_to_bag(&original, "bag", "tok", false);
    assert!(destroyed.widget_by_id("tok").is_none());
    assert_eq!(widget_in(&destroyed, "bag").bag_remaining, strings(&["a.png"]));
    assert_eq!(return_image_token_to_bag(&original, "bag", "nope", true), original);
}

#[test]
fn deal_new_cards_appends_with_ascending_z() {
    let dealt = deal_new_cards(&pile(), vec![card("n1").z(99).build(), card("n2").z(99).build()]);
    assert_eq!(card_in(&dealt, "n1").z_index, 3);
    assert_eq!(card_in(&dealt, "n2").z_index, 4);
    assert_eq!(dealt.revision, 1);
    assert_eq!(deal_new_cards(&pile(), vec![]), pile());
}
