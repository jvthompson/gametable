//! Ports of CardTable's `tests/stack_utils_test.lua` and `tests/utils_test.lua` (seat layout,
//! geometry, state filtering, pack generation, card sort/filter and saved-game remapping).

mod common;

use std::collections::{HashMap, HashSet};

use common::*;
use gametable_core::card_filters::*;
use gametable_core::geometry::*;
use gametable_core::models::*;
use gametable_core::pack_generator::select_pack_cards;
use gametable_core::save_remap::*;
use gametable_core::seat_utils::*;
use gametable_core::stack_utils::*;
use gametable_core::state_filter::filter_state_for_recipient;
use gametable_core::util::{ParkMillerRandom, RandomIndexSource};

// ---------------------------------------------------------------------------
// stack_utils
// ---------------------------------------------------------------------------

#[test]
fn root_id_walks_chains_and_survives_dangling_parents_and_cycles() {
    let a = card("a").build();
    assert_eq!(root_id_of(std::slice::from_ref(&a), &a), "a");

    let cards = vec![card("a").build(), card("b").on("a").build(), card("c").on("b").build()];
    assert_eq!(root_id_of(&cards, &cards[2]), "a");
    assert_eq!(root_id_of(&cards, &cards[1]), "a");
    assert_eq!(root_id_of(&cards, &cards[0]), "a");

    let dangling = vec![card("c").on("gone").build()];
    assert_eq!(root_id_of(&dangling, &dangling[0]), "c");

    let cycle = vec![card("a").on("b").build(), card("b").on("a").build()];
    assert_eq!(root_id_of(&cycle, &cycle[0]), "a");
}

#[test]
fn stack_of_and_group_by_stack_keep_original_order() {
    let cards = vec![card("a").build(), card("x").build(), card("b").on("a").build(), card("c").on("b").build()];
    assert_eq!(ids_of_cards(stack_of(&cards, "a")), ["a", "b", "c"]);
    assert_eq!(ids_of_cards(stack_of(&cards, "x")), ["x"]);

    let cards = vec![card("a1").build(), card("x1").build(), card("b1").on("a1").build(), card("a2").on("a1").build()];
    let grouped = group_by_stack(&cards);
    assert_eq!(grouped.root_ids_in_order, ["a1", "x1"]);
    assert_eq!(ids_of_cards(grouped.cards_by_root_id["a1"].iter().copied()), ["a1", "b1", "a2"]);
    assert_eq!(ids_of_cards(grouped.cards_by_root_id["x1"].iter().copied()), ["x1"]);
}

#[test]
fn top_of_prefers_highest_z_and_the_earlier_card_on_ties() {
    let (a, b, c) = (card("a").z(1).build(), card("b").z(3).build(), card("c").z(2).build());
    assert_eq!(top_of(&[&a, &b, &c]).unwrap().instance_id, "b");
    let (d, e) = (card("d").z(5).build(), card("e").z(5).build());
    assert_eq!(top_of(&[&d, &e]).unwrap().instance_id, "d");
    assert_eq!(top_of(&[&a]).unwrap().instance_id, "a");
    assert!(top_of(&[]).is_none());
}

// ---------------------------------------------------------------------------
// seat_utils
// ---------------------------------------------------------------------------

fn four_players() -> Vec<PlayerInfo> {
    vec![host("p1", "P1"), player("p2", "p2"), player("p3", "p3"), player("p4", "p4")]
}

#[test]
fn hand_rows_for_four_players() {
    let players = four_players();
    for viewer in ["p1", "p2"] {
        let layout = compute_hand_row_layout(&players, viewer);
        assert_eq!(ids_of_players(layout.bottom_row), ["p1", "p2"]);
        assert_eq!(ids_of_players(layout.top_row), ["p3", "p4"]);
    }
    for viewer in ["p3", "p4"] {
        let layout = compute_hand_row_layout(&players, viewer);
        assert_eq!(ids_of_players(layout.bottom_row), ["p4", "p3"]);
        assert_eq!(ids_of_players(layout.top_row), ["p2", "p1"]);
    }
}

