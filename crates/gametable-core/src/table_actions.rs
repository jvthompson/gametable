//! Pure table-state transitions. Port of `src/core/table_actions.lua`.
//!
//! Every function takes the current [`TableState`] and returns the next one, leaving the input
//! untouched so callers can compare old and new (the session does this to write the game log).
//! The revision is bumped exactly where the original bumps it, including the cases where the
//! target id does not exist and the original still bumps.
//!
//! Cards and `cardToken` widgets share one z-order (the "card layer") so they interleave when
//! drawn; every other widget kind has its own z counter and always draws above the card layer.

use std::collections::{HashMap, HashSet};

use crate::deck_widget_zones::parse_deck_widget_zone_id;
use crate::models::{
    ActiveSearch, BOARD_WIDGET_COUNTER_MAXIMUM, BOARD_WIDGET_COUNTER_MINIMUM, BoardWidgetInstance, BoardWidgetKind,
    CardInstance, CardZone, RevealedToken, SearchTargetType, TableState, WidgetSizeOverride,
};
use crate::stack_utils::{stack_of, top_of};
use crate::util::{RandomIndexSource, shuffle_in_place};

/// A copy of the state with the revision bumped by one.
fn bumped(state: &TableState) -> TableState {
    let mut next = state.clone();
    next.revision += 1;
    next
}

/// One above the highest card z-index (0 when there are no cards).
pub fn next_z_index(state: &TableState) -> i64 {
    state.cards().iter().map(|card| card.z_index).max().map_or(0, |highest| highest + 1)
}

/// One below the lowest card z-index (0 when there are no cards).
pub fn previous_z_index(state: &TableState) -> i64 {
    state.cards().iter().map(|card| card.z_index).min().map_or(0, |lowest| lowest - 1)
}

/// Z-indexes of everything in the card layer: cards plus `cardToken` widgets.
fn card_layer_z_indexes(state: &TableState) -> impl Iterator<Item = i64> + '_ {
    let card_z_indexes = state.cards().iter().map(|card| card.z_index);
    let card_token_z_indexes =
        state.widgets().iter().filter(|widget| widget.kind == BoardWidgetKind::CardToken).map(|widget| widget.z_index);
    card_z_indexes.chain(card_token_z_indexes)
}

pub fn card_layer_next_z_index(state: &TableState) -> i64 {
    card_layer_z_indexes(state).max().map_or(0, |highest| highest + 1)
}

pub fn card_layer_previous_z_index(state: &TableState) -> i64 {
    card_layer_z_indexes(state).min().map_or(0, |lowest| lowest - 1)
}

/// One above the highest widget z-index (0 when there are no widgets).
fn next_widget_z_index(state: &TableState) -> i64 {
    state.widgets().iter().map(|widget| widget.z_index).max().map_or(0, |highest| highest + 1)
}

/// The z-index a widget of this kind gets when it is brought to the front.
fn front_z_index_for_widget_kind(state: &TableState, kind: Option<BoardWidgetKind>) -> i64 {
    if kind == Some(BoardWidgetKind::CardToken) { card_layer_next_z_index(state) } else { next_widget_z_index(state) }
}

/// Where an attachment anchor (a card, or a free card token) currently is.
fn attach_anchor_position(state: &TableState, anchor_id: &str) -> Option<(f64, f64)> {
    if let Some(card) = state.card_by_id(anchor_id) {
        return Some((card.x, card.y));
    }
    state
        .widget_by_id(anchor_id)
        .filter(|widget| widget.kind == BoardWidgetKind::CardToken)
        .map(|widget| (widget.x, widget.y))
}

/// Keeps every attached widget at its anchor's position plus its attach offset.
fn sync_attached_widgets(state: &mut TableState) {
    // Anchor positions are read before any widget moves, as in the original.
    let new_positions: Vec<Option<(f64, f64)>> = state
        .widgets()
        .iter()
        .map(|widget| {
            let anchor_id = widget.attached_card_id.as_deref()?;
            let (anchor_x, anchor_y) = attach_anchor_position(state, anchor_id)?;
            let new_position = (anchor_x + widget.attach_offset_x, anchor_y + widget.attach_offset_y);
            (new_position != (widget.x, widget.y)).then_some(new_position)
        })
        .collect();
    if new_positions.iter().all(Option::is_none) {
        return;
    }
    for (widget, new_position) in state.widgets_mut().iter_mut().zip(new_positions) {
        if let Some((x, y)) = new_position {
            widget.x = x;
            widget.y = y;
        }
    }
}