#[test]
fn hand_rows_for_three_and_two_players() {
    let three = &four_players()[..3];
    let layout = compute_hand_row_layout(three, "p1");
    assert_eq!(ids_of_players(layout.bottom_row), ["p1"]);
    assert_eq!(ids_of_players(layout.top_row), ["p2", "p3"]);
    for viewer in ["p2", "p3"] {
        let layout = compute_hand_row_layout(three, viewer);
        assert_eq!(ids_of_players(layout.bottom_row), ["p3", "p2"]);
        assert_eq!(ids_of_players(layout.top_row), ["p1"]);
    }
    let two = &four_players()[..2];
    let layout = compute_hand_row_layout(two, "p2");
    assert_eq!(ids_of_players(layout.bottom_row), ["p2"]);
    assert_eq!(ids_of_players(layout.top_row), ["p1"]);
    assert!(is_viewer_mirrored(two, "p2"));
    assert!(!is_viewer_mirrored(two, "p1"));
}

// ---------------------------------------------------------------------------
// geometry
// ---------------------------------------------------------------------------

#[test]
fn geometry_matches_the_original_tests() {
    assert_eq!(world_centering_offset(1600.0, 900.0, 2000.0, 500.0), (200.0, -200.0));
    assert_eq!(clamp_card_center_y(200.0, 400.0, 100.0, 10.0), 200.0);
    assert_eq!(clamp_card_center_y(-20.0, 400.0, 100.0, 10.0), -20.0);
    assert_eq!(clamp_card_center_y(450.0, 400.0, 100.0, 10.0), 450.0);
    assert_eq!(clamp_card_center_y(-500.0, 400.0, 100.0, 10.0), -140.0);
    assert_eq!(clamp_card_center_y(999.0, 10.0, 100.0, 10.0), 5.0);
    assert_eq!(canonical_to_local_pixel(0.25, 0.75, 800.0, 400.0, true), (600.0, 100.0));
    assert_eq!(local_pixel_to_canonical(200.0, 300.0, 800.0, 400.0, true), (0.75, 0.25));
    assert!((local_pixel_to_canonical(900.0, 0.0, 800.0, 400.0, false).0 - 1.125).abs() < 1e-9);
    assert_eq!(local_pixel_to_canonical(-1000.0, 0.0, 800.0, 400.0, false).0, -0.5);
    assert_eq!(local_pixel_to_canonical(2000.0, 0.0, 800.0, 400.0, false).0, 1.5);
}

// ---------------------------------------------------------------------------
// state_filter
// ---------------------------------------------------------------------------

fn find<'a>(state: &'a TableState, instance_id: &str) -> &'a CardInstance {
    state.card_by_id(instance_id).unwrap()
}

#[test]
fn filter_hides_opponent_hand_cards_only() {
    let mut two_hands = state(
        vec![],
        vec![
            card("mine").definition("hearts_A").face_up().in_hand("p1").build(),
            card("theirs").definition("spades_K").face_up().in_hand("p2").z(1).build(),
            card("onTable").definition("clubs_5").at(10.0, 10.0).z(2).build(),
        ],
    );
    two_hands.revision = 3;
    let for_p1 = filter_state_for_recipient(&two_hands, "p1", &HashSet::new());
    assert!(!find(&for_p1, "theirs").face_up);
    assert_eq!(find(&for_p1, "theirs").definition_id, HIDDEN_DEFINITION_ID);
    assert_eq!(find(&for_p1, "mine").definition_id, "hearts_A");
    assert_eq!(find(&for_p1, "onTable").definition_id, "clubs_5");
    assert_eq!(for_p1.revision, 3);
    let for_p2 = filter_state_for_recipient(&two_hands, "p2", &HashSet::new());
    assert_eq!(find(&for_p2, "mine").definition_id, HIDDEN_DEFINITION_ID);
    assert_eq!(find(&for_p2, "theirs").definition_id, "spades_K");
}

#[test]
fn filter_hides_private_zones_and_passes_shared_and_visible_zones() {
    let table = state(
        vec![],
        vec![
            card("a").definition("x").in_zone("draw_deck").owner("p2").build(),
            card("b").definition("y").in_zone("deck").build(),
            card("c").definition("z").face_up().in_zone("discard_pile").owner("p2").build(),
            card("d").definition("w").face_up().in_zone("deckWidget:dw1:main_deck").owner("p2").build(),
        ],
    );
    let visible: HashSet<String> = ["discard_pile".to_string()].into();
    let for_p1 = filter_state_for_recipient(&table, "p1", &visible);
    assert_eq!(find(&for_p1, "a").definition_id, HIDDEN_DEFINITION_ID);
    assert_eq!(find(&for_p1, "b").definition_id, "y");
    assert_eq!(find(&for_p1, "c").definition_id, "z");
    assert_eq!(find(&for_p1, "c").owner_id.as_deref(), Some("p2"));
    assert_eq!(find(&for_p1, "d").definition_id, HIDDEN_DEFINITION_ID);
    let for_owner = filter_state_for_recipient(&table, "p2", &HashSet::new());
    assert_eq!(find(&for_owner, "d").definition_id, "w");
}

// ---------------------------------------------------------------------------
// pack_generator
// ---------------------------------------------------------------------------

fn numbered_pool(count: usize) -> Vec<CardDefinition> {
    (0..count).map(|index| card_definition(&format!("c{index}"), &[])).collect()
}

fn setting(tag: &str, count: f64) -> PackSetting {
    PackSetting { tag: tag.to_string(), count }
}

/// Returns scripted values in order (clamped into range), then 1 once exhausted.
struct ScriptedRandom {
    values: Vec<usize>,
    next_position: usize,
}

impl RandomIndexSource for ScriptedRandom {
    fn random_from_one_to(&mut self, upper_inclusive: usize) -> usize {
        let value = self.values.get(self.next_position).copied().unwrap_or(1);
        self.next_position += 1;
        value.min(upper_inclusive)
    }
}

fn scripted(values: &[usize]) -> ScriptedRandom {
    ScriptedRandom { values: values.to_vec(), next_position: 0 }
}

#[test]
fn pack_without_settings_is_ten_distinct_cards() {
    let pool = numbered_pool(15);
    let pack = select_pack_cards(&pool, &[], &mut ParkMillerRandom::with_seed(1));
    assert_eq!(pack.len(), 10);
    assert_eq!(ids_of_definitions(pack.iter().copied()).into_iter().collect::<HashSet<_>>().len(), 10);
    assert_eq!(select_pack_cards(&numbered_pool(2), &[], &mut ParkMillerRandom::with_seed(1)).len(), 2);
}

#[test]
fn pack_tag_settings_shortfalls_and_no_double_counting() {
    let pool = vec![
        card_definition("r1", &["Rare"]),
        card_definition("r2", &["Rare"]),
        card_definition("r3", &["Rare"]),
        card_definition("c1", &["Common"]),
    ];
    let pack = select_pack_cards(&pool, &[setting("Rare", 2.0)], &mut ParkMillerRandom::with_seed(1));
    assert_eq!(pack.len(), 2);
    assert!(pack.iter().all(|card| card.types.contains(&"Rare".to_string())));

    let single_rare = vec![card_definition("r1", &["Rare"])];
    assert_eq!(select_pack_cards(&single_rare, &[setting("Rare", 5.0)], &mut ParkMillerRandom::with_seed(1)).len(), 1);

    let both = vec![card_definition("both", &["Rare", "Foil"])];
    let pack =
        select_pack_cards(&both, &[setting("Rare", 1.0), setting("Foil", 1.0)], &mut ParkMillerRandom::with_seed(1));
    assert_eq!(ids_of_definitions(pack), ["both"]);
    assert!(select_pack_cards(&[], &[setting("Rare", 3.0)], &mut ParkMillerRandom::with_seed(1)).is_empty());
}