/// Lua's `c.unownable and NIL or c.ownerId`: unownable cards lose their owner when they move.
fn owner_after_moving(card: &CardInstance) -> Option<String> {
    if card.unownable { None } else { card.owner_id.clone() }
}

fn instance_ids_of(cards: &[&CardInstance]) -> HashSet<String> {
    cards.iter().map(|card| card.instance_id.clone()).collect()
}

// ---------------------------------------------------------------------------
// Cards
// ---------------------------------------------------------------------------

/// Moves one card onto the table at `(x, y)`, out of any pile or zone, and brings it to the front.
pub fn move_card(state: &TableState, instance_id: &str, x: f64, y: f64) -> TableState {
    let front_z = next_z_index(state);
    let mut next = bumped(state);
    for card in next.cards_mut().iter_mut().filter(|card| card.instance_id == instance_id) {
        card.x = x;
        card.y = y;
        card.zone = CardZone::Table;
        card.stack_parent_id = None;
        card.zone_id = None;
        card.z_index = front_z;
        card.owner_id = owner_after_moving(card);
    }
    sync_attached_widgets(&mut next);
    next
}

/// Moves a whole pile to `(x, y)`, bringing its top card to the front.
pub fn move_stack(state: &TableState, root_instance_id: &str, x: f64, y: f64) -> TableState {
    let stack = stack_of(state.cards(), root_instance_id);
    let Some(top) = top_of(&stack) else { return state.clone() };
    let top_id = top.instance_id.clone();
    let stack_ids = instance_ids_of(&stack);
    let front_z = next_z_index(state);
    let mut next = bumped(state);
    for card in next.cards_mut().iter_mut().filter(|card| stack_ids.contains(&card.instance_id)) {
        card.x = x;
        card.y = y;
        if card.instance_id == top_id {
            card.z_index = front_z;
        }
    }
    sync_attached_widgets(&mut next);
    next
}

/// Rotates the table cards of a pile. `step` is in quarter turns: 1 for 90°, 0.5 for 45°.
pub fn rotate_stack(state: &TableState, root_instance_id: &str, clockwise: bool, step: f64) -> TableState {
    let stack = stack_of(state.cards(), root_instance_id);
    let Some(top) = top_of(&stack) else { return state.clone() };
    let top_id = top.instance_id.clone();
    let stack_ids = instance_ids_of(&stack);
    let front_z = next_z_index(state);
    let delta = if clockwise { step } else { -step };
    let mut next = bumped(state);
    for card in next.cards_mut().iter_mut() {
        if stack_ids.contains(&card.instance_id) && card.zone == CardZone::Table {
            card.rotation_turns += delta;
            if card.instance_id == top_id {
                card.z_index = front_z;
            }
        }
    }
    next
}

/// Sends a pile's top card underneath every other card.
pub fn send_to_back(state: &TableState, root_instance_id: &str) -> TableState {
    let stack = stack_of(state.cards(), root_instance_id);
    let Some(top) = top_of(&stack) else { return state.clone() };
    let top_id = top.instance_id.clone();
    let back_z = previous_z_index(state);
    let mut next = bumped(state);
    for card in next.cards_mut().iter_mut().filter(|card| card.instance_id == top_id) {
        card.z_index = back_z;
    }
    next
}

/// Puts a card face up in `owner_id`'s hand.
pub fn move_to_hand(state: &TableState, instance_id: &str, owner_id: &str) -> TableState {
    let front_z = next_z_index(state);
    let mut next = bumped(state);
    for card in next.cards_mut().iter_mut().filter(|card| card.instance_id == instance_id) {
        card.zone = CardZone::Hand;
        card.owner_id = Some(owner_id.to_string());
        card.face_up = true;
        card.stack_parent_id = None;
        card.zone_id = None;
        card.rotation_turns = 0.0;
        card.z_index = front_z;
    }
    next
}

/// Cards ordered by z-index; equal z-indexes keep their original order.
pub fn sorted_by_z_index<'cards>(mut cards: Vec<&'cards CardInstance>) -> Vec<&'cards CardInstance> {
    cards.sort_by_key(|card| card.z_index); // stable
    cards
}

/// A player's hand in display order (by z-index).
pub fn hand_cards<'state>(state: &'state TableState, owner_id: Option<&str>) -> Vec<&'state CardInstance> {
    sorted_by_z_index(
        state
            .cards()
            .iter()
            .filter(|card| card.zone == CardZone::Hand && card.owner_id.as_deref() == owner_id)
            .collect(),
    )
}