#[test]
fn pack_seeded_generation_repeats() {
    let pool = numbered_pool(20);
    let first = ids_of_definitions(select_pack_cards(&pool, &[], &mut ParkMillerRandom::with_seed(42)));
    let second = ids_of_definitions(select_pack_cards(&pool, &[], &mut ParkMillerRandom::with_seed(42)));
    assert_eq!(first, second);
}

#[test]
fn pack_fractional_settings() {
    let mut pool = numbered_pool(10);
    pool.push(card_definition("legendary", &["Legendary"]));
    assert_eq!(select_pack_cards(&pool, &[setting("Legendary", 0.4)], &mut ParkMillerRandom::with_seed(1)).len(), 10);

    let set_cards = vec![card_definition("common1", &["Common"]), card_definition("legendary1", &["Legendary"])];
    let settings = [setting("Common", 1.0), setting("Legendary", 0.5)];
    let hit = select_pack_cards(&set_cards, &settings, &mut scripted(&[500_000]));
    assert_eq!(ids_of_definitions(hit), ["common1", "legendary1"]);
    let miss = select_pack_cards(&set_cards, &settings, &mut scripted(&[500_001]));
    assert_eq!(ids_of_definitions(miss), ["common1"]);

    let rares =
        vec![card_definition("r1", &["Rare"]), card_definition("r2", &["Rare"]), card_definition("r3", &["Rare"])];
    assert_eq!(select_pack_cards(&rares, &[setting("Rare", 1.5)], &mut scripted(&[1, 1, 500_000])).len(), 2);
    assert_eq!(select_pack_cards(&rares, &[setting("Rare", 1.5)], &mut scripted(&[1, 1, 999_999])).len(), 1);
}

#[test]
fn pack_matches_qualified_tags_by_tag_name() {
    let pool = vec![card_definition("r1", &["Rarity|Rare"]), card_definition("c1", &["Rarity|Common"])];
    let pack = select_pack_cards(&pool, &[setting("Rare", 1.0)], &mut ParkMillerRandom::with_seed(1));
    assert_eq!(ids_of_definitions(pack), ["r1"]);
}

// ---------------------------------------------------------------------------
// card_filters
// ---------------------------------------------------------------------------

fn sort_fixture() -> (Vec<GameSet>, TagGroup, Vec<CardDefinition>) {
    let sets = vec![
        GameSet { id: "set_a".into(), name: "A".into(), pack_settings: vec![] },
        GameSet { id: "set_b".into(), name: "B".into(), pack_settings: vec![] },
    ];
    let race = TagGroup {
        id: "race".into(),
        name: "Race".into(),
        tags: vec!["Dunadan".into(), "Elf".into(), "Hobbit".into()],
    };
    let with_set = |id: &str, title: &str, set: Option<&str>, tags: &[&str]| CardDefinition {
        set_id: set.map(String::from),
        types: tags.iter().map(|tag| tag.to_string()).collect(),
        ..CardDefinition::new(id, title)
    };
    let cards = vec![
        with_set("u", "No Set Card", None, &[]),
        with_set("d", "Dunadan Card", Some("set_b"), &["Dunadan"]),
        with_set("n", "No Race Card", Some("set_a"), &["Resource"]),
        with_set("e", "Elf Card", Some("set_a"), &["Elf"]),
    ];
    (sets, race, cards)
}

#[test]
fn sorts_by_name_set_and_tag_group() {
    let (sets, race, cards) = sort_fixture();
    let groups = [race];
    let refs: Vec<&CardDefinition> = cards.iter().collect();
    let sorted_ids = |order: CardSortOrder| ids_of_definitions(sort_card_definitions(&refs, &order, &sets, &groups));
    assert_eq!(sorted_ids(CardSortOrder::ByName), ["d", "e", "n", "u"]);
    assert_eq!(sorted_ids(CardSortOrder::BySet), ["e", "n", "d", "u"]);
    assert_eq!(sorted_ids(CardSortOrder::from_key("race")), ["d", "e", "n", "u"]);
}