/// Moves a card to `target_index` (0-based, clamped) within its owner's hand.
pub fn reorder_hand(state: &TableState, instance_id: &str, owner_id: Option<&str>, target_index: i64) -> TableState {
    let mut hand: Vec<String> = hand_cards(state, owner_id).iter().map(|card| card.instance_id.clone()).collect();
    let Some(current_index) = hand.iter().position(|id| id == instance_id) else { return state.clone() };
    let moved = hand.remove(current_index);
    let clamped_target = target_index.clamp(0, hand.len() as i64) as usize;
    hand.insert(clamped_target, moved);
    let base_z = next_z_index(state);
    let new_z_by_id: HashMap<String, i64> =
        hand.into_iter().enumerate().map(|(position, id)| (id, base_z + position as i64)).collect();
    let mut next = bumped(state);
    for card in next.cards_mut().iter_mut() {
        if let Some(&new_z) = new_z_by_id.get(&card.instance_id) {
            card.z_index = new_z;
        }
    }
    next
}

/// Turns a card over and brings it to the front.
pub fn flip_card(state: &TableState, instance_id: &str) -> TableState {
    let front_z = next_z_index(state);
    let mut next = bumped(state);
    for card in next.cards_mut().iter_mut().filter(|card| card.instance_id == instance_id) {
        card.face_up = !card.face_up;
        card.z_index = front_z;
    }
    next
}

/// Gives a card to a player, or releases ownership when `new_owner_id` is `None`.
/// Unownable cards always end up unowned.
pub fn give_card(state: &TableState, instance_id: &str, new_owner_id: Option<&str>) -> TableState {
    let front_z = next_z_index(state);
    let mut next = bumped(state);
    for card in next.cards_mut().iter_mut().filter(|card| card.instance_id == instance_id) {
        card.owner_id = if card.unownable { None } else { new_owner_id.map(str::to_string) };
        card.z_index = front_z;
    }
    next
}

/// Puts a card on top of another card, joining its pile.
pub fn stack_card(state: &TableState, instance_id: &str, onto_instance_id: &str) -> TableState {
    if instance_id == onto_instance_id {
        return state.clone();
    }
    let Some(target) = state.card_by_id(onto_instance_id) else { return state.clone() };
    let (target_x, target_y, target_zone) = (target.x, target.y, target.zone);
    let front_z = next_z_index(state);
    let mut next = bumped(state);
    for card in next.cards_mut().iter_mut().filter(|card| card.instance_id == instance_id) {
        card.x = target_x;
        card.y = target_y;
        card.zone = target_zone;
        card.stack_parent_id = Some(onto_instance_id.to_string());
        card.zone_id = None;
        card.z_index = front_z;
        card.owner_id = owner_after_moving(card);
    }
    sync_attached_widgets(&mut next);
    next
}

/// Moves a selection: the primary card goes to `(x, y)` and every passenger pile moves by the
/// same offset. The primary comes to the front, then each passenger pile's top card above it.
pub fn move_group(
    state: &TableState,
    primary_instance_id: &str,
    passenger_root_instance_ids: &[String],
    x: f64,
    y: f64,
) -> TableState {
    let Some(primary) = state.card_by_id(primary_instance_id) else { return state.clone() };
    let (delta_x, delta_y) = (x - primary.x, y - primary.y);
    let primary_new_z = next_z_index(state);
    let mut next_free_z = primary_new_z + 1;
    let mut passenger_ids: HashSet<String> = HashSet::new();
    let mut new_z_by_passenger_top_id: HashMap<String, i64> = HashMap::new();
    for root_id in passenger_root_instance_ids {
        let stack = stack_of(state.cards(), root_id);
        if let Some(top) = top_of(&stack) {
            passenger_ids.extend(stack.iter().map(|card| card.instance_id.clone()));
            new_z_by_passenger_top_id.insert(top.instance_id.clone(), next_free_z);
            next_free_z += 1;
        }
    }
    let mut next = bumped(state);
    for card in next.cards_mut().iter_mut() {
        if card.instance_id == primary_instance_id {
            card.x = x;
            card.y = y;
            card.zone = CardZone::Table;
            card.stack_parent_id = None;
            card.zone_id = None;
            card.z_index = primary_new_z;
            card.owner_id = owner_after_moving(card);
        } else if passenger_ids.contains(&card.instance_id) {
            card.x += delta_x;
            card.y += delta_y;
            if let Some(&new_z) = new_z_by_passenger_top_id.get(&card.instance_id) {
                card.z_index = new_z;
            }
        }
    }
    sync_attached_widgets(&mut next);
    next
}

/// Applies `change` to each drawn card and stacks them on the front in draw order.
fn apply_drawn(state: &TableState, drawn_ids: &[String], change: impl Fn(&mut CardInstance)) -> TableState {
    let base_z = next_z_index(state);
    let new_z_by_id: HashMap<&str, i64> =
        drawn_ids.iter().enumerate().map(|(position, id)| (id.as_str(), base_z + position as i64)).collect();
    let mut next = bumped(state);
    for card in next.cards_mut().iter_mut() {
        if let Some(&new_z) = new_z_by_id.get(card.instance_id.as_str()) {
            change(card);
            card.z_index = new_z;
        }
    }
    next
}

/// Result of a draw: the new state and the drawn card ids in draw order.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawResult {
    pub state: TableState,
    pub drawn_instance_ids: Vec<String>,
}

/// Draws up to `count` cards from the top of a table pile into `owner_id`'s hand. The pile's
/// root (bottom) card is drawn last.
pub fn draw_card(state: &TableState, pile_instance_id: &str, owner_id: &str, count: i64) -> DrawResult {
    let stack = stack_of(state.cards(), pile_instance_id);
    if stack.is_empty() || count <= 0 {
        return DrawResult { state: state.clone(), drawn_instance_ids: Vec::new() };
    }
    let mut remaining = stack;
    let mut drawn_ids = Vec::new();
    while (drawn_ids.len() as i64) < count && !remaining.is_empty() {
        let candidates: Vec<&CardInstance> = if remaining.len() > 1 {
            remaining.iter().copied().filter(|card| card.instance_id != pile_instance_id).collect()
        } else {
            remaining.clone()
        };
        let Some(top) = top_of(&candidates) else { break };
        let top_id = top.instance_id.clone();
        remaining.retain(|card| card.instance_id != top_id);
        drawn_ids.push(top_id);
    }
    let state = apply_drawn(state, &drawn_ids, |card| {
        card.zone = CardZone::Hand;
        card.owner_id = Some(owner_id.to_string());
        card.face_up = true;
        card.stack_parent_id = None;
        card.rotation_turns = 0.0;
    });
    DrawResult { state, drawn_instance_ids: drawn_ids }
}

/// Cards in a zone belonging to `zone_owner_id` (`None` for a shared zone), in card order.
pub fn zone_cards<'state>(
    state: &'state TableState,
    zone_id: &str,
    zone_owner_id: Option<&str>,
) -> Vec<&'state CardInstance> {
    state
        .cards()
        .iter()
        .filter(|card| {
            card.zone == CardZone::Zone
                && card.zone_id.as_deref() == Some(zone_id)
                && card.owner_id.as_deref() == zone_owner_id
        })
        .collect()
}

/// Draws up to `count` cards from the top of a zone into `to_owner_id`'s hand.
pub fn draw_from_zone(
    state: &TableState,
    zone_id: &str,
    zone_owner_id: Option<&str>,
    to_owner_id: &str,
    count: i64,
) -> DrawResult {
    let mut remaining = zone_cards(state, zone_id, zone_owner_id);
    if remaining.is_empty() || count <= 0 {
        return DrawResult { state: state.clone(), drawn_instance_ids: Vec::new() };
    }
    let mut drawn_ids = Vec::new();
    while (drawn_ids.len() as i64) < count {
        let Some(top) = top_of(&remaining) else { break };
        let top_id = top.instance_id.clone();
        remaining.retain(|card| card.instance_id != top_id);
        drawn_ids.push(top_id);
    }
    let state = apply_drawn(state, &drawn_ids, |card| {
        card.zone = CardZone::Hand;
        card.owner_id = Some(to_owner_id.to_string());
        card.face_up = true;
        card.zone_id = None;
        card.rotation_turns = 0.0;
    });
    DrawResult { state, drawn_instance_ids: drawn_ids }
}

/// Adds new cards on top of everything, numbering their z-indexes in order.
pub fn deal_new_cards(state: &TableState, new_cards: Vec<CardInstance>) -> TableState {
    if new_cards.is_empty() {
        return state.clone();
    }
    let base_z = next_z_index(state);
    let mut next = bumped(state);
    let cards = next.cards_mut();
    for (position, mut card) in new_cards.into_iter().enumerate() {
        card.z_index = base_z + position as i64;
        cards.push(card);
    }
    next
}