fn tags(group_id: &str, tag_names: &[&str]) -> HashMap<String, HashSet<String>> {
    [(group_id.to_string(), tag_names.iter().map(|tag| tag.to_string()).collect())].into()
}

#[test]
fn filters_by_tags_sets_and_text() {
    let (_, race, cards) = sort_fixture();
    let groups = [race];
    let card = |id: &str| cards.iter().find(|card| card.id == id).unwrap();
    let include_elf = CardFilterSelection { included_tags_by_group_id: tags("race", &["Elf"]), ..Default::default() };
    assert!(card_matches_filters(card("e"), &groups, &include_elf));
    assert!(!card_matches_filters(card("n"), &groups, &include_elf));
    let exclude_elf = CardFilterSelection { excluded_tags_by_group_id: tags("race", &["Elf"]), ..Default::default() };
    assert!(!card_matches_filters(card("e"), &groups, &exclude_elf));
    let only_set_a = CardFilterSelection { selected_set_ids: ["set_a".to_string()].into(), ..Default::default() };
    assert!(!card_matches_filters(card("d"), &groups, &only_set_a));
    assert!(card_matches_filters(card("u"), &groups, &only_set_a));
    let search = |text: &str| CardFilterSelection { search_query: text.to_string(), ..Default::default() };
    assert!(card_matches_filters(card("d"), &groups, &search("  dunADAN ")));
    assert!(!card_matches_filters(card("d"), &groups, &search("zzz")));
}

#[test]
fn extra_field_search() {
    let mut with_field = CardDefinition::new("x", "X Card");
    with_field.extra_fields.insert("episode", "Blood Harvest");
    let search = |text: &str| CardFilterSelection { search_query: text.to_string(), ..Default::default() };
    assert!(card_matches_filters(&with_field, &[], &search(":episode Blood")));
    assert!(card_matches_filters(&with_field, &[], &search(":Episode blood harvest")));
    assert!(!card_matches_filters(&with_field, &[], &search(":episode zzz")));
    assert!(!card_matches_filters(&with_field, &[], &search(":missingfield")));
    assert!(card_matches_filters(&with_field, &[], &search(":episode")));
    assert!(!card_matches_filters(&CardDefinition::new("d", "D"), &[], &search(":episode")));
}

#[test]
fn qualified_tags_do_not_cross_match_between_groups() {
    let (sets, race, cards) = sort_fixture();
    let faction = TagGroup { id: "faction".into(), name: "Faction".into(), tags: vec!["Elf".into(), "Human".into()] };
    let groups = [race, faction];
    let faction_elf =
        CardDefinition { types: vec!["Faction|Elf".into()], ..CardDefinition::new("e2", "Faction Elf Card") };
    let race_elf = CardDefinition { types: vec!["Race|Elf".into()], ..CardDefinition::new("eq", "Race Elf Card") };
    let include_elf = CardFilterSelection { included_tags_by_group_id: tags("race", &["Elf"]), ..Default::default() };
    assert!(card_matches_filters(&race_elf, &groups, &include_elf));
    assert!(!card_matches_filters(&faction_elf, &groups, &include_elf));
    let legacy_elf = cards.iter().find(|card| card.id == "e").unwrap();
    assert!(card_matches_filters(legacy_elf, &groups, &include_elf));
    let sorted = sort_card_definitions(&[&faction_elf, &race_elf], &CardSortOrder::from_key("race"), &sets, &groups);
    assert_eq!(ids_of_definitions(sorted), ["eq", "e2"]);
}

// ---------------------------------------------------------------------------
// save_remap
// ---------------------------------------------------------------------------

#[test]
fn matches_players_by_unique_name() {
    let saved = [host("saved_alice", "Alice"), player("saved_bob", "Bob")];
    let current = [host("cur_bob", "Bob"), player("cur_alice", "Alice")];
    let result = match_players_by_name(&saved, &current);
    let expected: HashMap<String, String> =
        [("saved_alice".into(), "cur_alice".into()), ("saved_bob".into(), "cur_bob".into())].into();
    assert_eq!(result.current_id_by_saved_id, expected);
    assert!(result.unmatched_saved_players.is_empty());

    let duplicate_names = [host("saved_alice", "Alice"), player("saved_alice2", "Alice")];
    let result = match_players_by_name(&duplicate_names, &[player("cur_alice", "Alice")]);
    assert!(result.current_id_by_saved_id.is_empty());
    assert_eq!(result.unmatched_saved_players.len(), 2);
    assert_eq!(result.unmatched_current_players.len(), 1);
}

fn alice_and_bob_mapping() -> HashMap<String, String> {
    [("saved_alice".into(), "cur_alice".into()), ("saved_bob".into(), "cur_bob".into())].into()
}

#[test]
fn builds_loaded_table_state() {
    let mut attached = widget("w1", BoardWidgetKind::Token);
    attached.owner_id = Some("saved_alice".into());
    attached.creator_id = Some("saved_bob".into());
    attached.attached_card_id = Some("c3".into());
    let mut saved = state(
        vec![host("saved_alice", "Alice"), player("saved_bob", "Bob")],
        vec![
            card("c1").definition("card_a").in_hand("saved_alice").z(1).build(),
            card("c2").definition("card_b").owner("saved_bob").on("c1").z(2).build(),
            card("c3").definition("gone_card").z(3).build(),
        ],
    )
    .with_widgets(vec![attached]);
    saved.revision = 42;
    saved.searches = vec![ActiveSearch {
        searcher_id: "x".into(),
        target_type: SearchTargetType::Zone,
        target_id: "z".into(),
        target_owner_id: None,
    }];
    saved.log = vec![LogEntry::new(1500, "Alice drew a card")];
    saved.current_player_id = Some("saved_bob".into());

    let current = [host("cur_bob", "Bob"), player("cur_alice", "Alice")];
    let valid: HashSet<String> = ["card_a".to_string(), "card_b".to_string()].into();
    let loaded = build_loaded_table_state(&saved, &current, &alice_and_bob_mapping(), &valid);
    let state = &loaded.state;
    assert_eq!(ids_of_players(&state.players), ["cur_alice", "cur_bob"]);
    assert_eq!(loaded.dropped_card_count, 1);
    assert_eq!(find(state, "c1").owner_id.as_deref(), Some("cur_alice"));
    assert_eq!(find(state, "c2").stack_parent_id.as_deref(), Some("c1"));
    assert_eq!(state.widgets()[0].owner_id.as_deref(), Some("cur_alice"));
    assert_eq!(state.widgets()[0].creator_id.as_deref(), Some("cur_bob"));
    assert_eq!(state.widgets()[0].attached_card_id, None);
    assert!(state.searches.is_empty());
    assert_eq!(state.revision, 42);
    assert_eq!(state.log, vec![LogEntry::new(1500, "Alice drew a card")]);
    assert_eq!(state.current_player_id.as_deref(), Some("cur_bob"));
}

#[test]
fn loaded_state_clears_unmatched_current_player_and_dangling_parents() {
    let mut saved = state(vec![host("saved_alice", "Alice")], vec![]);
    saved.current_player_id = Some("saved_bob".into());
    let mapping: HashMap<String, String> = [("saved_alice".into(), "cur_alice".into())].into();
    let current = [player("cur_alice", "Alice")];
    let loaded = build_loaded_table_state(&saved, &current, &mapping, &HashSet::new());
    assert_eq!(loaded.state.current_player_id, None);

    let saved = state(
        vec![host("saved_alice", "Alice")],
        vec![
            card("child").definition("card_a").on("missing_parent").z(1).build(),
            card("missing_parent").definition("gone_card").build(),
        ],
    );
    let valid: HashSet<String> = ["card_a".to_string()].into();
    let loaded = build_loaded_table_state(&saved, &current, &mapping, &valid);
    assert_eq!(loaded.state.cards().len(), 1);
    assert_eq!(loaded.state.cards()[0].stack_parent_id, None);
}