/// Puts a card into a zone (on top, or at the bottom when `to_bottom`), at the zone's position.
/// A card already in that zone is left alone.
pub fn return_to_zone(
    state: &TableState,
    instance_id: &str,
    zone_id: &str,
    zone_owner_id: Option<&str>,
    face_up: bool,
    to_bottom: bool,
) -> TableState {
    let cards_in_zone = zone_cards(state, zone_id, zone_owner_id);
    if cards_in_zone.iter().any(|card| card.instance_id == instance_id) {
        return state.clone();
    }
    let new_z = match cards_in_zone.iter().map(|card| card.z_index).min() {
        Some(lowest) if to_bottom => lowest - 1,
        _ => next_z_index(state),
    };
    let anchor_position = cards_in_zone.first().map(|anchor| (anchor.x, anchor.y));
    let mut next = bumped(state);
    for card in next.cards_mut().iter_mut().filter(|card| card.instance_id == instance_id) {
        if let Some((anchor_x, anchor_y)) = anchor_position {
            card.x = anchor_x;
            card.y = anchor_y;
        }
        card.zone = CardZone::Zone;
        card.zone_id = Some(zone_id.to_string());
        card.owner_id = zone_owner_id.map(str::to_string);
        card.face_up = face_up;
        card.stack_parent_id = None;
        card.rotation_turns = 0.0;
        card.z_index = new_z;
    }
    next
}

/// Shuffles the given cards' z-order on top of everything and turns them face down.
fn reshuffle(state: &TableState, cards_to_shuffle: &[&CardInstance], random: &mut dyn RandomIndexSource) -> TableState {
    let mut shuffled_ids: Vec<String> = cards_to_shuffle.iter().map(|card| card.instance_id.clone()).collect();
    shuffle_in_place(&mut shuffled_ids, random);
    let base_z = next_z_index(state);
    let new_z_by_id: HashMap<String, i64> =
        shuffled_ids.into_iter().enumerate().map(|(position, id)| (id, base_z + position as i64)).collect();
    let mut next = bumped(state);
    for card in next.cards_mut().iter_mut() {
        if let Some(&new_z) = new_z_by_id.get(&card.instance_id) {
            card.z_index = new_z;
            card.face_up = false;
        }
    }
    next
}

/// Shuffles a zone (fewer than two cards: no change).
pub fn shuffle_zone(
    state: &TableState,
    zone_id: &str,
    zone_owner_id: Option<&str>,
    random: &mut dyn RandomIndexSource,
) -> TableState {
    let cards_in_zone = zone_cards(state, zone_id, zone_owner_id);
    if cards_in_zone.len() < 2 {
        return state.clone();
    }
    reshuffle(state, &cards_in_zone, random)
}

/// Shuffles a table pile (fewer than two cards: no change).
pub fn shuffle_pile(state: &TableState, pile_root_instance_id: &str, random: &mut dyn RandomIndexSource) -> TableState {
    let stack = stack_of(state.cards(), pile_root_instance_id);
    if stack.len() < 2 {
        return state.clone();
    }
    reshuffle(state, &stack, random)
}

// ---------------------------------------------------------------------------
// Searches
// ---------------------------------------------------------------------------

/// Starts (or replaces) a player's search.
pub fn start_search(
    state: &TableState,
    searcher_id: &str,
    target_type: SearchTargetType,
    target_id: &str,
    target_owner_id: Option<&str>,
) -> TableState {
    let mut next = bumped(state);
    next.searches.retain(|search| search.searcher_id != searcher_id);
    next.searches.push(ActiveSearch {
        searcher_id: searcher_id.to_string(),
        target_type,
        target_id: target_id.to_string(),
        target_owner_id: target_owner_id.map(str::to_string),
    });
    next
}

/// Ends a player's search (no change when they were not searching).
pub fn stop_search(state: &TableState, searcher_id: &str) -> TableState {
    if !state.searches.iter().any(|search| search.searcher_id == searcher_id) {
        return state.clone();
    }
    let mut next = bumped(state);
    next.searches.retain(|search| search.searcher_id != searcher_id);
    next
}

// ---------------------------------------------------------------------------
// Board widgets
// ---------------------------------------------------------------------------

fn append_widget(state: &TableState, widget: BoardWidgetInstance) -> TableState {
    let mut next = bumped(state);
    next.widgets_mut().push(widget);
    next
}

/// Applies `change` to every widget with this id (as the original's `mapWidgets` does) in a
/// bumped copy of the state. The state is bumped even when no widget has the id.
fn bumped_with_each_widget_change(
    state: &TableState,
    instance_id: &str,
    change: impl Fn(&mut BoardWidgetInstance),
) -> TableState {
    let mut next = bumped(state);
    for widget in next.widgets_mut().iter_mut().filter(|widget| widget.instance_id == instance_id) {
        change(widget);
    }
    next
}

/// Places a new widget of `kind` at `(x, y)` on top of its layer.
pub fn create_widget(
    state: &TableState,
    instance_id: &str,
    kind: BoardWidgetKind,
    x: f64,
    y: f64,
    owner_id: Option<&str>,
    image_path: Option<&str>,
) -> TableState {
    let z_index = front_z_index_for_widget_kind(state, Some(kind));
    let mut widget = BoardWidgetInstance::new(instance_id, kind, x, y, z_index);
    widget.owner_id = owner_id.map(str::to_string);
    widget.image_path = image_path.map(str::to_string);
    append_widget(state, widget)
}

/// Adds a fully specified widget (used when dealing docked counters); its z-index is assigned here.
pub fn add_widget(state: &TableState, mut widget: BoardWidgetInstance) -> TableState {
    widget.z_index = next_widget_z_index(state);
    append_widget(state, widget)
}

/// Adds an arrow from `(x, y)` to `(x2, y2)`.
pub fn create_arrow(
    state: &TableState,
    instance_id: &str,
    x: f64,
    y: f64,
    x2: f64,
    y2: f64,
    creator_id: Option<&str>,
) -> TableState {
    let mut arrow = BoardWidgetInstance::new(instance_id, BoardWidgetKind::Arrow, x, y, next_widget_z_index(state));
    arrow.x2 = Some(x2);
    arrow.y2 = Some(y2);
    arrow.creator_id = creator_id.map(str::to_string);
    append_widget(state, arrow)
}

/// Moves a widget, detaching it from any card, and brings it to the front of its layer.
/// Widgets riding on it (when it is a card token) move with it.
pub fn move_widget(state: &TableState, instance_id: &str, x: f64, y: f64) -> TableState {
    let moving_kind = state.widget_by_id(instance_id).map(|widget| widget.kind);
    let front_z = front_z_index_for_widget_kind(state, moving_kind);
    let mut next = bumped_with_each_widget_change(state, instance_id, |widget| {
        widget.x = x;
        widget.y = y;
        widget.z_index = front_z;
        widget.attached_card_id = None;
        widget.attach_offset_x = 0.0;
        widget.attach_offset_y = 0.0;
    });
    sync_attached_widgets(&mut next);
    next
}

/// Attaches a widget to a card (or a free card token) so it follows it. Card tokens themselves
/// can never be attached.
pub fn attach_widget_to_card(state: &TableState, instance_id: &str, card_id: &str, x: f64, y: f64) -> TableState {
    if state.widget_by_id(instance_id).is_some_and(|widget| widget.kind == BoardWidgetKind::CardToken) {
        return state.clone();
    }
    let Some((anchor_x, anchor_y)) = attach_anchor_position(state, card_id) else { return state.clone() };
    let front_z = next_widget_z_index(state);
    bumped_with_each_widget_change(state, instance_id, |widget| {
        widget.x = x;
        widget.y = y;
        widget.z_index = front_z;
        widget.attached_card_id = Some(card_id.to_string());
        widget.attach_offset_x = x - anchor_x;
        widget.attach_offset_y = y - anchor_y;
    })
}

/// Sets a counter's value, clamped to 0..=99999.
pub fn set_widget_value(state: &TableState, instance_id: &str, value: f64) -> TableState {
    let clamped = if value < BOARD_WIDGET_COUNTER_MINIMUM {
        BOARD_WIDGET_COUNTER_MINIMUM
    } else if value > BOARD_WIDGET_COUNTER_MAXIMUM {
        BOARD_WIDGET_COUNTER_MAXIMUM
    } else {
        value
    };
    bumped_with_each_widget_change(state, instance_id, |widget| widget.value = clamped)
}

/// Removes a widget. A deck widget spills its held cards onto the table at its position, and any
/// widgets attached to the removed one are detached.
pub fn delete_widget(state: &TableState, instance_id: &str) -> TableState {
    let removed = state.widgets().iter().find(|widget| widget.instance_id == instance_id).cloned();
    let base_z = next_z_index(state);
    let mut next = bumped(state);

    if let Some(removed) = removed.filter(|widget| widget.kind == BoardWidgetKind::DeckBuilder) {
        let mut spilled_count = 0;
        for card in next.cards_mut().iter_mut().filter(|card| card.zone == CardZone::Zone) {
            let held_by_removed_widget = parse_deck_widget_zone_id(card.zone_id.as_deref().unwrap_or(""))
                .is_some_and(|parsed| parsed.widget_instance_id == instance_id);
            if held_by_removed_widget {
                card.zone = CardZone::Table;
                card.zone_id = None;
                card.stack_parent_id = None;
                card.x = removed.x;
                card.y = removed.y;
                card.z_index = base_z + spilled_count;
                card.owner_id = owner_after_moving(card);
                spilled_count += 1;
            }
        }
    }

    let widgets = next.widgets_mut();
    widgets.retain(|widget| widget.instance_id != instance_id);
    for widget in widgets.iter_mut().filter(|widget| widget.attached_card_id.as_deref() == Some(instance_id)) {
        widget.attached_card_id = None;
        widget.attach_offset_x = 0.0;
        widget.attach_offset_y = 0.0;
    }
    next
}

pub fn set_widget_colors(
    state: &TableState,
    instance_id: &str,
    background_color: crate::color::ArgbColor,
    text_color: crate::color::ArgbColor,
) -> TableState {
    bumped_with_each_widget_change(state, instance_id, |widget| {
        widget.background_color = background_color;
        widget.text_color = text_color;
    })
}

/// Sets a token's label and its colour.
pub fn set_widget_text(
    state: &TableState,
    instance_id: &str,
    text: &str,
    text_color: crate::color::ArgbColor,
) -> TableState {
    bumped_with_each_widget_change(state, instance_id, |widget| {
        widget.text = text.to_string();
        widget.text_color = text_color;
    })
}

/// Sets (or with `None`, clears) a token's size override.
pub fn set_widget_size(state: &TableState, instance_id: &str, size_override: Option<WidgetSizeOverride>) -> TableState {
    bumped_with_each_widget_change(state, instance_id, |widget| widget.size_override = size_override)
}

/// Rotates a free token and brings it to the front. `step` is in quarter turns.
pub fn rotate_widget(state: &TableState, instance_id: &str, clockwise: bool, step: f64) -> TableState {
    let delta = if clockwise { step } else { -step };
    let kind = state.widget_by_id(instance_id).map(|widget| widget.kind);
    let front_z = front_z_index_for_widget_kind(state, kind);
    bumped_with_each_widget_change(state, instance_id, |widget| {
        widget.rotation_turns += delta;
        widget.z_index = front_z;
    })
}

/// Flips a free token and brings it to the front.
pub fn flip_widget(state: &TableState, instance_id: &str) -> TableState {
    let kind = state.widget_by_id(instance_id).map(|widget| widget.kind);
    let front_z = front_z_index_for_widget_kind(state, kind);
    bumped_with_each_widget_change(state, instance_id, |widget| {
        widget.face_up = !widget.face_up;
        widget.z_index = front_z;
    })
}

/// Sends a card token behind every card and card token.
pub fn send_widget_to_back(state: &TableState, instance_id: &str) -> TableState {
    if state.widget_by_id(instance_id).is_none() {
        return state.clone();
    }
    let back_z = card_layer_previous_z_index(state);
    bumped_with_each_widget_change(state, instance_id, |widget| widget.z_index = back_z)
}

/// Copies a widget's appearance (not its ownership, attachment or bag contents) to a new widget.
pub fn duplicate_widget(
    state: &TableState,
    source_instance_id: &str,
    new_instance_id: &str,
    x: f64,
    y: f64,
) -> TableState {
    let Some(source) = state.widgets().iter().find(|widget| widget.instance_id == source_instance_id) else {
        return state.clone();
    };
    let z_index = front_z_index_for_widget_kind(state, Some(source.kind));
    let mut copy = BoardWidgetInstance::new(new_instance_id, source.kind, x, y, z_index);
    copy.value = source.value;
    copy.background_color = source.background_color;
    copy.text_color = source.text_color;
    copy.image_path = source.image_path.clone();
    copy.size_override = source.size_override;
    copy.rotation_turns = source.rotation_turns;
    copy.face_up = source.face_up;
    copy.text = source.text.clone();
    append_widget(state, copy)
}

/// Adds several widgets with sequential z-indexes in one revision.
pub fn add_widgets(state: &TableState, new_widgets: Vec<BoardWidgetInstance>) -> TableState {
    if new_widgets.is_empty() {
        return state.clone();
    }
    let base_z = next_widget_z_index(state);
    let mut next = bumped(state);
    let widgets = next.widgets_mut();
    for (position, mut widget) in new_widgets.into_iter().enumerate() {
        widget.z_index = base_z + position as i64;
        widgets.push(widget);
    }
    next
}

// ---------------------------------------------------------------------------
// Token bags
// ---------------------------------------------------------------------------

/// Moves one copy of `image_path` from the bag's remaining tokens to its revealed tokens.
pub fn reveal_bag_token(state: &TableState, instance_id: &str, image_path: &str, slot_id: &str) -> TableState {
    bumped_with_each_widget_change(state, instance_id, |bag| {
        if let Some(position) = bag.bag_remaining.iter().position(|remaining| remaining == image_path) {
            bag.bag_remaining.remove(position);
        }
        bag.bag_revealed.push(RevealedToken::new(slot_id, image_path));
    })
}

/// Removes a revealed token; it goes back into the bag only when `return_to_bag`.
pub fn resolve_bag_token(state: &TableState, instance_id: &str, slot_id: &str, return_to_bag: bool) -> TableState {
    bumped_with_each_widget_change(state, instance_id, |bag| {
        // Lua keeps the last revealed entry with this slot id and drops all of them.
        let Some(resolved) = bag.bag_revealed.iter().rev().find(|revealed| revealed.slot_id == slot_id).cloned() else {
            return;
        };
        bag.bag_revealed.retain(|revealed| revealed.slot_id != slot_id);
        if return_to_bag {
            bag.bag_remaining.push(resolved.image_path);
        }
    })
}

/// Takes a revealed token out of the bag and puts it on the table as a new image token.
pub fn place_bag_token_on_table(
    state: &TableState,
    instance_id: &str,
    slot_id: &str,
    new_instance_id: &str,
    x: f64,
    y: f64,
    owner_id: Option<&str>,
    size_override: Option<WidgetSizeOverride>,
) -> TableState {
    let revealed_entry = state
        .widgets()
        .iter()
        .filter(|widget| widget.instance_id == instance_id)
        .flat_map(|widget| widget.bag_revealed.iter())
        .filter(|revealed| revealed.slot_id == slot_id)
        .last()
        .cloned();
    let Some(revealed_entry) = revealed_entry else { return state.clone() };
    let front_z = next_widget_z_index(state);
    let mut next = bumped_with_each_widget_change(state, instance_id, |bag| {
        bag.bag_revealed.retain(|revealed| revealed.slot_id != slot_id);
    });
    let mut placed = BoardWidgetInstance::new(new_instance_id, BoardWidgetKind::ImageToken, x, y, front_z);
    placed.image_path = Some(revealed_entry.image_path);
    placed.owner_id = owner_id.map(str::to_string);
    placed.size_override = size_override;
    next.widgets_mut().push(placed);
    next
}

/// Refills the bag and clears its revealed tokens.
pub fn reset_bag(state: &TableState, instance_id: &str, fresh_remaining: &[String]) -> TableState {
    bumped_with_each_widget_change(state, instance_id, |bag| {
        bag.bag_remaining = fresh_remaining.to_vec();
        bag.bag_revealed.clear();
    })
}

/// Replaces the bag's remaining tokens, leaving revealed ones alone.
pub fn set_bag_remaining(state: &TableState, instance_id: &str, bag_remaining: &[String]) -> TableState {
    bumped_with_each_widget_change(state, instance_id, |bag| bag.bag_remaining = bag_remaining.to_vec())
}

/// Removes an image token from the table, putting its image into the bag when `return_to_bag`.
pub fn return_image_token_to_bag(
    state: &TableState,
    bag_instance_id: &str,
    token_instance_id: &str,
    return_to_bag: bool,
) -> TableState {
    let Some(token) = state.widgets().iter().filter(|widget| widget.instance_id == token_instance_id).last().cloned()
    else {
        return state.clone();
    };
    let mut next = bumped(state);
    let widgets = next.widgets_mut();
    widgets.retain(|widget| widget.instance_id != token_instance_id);
    if return_to_bag && let Some(image_path) = token.image_path {
        for bag in widgets.iter_mut().filter(|widget| widget.instance_id == bag_instance_id) {
            bag.bag_remaining.push(image_path.clone());
        }
    }
    next
}
