//! Mouse input on the table: drags and drops, box selection, arrows, panning and zooming,
//! clicks. Ports of `T:processInput`, the drag lifecycle and the drop handlers (`_handleDragEnd`
//! and friends) of `src/ui/table/table_screen.lua`. Input runs at the start of each frame against
//! the previous frame's hit items.

use std::collections::HashSet;

use gametable_core::geometry::{WORLD_HEIGHT, WORLD_WIDTH, local_pixel_to_canonical};
use gametable_core::models::{BoardWidgetInstance, BoardWidgetKind, CardInstance, ZoneKind, ZoneWidgetKind};
use gametable_core::uuid::new_v4_uuid;

use crate::app::Frame;
use crate::graphics::draw_list::Color;
use crate::sound::SoundEffect;
use crate::table::actions::TableAction;
use crate::table::card_render::{CARD_HEIGHT, CARD_WIDTH, orientation_turns};
use crate::table::screen::{
    DRAG_THRESHOLD, MAX_ZOOM, MIN_ZOOM, PREVIEW_THROTTLE, PickupItem, SHARED_OWNER_KEY, STACK_HIT_RADIUS, TableScreen,
    top_card,
};
use crate::table::selection::{box_hits, half_extents, rect_from_points};
use crate::table::view::{HitItem, ItemKind, ShuffleTarget};
use crate::ui::context::Rect;

/// A card (or pile) being dragged and how it was grabbed.
#[derive(Debug, Clone, PartialEq)]
pub struct CardSource {
    pub card: CardInstance,
    pub root_id: Option<String>,
    pub cards: Vec<CardInstance>,
    /// Alt lifts the cards stacked on it too (not for an opponent's card).
    pub allow_group: bool,
    pub angle: f32,
    pub border: Option<Color>,
    /// Search results show their face even when face down.
    pub force_face_up: bool,
}

impl CardSource {
    pub fn plain(card: CardInstance) -> CardSource {
        CardSource {
            card,
            root_id: None,
            cards: Vec::new(),
            allow_group: false,
            angle: 0.0,
            border: None,
            force_face_up: false,
        }
    }
}

/// What a press would drag (`src.kind` and its payload).
#[derive(Debug, Clone, PartialEq)]
pub enum DragSourceKind {
    Card(CardSource),
    Pile(CardSource),
    Widget(BoardWidgetInstance),
    Token(BoardWidgetInstance),
    CardToken(BoardWidgetInstance),
    RevealedBagToken { widget: BoardWidgetInstance, slot_id: String, image_path: String },
    Library { definition_id: String },
    ImageToken { image_file: String },
    NewToken,
    NewCardToken,
    Selection { anchor_root_id: Option<String>, anchor_token_id: Option<String> },
}

/// What a press that never became a drag does.
#[derive(Debug, Clone, PartialEq)]
pub enum ClickAction {
    DrawCard { pile_instance_id: String },
    ReturnBagToken { instance_id: String, slot_id: String },
    ToggleSelected(Box<HitItem>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct DragSource {
    pub kind: DragSourceKind,
    /// The dragged thing's screen centre at the press.
    pub center: (f32, f32),
    pub scale: f32,
    pub on_click: Option<ClickAction>,
}

impl DragSource {
    pub fn new(kind: DragSourceKind, center: (f32, f32), scale: f32) -> DragSource {
        DragSource { kind, center, scale, on_click: None }
    }
}

/// A press waiting to become a drag (`T.pending`).
#[derive(Debug, Clone, PartialEq)]
pub struct PendingPress {
    pub source: DragSource,
    pub press: (f32, f32),
}

/// A drag in progress (`T.drag`).
#[derive(Debug, Clone, PartialEq)]
pub struct Drag {
    pub started: bool,
    pub source: DragSource,
    pub grab: (f32, f32),
    pub grab_at_start: (f32, f32),
    pub scale: f32,
    pub scale_at_start: f32,
    pub primary_id: Option<String>,
    /// The items moving together (cards and card tokens), the dragged one first for cards.
    pub group: Vec<PickupItem>,
    /// Every instance in the dragged group (ghosted while dragging); empty for a single item.
    pub group_ids: HashSet<String>,
    pub token_ids: HashSet<String>,
    pub widget_id: Option<String>,
    pub angle: f32,
    /// Alt at the start of the drag (releasing it mid-drag doesn't change what moves).
    pub alt_down: bool,
    pub anchor_root_id: Option<String>,
    pub anchor_token_id: Option<String>,
    /// A selection's cards bottom-most first.
    pub draw_order: Vec<CardInstance>,
}

/// A selection box being dragged out (world units).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxSelect {
    pub start: (f32, f32),
    pub current: Option<(f32, f32)>,
    pub additive: bool,
}

/// An arrow being drawn with Tab (world units).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrowDrag {
    pub start: (f32, f32),
    pub current: (f32, f32),
}

/// A middle-button pan in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MiddlePan {
    pub press: (f32, f32),
    pub camera: (f32, f32),
}

fn in_rect(rect: Option<Rect>, x: f32, y: f32) -> bool {
    rect.is_some_and(|rect| x >= rect.x && y >= rect.y && x < rect.x + rect.width && y < rect.y + rect.height)
}

fn is_draggable(kind: &ItemKind) -> bool {
    matches!(
        kind,
        ItemKind::TableCard { .. }
            | ItemKind::Pile { .. }
            | ItemKind::HandCard { .. }
            | ItemKind::Zone { .. }
            | ItemKind::Counter { .. }
            | ItemKind::Token { .. }
            | ItemKind::PackGenerator { .. }
            | ItemKind::DeckHeader { .. }
            | ItemKind::RevealedBagToken { .. }
    )
}

/// Item kinds that belong to the table surface (Tab draws arrows over them; right-click falls
/// through to the board menu).
pub(super) fn is_table_kind(kind: &ItemKind) -> bool {
    matches!(
        kind,
        ItemKind::Table
            | ItemKind::TableCard { .. }
            | ItemKind::Pile { .. }
            | ItemKind::Counter { .. }
            | ItemKind::Token { .. }
            | ItemKind::PackGenerator { .. }
            | ItemKind::DeckBody { .. }
            | ItemKind::DeckHeader { .. }
            | ItemKind::DeckSave { .. }
            | ItemKind::Shuffle { .. }
    )
}

impl TableScreen {
    pub(super) fn is_over_table(&self, x: f32, y: f32) -> bool {
        in_rect(self.table_rect, x, y)
    }

    pub(super) fn is_over_local_hand(&self, x: f32, y: f32) -> bool {
        in_rect(self.hand_rect, x, y)
    }

    pub(super) fn docked_zone_id_at(&self, x: f32, y: f32) -> Option<String> {
        self.drop_zones.iter().find(|zone| in_rect(Some(zone.rect), x, y)).map(|zone| zone.zone_id.clone())
    }

    /// Only Token Bag squares, so a dragged card is never put away in a bag.
    fn bag_zone_id_at(&self, x: f32, y: f32) -> Option<String> {
        self.bag_drop_zones.iter().find(|zone| in_rect(Some(zone.rect), x, y)).map(|zone| zone.zone_id.clone())
    }

    /// Where among the other hand cards a drop at screen x lands.
    fn hand_drop_index(&self, other_ids: &[String], x: f32) -> i64 {
        for (index, id) in other_ids.iter().enumerate() {
            if let Some((_, center_x)) = self.hand_card_centers.iter().find(|(card_id, _)| card_id == id)
                && x < *center_x
            {
                return index as i64;
            }
        }
        other_ids.len() as i64
    }

    /// The nearest pile top within the stacking radius, other than the excluded ones.
    fn find_stack_target(&self, excluded: &HashSet<String>, world_x: f32, world_y: f32) -> Option<CardInstance> {
        let mut best: Option<(f64, &CardInstance)> = None;
        for card in &self.derived.tops {
            if excluded.contains(&card.instance_id) {
                continue;
            }
            let (x, y) = self.canonical_to_world(card.x, card.y);
            let distance = (f64::from(x - world_x).powi(2) + f64::from(y - world_y).powi(2)).sqrt();
            if distance < STACK_HIT_RADIUS && best.is_none_or(|(best_distance, _)| distance < best_distance) {
                best = Some((distance, card));
            }
        }
        best.map(|(_, card)| card.clone())
    }

    /// Like [`find_stack_target`](Self::find_stack_target) but a card token of yours can anchor too.
    /// Returns (instance id, x, y).
    fn find_attach_target(&self, excluded: &HashSet<String>, world_x: f32, world_y: f32) -> Option<(String, f64, f64)> {
        if let Some(card) = self.find_stack_target(excluded, world_x, world_y) {
            return Some((card.instance_id, card.x, card.y));
        }
        let mut best: Option<(f64, &BoardWidgetInstance)> = None;
        for widget in self.state.widgets() {
            if widget.kind != BoardWidgetKind::CardToken
                || widget.owner_id.as_deref() != Some(self.acting_player_id.as_str())
                || excluded.contains(&widget.instance_id)
            {
                continue;
            }
            let (x, y) = self.canonical_to_world(widget.x, widget.y);
            let distance = (f64::from(x - world_x).powi(2) + f64::from(y - world_y).powi(2)).sqrt();
            if distance < STACK_HIT_RADIUS && best.is_none_or(|(best_distance, _)| distance < best_distance) {
                best = Some((distance, widget));
            }
        }
        best.map(|(_, widget)| (widget.instance_id.clone(), widget.x, widget.y))
    }

    /// `T:pickupGroup`: what's visually stacked on `dragged` (transitive overlap with a higher
    /// z-index), sorted by z-index.
    pub(super) fn pickup_group(&self, dragged: &PickupItem) -> Vec<PickupItem> {
        let candidates = &self.derived.pickup_candidates;
        let world_of = |item: &PickupItem| {
            let (x, y) = item.position();
            self.canonical_to_world(x, y)
        };
        let overlaps = |first: &PickupItem, second: &PickupItem| {
            let (a, b) = (world_of(first), world_of(second));
            (a.0 - b.0).abs() < CARD_WIDTH && (a.1 - b.1).abs() < CARD_HEIGHT
        };
        let mut result = vec![dragged.clone()];
        let mut visited: HashSet<String> = HashSet::from([dragged.instance_id().to_string()]);
        let mut frontier = vec![dragged.clone()];
        while let Some(current) = frontier.pop() {
            for other in candidates {
                if !visited.contains(other.instance_id())
                    && other.z_index() > current.z_index()
                    && overlaps(&current, other)
                {
                    visited.insert(other.instance_id().to_string());
                    result.push(other.clone());
                    frontier.push(other.clone());
                }
            }
        }
        result.sort_by_key(PickupItem::z_index);
        result
    }

    /// Moves piles and loose tokens by a world delta, keeping their layout; anything that would
    /// end up (partly) off the table is pulled fully back onto it instead.
    pub(super) fn move_items_by(
        &mut self,
        frame: &mut Frame,
        root_ids: &[String],
        token_ids: &[String],
        delta: (f32, f32),
    ) {
        let mut in_bounds = Vec::new();
        let mut primary: Option<(String, f64, f64)> = None;
        for root_id in root_ids {
            let (Some(root), Some(group)) =
                (self.derived.cards_by_id.get(root_id), self.derived.pile_groups.get(root_id))
            else {
                continue;
            };
            let (world_x, world_y) = self.canonical_to_world(root.x, root.y);
            let (new_x, new_y) = (world_x + delta.0, world_y + delta.1);
            let half = top_card(group).map_or((CARD_WIDTH / 2.0, CARD_HEIGHT / 2.0), |top| self.card_half_extents(top));
            let (clamped_x, clamped_y) = crate::table::selection::clamp_world_center(new_x, new_y, half.0, half.1);
            if clamped_x == new_x && clamped_y == new_y {
                in_bounds.push(root_id.clone());
                if primary.is_none() {
                    let (fraction_x, fraction_y) = self.world_to_canonical(new_x, new_y);
                    primary = Some((root_id.clone(), fraction_x, fraction_y));
                }
            } else {
                let (x, y) = self.world_to_canonical(clamped_x, clamped_y);
                self.perform(TableAction::MoveStack { root_instance_id: root_id.clone(), x, y });
            }
        }
        // The primary is listed among the passengers too, so its whole pile travels with it.
        if let Some((primary_instance_id, x, y)) = primary {
            self.perform(TableAction::MoveGroup { primary_instance_id, passenger_root_ids: in_bounds, x, y });
        }
        for token_id in token_ids {
            let Some(widget) = self.widget_by_id(token_id).cloned() else { continue };
            let (width, height) = self.widget_instance_size(frame, &widget);
            let (world_x, world_y) = self.canonical_to_world(widget.x, widget.y);
            let (x, y) = crate::table::selection::clamp_world_center(
                world_x + delta.0,
                world_y + delta.1,
                width / 2.0,
                height / 2.0,
            );
            let (x, y) = self.world_to_canonical(x, y);
            self.perform(TableAction::MoveWidget { instance_id: token_id.clone(), x, y });
        }
    }

    /// Moves a mixed group (cards and card tokens) by the delta from `origin` to the drop point.
    fn move_mixed_group_by(&mut self, frame: &mut Frame, group: &[PickupItem], origin: (f64, f64), drop: (f32, f32)) {
        let mut roots: Vec<String> = Vec::new();
        let mut token_ids = Vec::new();
        for item in group {
            match item {
                PickupItem::CardToken(widget) => token_ids.push(widget.instance_id.clone()),
                PickupItem::Card(card) => {
                    let root_id = self.root_id_of(card);
                    if !roots.contains(&root_id) {
                        roots.push(root_id);
                    }
                }
            }
        }
        let (origin_x, origin_y) = self.canonical_to_world(origin.0, origin.1);
        let (world_x, world_y) = self.screen_to_world(drop.0, drop.1);
        self.move_items_by(frame, &roots, &token_ids, (world_x - origin_x, world_y - origin_y));
    }

    /// Sends cards to a zone keeping their order (to the bottom the top-most goes first).
    pub(super) fn send_cards_to_zone(&mut self, cards: &[CardInstance], zone_id: &str, to_bottom: bool) {
        let ordered: Vec<&CardInstance> = if to_bottom { cards.iter().rev().collect() } else { cards.iter().collect() };
        for card in ordered {
            self.perform(TableAction::ReturnToZone {
                instance_id: card.instance_id.clone(),
                zone_id: zone_id.to_string(),
                to_bottom,
            });
        }
    }

    /// "Create Pile Here": the selected cards stacked into one pile at the point, in order;
    /// selected tokens attach to it.
    pub(super) fn create_pile_at(&mut self, x: f32, y: f32) {
        let cards = self.selected_cards();
        if cards.len() < 2 {
            return;
        }
        let bottom = cards[0].clone();
        let (fraction_x, fraction_y) = self.screen_to_canonical_clamped(x, y, self.card_half_extents(&bottom));
        self.perform(TableAction::MoveCard { instance_id: bottom.instance_id.clone(), x: fraction_x, y: fraction_y });
        for card in &cards[1..] {
            self.perform(TableAction::StackCard {
                instance_id: card.instance_id.clone(),
                onto_instance_id: bottom.instance_id.clone(),
            });
        }
        let top_id = cards[cards.len() - 1].instance_id.clone();
        for instance_id in self.selected_token_ids() {
            self.perform(TableAction::AttachWidgetToCard {
                instance_id,
                card_id: top_id.clone(),
                x: fraction_x,
                y: fraction_y,
            });
        }
    }

    // -----------------------------------------------------------------------
    // Drops
    // -----------------------------------------------------------------------

    /// `T:handleCardDrop`: (x, y) is the dropped card's screen centre; `group` is what was latched
    /// at drag start.
    fn handle_card_drop(
        &mut self,
        frame: &mut Frame,
        instance_id: &str,
        x: f32,
        y: f32,
        group: Option<Vec<PickupItem>>,
    ) {
        self.perform(TableAction::EndCardDragPreview);
        let origin = self.derived.cards_by_id.get(instance_id).cloned();
        let alt = frame.keyboard.alt();
        let dragged = self.derived.pickup_candidates.iter().find(|item| item.instance_id() == instance_id).cloned();
        if let Some(dragged) = dragged {
            let group = group.unwrap_or_else(|| if alt { self.pickup_group(&dragged) } else { vec![dragged.clone()] });
            if group.len() > 1 {
                if self.is_over_table(x, y) {
                    self.move_mixed_group_by(frame, &group, dragged.position(), (x, y));
                }
                return;
            }
        }
        // Zones dock on top of the hand band, so a drop on a zone's own box goes to the zone.
        if let Some(zone_id) = self.docked_zone_id_at(x, y) {
            self.perform(TableAction::ReturnToZone { instance_id: instance_id.to_string(), zone_id, to_bottom: alt });
            return;
        }
        if self.is_over_local_hand(x, y) {
            let in_hand = self.derived.local_hand.iter().any(|card| card.instance_id == instance_id);
            let others: Vec<String> = self
                .derived
                .local_hand
                .iter()
                .filter(|card| card.instance_id != instance_id)
                .map(|card| card.instance_id.clone())
                .collect();
            let target_index = self.hand_drop_index(&others, x);
            // A card from elsewhere joins the hand at the end, then moves to its slot.
            if !in_hand {
                self.perform(TableAction::MoveToHand { instance_id: instance_id.to_string() });
            }
            self.perform(TableAction::ReorderHand { instance_id: instance_id.to_string(), target_index });
            return;
        }
        if !self.is_over_table(x, y) {
            return;
        }
        let (world_x, world_y) = self.screen_to_world(x, y);
        let target = self.find_stack_target(&HashSet::from([instance_id.to_string()]), world_x, world_y);
        if origin.as_ref().is_some_and(|origin| origin.zone == gametable_core::models::CardZone::Hand) {
            frame.sounds.play(SoundEffect::PlayCard);
        }
        match target {
            Some(target) if alt => {
                // A pile with a different owner refuses it; the card stays put.
                if self.backend.session().can_stack_onto(instance_id, &target.instance_id) {
                    self.perform(TableAction::StackCard {
                        instance_id: instance_id.to_string(),
                        onto_instance_id: target.instance_id,
                    });
                }
            }
            _ => {
                let half = origin
                    .as_ref()
                    .map_or((CARD_WIDTH / 2.0, CARD_HEIGHT / 2.0), |origin| self.card_half_extents(origin));
                let (fraction_x, fraction_y) = self.screen_to_canonical_clamped(x, y, half);
                self.perform(TableAction::MoveCard {
                    instance_id: instance_id.to_string(),
                    x: fraction_x,
                    y: fraction_y,
                });
            }
        }
    }

    /// The world position of the item a selection drag started from.
    pub(super) fn selection_anchor_world(&self, drag: &Drag) -> Option<(f32, f32)> {
        if let Some(token_id) = &drag.anchor_token_id {
            let widget = self.widget_by_id(token_id)?;
            return Some(self.canonical_to_world(widget.x, widget.y));
        }
        let top = drag.group.first()?;
        let (x, y) = self.derived.cards_by_id.get(top.instance_id()).map_or(top.position(), |card| (card.x, card.y));
        Some(self.canonical_to_world(x, y))
    }

    fn handle_selection_drop(&mut self, frame: &mut Frame, drag: &Drag, x: f32, y: f32) {
        if drag.primary_id.is_some() {
            self.perform(TableAction::EndCardDragPreview);
        }
        let cards = self.selected_cards();
        let alt = frame.keyboard.alt();
        if let Some(zone_id) = self.docked_zone_id_at(x, y) {
            self.send_cards_to_zone(&cards, &zone_id, alt);
            return;
        }
        if self.is_over_local_hand(x, y) {
            let others: Vec<String> = self.derived.local_hand.iter().map(|card| card.instance_id.clone()).collect();
            let index = self.hand_drop_index(&others, x);
            for (offset, card) in cards.iter().enumerate() {
                self.perform(TableAction::MoveToHand { instance_id: card.instance_id.clone() });
                self.perform(TableAction::ReorderHand {
                    instance_id: card.instance_id.clone(),
                    target_index: index + offset as i64,
                });
            }
            if !cards.is_empty() {
                frame.sounds.play(SoundEffect::Draw);
            }
            return;
        }
        if !self.is_over_table(x, y) {
            return;
        }
        let (world_x, world_y) = self.screen_to_world(x, y);
        let token_ids = self.selected_token_ids();
        if alt && let Some(target) = self.find_stack_target(&drag.group_ids, world_x, world_y) {
            // Piles never mix owners: if any card would be refused, nothing moves.
            let session = self.backend.session();
            if cards.iter().any(|card| !session.can_stack_onto(&card.instance_id, &target.instance_id)) {
                return;
            }
            for card in &cards {
                self.perform(TableAction::StackCard {
                    instance_id: card.instance_id.clone(),
                    onto_instance_id: target.instance_id.clone(),
                });
            }
            for instance_id in token_ids {
                self.perform(TableAction::AttachWidgetToCard {
                    instance_id,
                    card_id: target.instance_id.clone(),
                    x: target.x,
                    y: target.y,
                });
            }
            return;
        }
        if let Some((anchor_x, anchor_y)) = self.selection_anchor_world(drag) {
            let roots = self.selected_root_list();
            self.move_items_by(frame, &roots, &token_ids, (world_x - anchor_x, world_y - anchor_y));
        }
    }

    fn handle_pile_drop(&mut self, frame: &mut Frame, root_id: &str, cards: &[CardInstance], x: f32, y: f32) {
        self.perform(TableAction::EndCardDragPreview);
        if let Some(zone_id) = self.docked_zone_id_at(x, y) {
            self.send_cards_to_zone(cards, &zone_id, frame.keyboard.alt());
            return;
        }
        if self.is_over_local_hand(x, y) {
            let mut ordered = cards.to_vec();
            ordered.sort_by_key(|card| card.z_index);
            let others: Vec<String> = self.derived.local_hand.iter().map(|card| card.instance_id.clone()).collect();
            let index = self.hand_drop_index(&others, x);
            for (offset, card) in ordered.iter().enumerate() {
                self.perform(TableAction::MoveToHand { instance_id: card.instance_id.clone() });
                self.perform(TableAction::ReorderHand {
                    instance_id: card.instance_id.clone(),
                    target_index: index + offset as i64,
                });
            }
            return;
        }
        if !self.is_over_table(x, y) {
            return;
        }
        let half = top_card(cards).map_or((CARD_WIDTH / 2.0, CARD_HEIGHT / 2.0), |top| self.card_half_extents(top));
        let (fraction_x, fraction_y) = self.screen_to_canonical_clamped(x, y, half);
        self.perform(TableAction::MoveStack { root_instance_id: root_id.to_string(), x: fraction_x, y: fraction_y });
    }

    fn handle_widget_drop(&mut self, widget_id: &str, kind: BoardWidgetKind, x: f32, y: f32) {
        if !self.is_over_table(x, y) {
            return;
        }
        let (width, height) = self.widget_size(kind);
        let (fraction_x, fraction_y) = self.screen_to_canonical_clamped(x, y, (width / 2.0, height / 2.0));
        self.perform(TableAction::MoveWidget { instance_id: widget_id.to_string(), x: fraction_x, y: fraction_y });
    }

    /// A card token's own drop: with Alt a group lifted on it moves together; otherwise a plain move.
    fn handle_card_token_drop(&mut self, frame: &mut Frame, instance_id: &str, x: f32, y: f32, group: &[PickupItem]) {
        if group.len() > 1 {
            if !self.is_over_table(x, y) {
                return;
            }
            let Some(origin) = self.widget_by_id(instance_id).map(|widget| (widget.x, widget.y)) else { return };
            self.move_mixed_group_by(frame, group, origin, (x, y));
            return;
        }
        self.handle_widget_drop(instance_id, BoardWidgetKind::CardToken, x, y);
    }

    /// The bag widget docked in `zone_id` for whoever this drop counts as.
    fn bag_widget_for_drop(&self, zone_id: &str) -> Option<BoardWidgetInstance> {
        let zone = self.zone_by_id(zone_id)?;
        if zone.kind != ZoneKind::Widget || zone.widget_kind != ZoneWidgetKind::TokenBag {
            return None;
        }
        let owner_key = if zone.shared { SHARED_OWNER_KEY.to_string() } else { self.acting_player_id.clone() };
        self.derived.widget_for.get(&(owner_key, zone_id.to_string())).cloned()
    }

    fn handle_token_drop(&mut self, frame: &mut Frame, widget_id: &str, kind: BoardWidgetKind, x: f32, y: f32) {
        if kind == BoardWidgetKind::ImageToken
            && let Some(bag) = self.bag_zone_id_at(x, y).and_then(|zone_id| self.bag_widget_for_drop(&zone_id))
        {
            self.perform(TableAction::ReturnImageTokenToBag {
                bag_instance_id: bag.instance_id,
                token_instance_id: widget_id.to_string(),
            });
            return;
        }
        let ctrl = frame.keyboard.ctrl();
        let (world_x, world_y) = self.screen_to_world(x, y);
        let target = self.find_attach_target(&HashSet::from([widget_id.to_string()]), world_x, world_y);
        let (width, height) = self.widget_size(kind);
        let (fraction_x, fraction_y) = self.screen_to_canonical_clamped(x, y, (width / 2.0, height / 2.0));
        if ctrl {
            if !self.is_over_table(x, y) {
                return;
            }
            let new_instance_id = new_v4_uuid();
            self.perform(TableAction::DuplicateWidget {
                source_instance_id: widget_id.to_string(),
                new_instance_id: new_instance_id.clone(),
                x: fraction_x,
                y: fraction_y,
            });
            if let Some((card_id, _, _)) = target {
                self.perform(TableAction::AttachWidgetToCard {
                    instance_id: new_instance_id,
                    card_id,
                    x: fraction_x,
                    y: fraction_y,
                });
            }
            return;
        }
        match target {
            Some((card_id, _, _)) => self.perform(TableAction::AttachWidgetToCard {
                instance_id: widget_id.to_string(),
                card_id,
                x: fraction_x,
                y: fraction_y,
            }),
            None => self.handle_widget_drop(widget_id, kind, x, y),
        }
    }

    fn handle_library_drop(&mut self, definition_id: &str, x: f32, y: f32) {
        if !self.is_over_table(x, y) {
            return;
        }
        let turns = self.definition(definition_id).map_or(0.0, |definition| orientation_turns(definition.orientation));
        let half = half_extents(CARD_WIDTH, CARD_HEIGHT, turns);
        let (fraction_x, fraction_y) = self.screen_to_canonical_clamped(x, y, half);
        self.perform(TableAction::CreateCardFromLibrary {
            definition_id: definition_id.to_string(),
            x: fraction_x,
            y: fraction_y,
        });
    }

    fn handle_new_widget_drop(&mut self, kind: BoardWidgetKind, image_path: Option<String>, x: f32, y: f32) {
        if !self.is_over_table(x, y) {
            return;
        }
        let (width, height) = self.widget_size(kind);
        let (fraction_x, fraction_y) = self.screen_to_canonical_clamped(x, y, (width / 2.0, height / 2.0));
        self.perform(TableAction::CreateWidget {
            instance_id: new_v4_uuid(),
            kind,
            x: fraction_x,
            y: fraction_y,
            image_path,
        });
    }

    /// Dropped off the table, a revealed token stays put (a stray drag must never destroy one).
    fn handle_revealed_bag_token_drop(
        &mut self,
        frame: &mut Frame,
        bag_id: &str,
        slot_id: &str,
        image_path: &str,
        x: f32,
        y: f32,
    ) {
        if !self.is_over_table(x, y) {
            return;
        }
        let (width, height) = self.native_image_size(frame, Some(image_path));
        let (fraction_x, fraction_y) = self.screen_to_canonical_clamped(x, y, (width / 2.0, height / 2.0));
        self.perform(TableAction::PlaceBagTokenOnTable {
            instance_id: bag_id.to_string(),
            slot_id: slot_id.to_string(),
            new_instance_id: new_v4_uuid(),
            x: fraction_x,
            y: fraction_y,
        });
    }

    // -----------------------------------------------------------------------
    // Drag lifecycle
    // -----------------------------------------------------------------------

    pub(super) fn press_source(&mut self, source: DragSource, x: f32, y: f32) {
        if self.spectator {
            return;
        }
        self.pending = Some(PendingPress { source, press: (x, y) });
    }

    /// Sends the drag ghost's position to other players (throttled unless `force`).
    pub(super) fn send_card_preview(&mut self, frame: &Frame, force: bool) {
        let Some(drag) = &self.drag else { return };
        let Some(primary_instance_id) = drag.primary_id.clone() else { return };
        if !force && frame.time - self.last_preview_at < PREVIEW_THROTTLE {
            return;
        }
        self.last_preview_at = frame.time;
        let (mouse_x, mouse_y) = frame.ui.mouse();
        let (fraction_x, fraction_y) = self.screen_to_canonical(mouse_x, mouse_y, None);
        let passenger_root_ids = drag.group.iter().skip(1).map(|item| item.instance_id().to_string()).collect();
        self.perform(TableAction::PreviewCardDrag { primary_instance_id, passenger_root_ids, fraction_x, fraction_y });
    }

    fn start_drag(&mut self, frame: &Frame) {
        let Some(pending) = self.pending.take() else { return };
        let source = pending.source;
        let grab = (source.center.0 - pending.press.0, source.center.1 - pending.press.1);
        let mut drag = Drag {
            started: true,
            source: source.clone(),
            grab,
            grab_at_start: grab,
            scale: source.scale,
            scale_at_start: source.scale,
            primary_id: None,
            group: Vec::new(),
            group_ids: HashSet::new(),
            token_ids: HashSet::new(),
            widget_id: None,
            angle: 0.0,
            alt_down: self.alt_down,
            anchor_root_id: None,
            anchor_token_id: None,
            draw_order: Vec::new(),
        };
        match &source.kind {
            DragSourceKind::Selection { anchor_root_id, anchor_token_id } => {
                drag.anchor_root_id = anchor_root_id.clone();
                drag.anchor_token_id = anchor_token_id.clone();
                let mut tops: Vec<CardInstance> = Vec::new();
                for root_id in self.selected_root_list() {
                    let cards = &self.derived.pile_groups[&root_id];
                    if let Some(top) = top_card(cards) {
                        if Some(&root_id) == anchor_root_id.as_ref() {
                            tops.insert(0, top.clone());
                        } else {
                            tops.push(top.clone());
                        }
                    }
                    drag.group_ids.extend(cards.iter().map(|card| card.instance_id.clone()));
                }
                let mut draw_order = tops.clone();
                draw_order.sort_by_key(|card| card.z_index);
                drag.draw_order = draw_order;
                drag.primary_id = tops.first().map(|card| card.instance_id.clone());
                drag.group = tops.into_iter().map(PickupItem::Card).collect();
                drag.token_ids = self.selected_token_ids().into_iter().collect();
                let has_primary = drag.primary_id.is_some();
                self.drag = Some(drag);
                if has_primary {
                    self.send_card_preview(frame, true);
                }
            }
            DragSourceKind::Card(card_source) | DragSourceKind::Pile(card_source) => {
                drag.primary_id = Some(card_source.card.instance_id.clone());
                let item = PickupItem::Card(card_source.card.clone());
                drag.group =
                    if card_source.allow_group && self.alt_down { self.pickup_group(&item) } else { vec![item] };
                if drag.group.len() > 1 {
                    drag.group_ids = drag.group.iter().map(|item| item.instance_id().to_string()).collect();
                }
                drag.angle = card_source.angle;
                self.drag = Some(drag);
                self.send_card_preview(frame, true);
            }
            DragSourceKind::CardToken(widget) => {
                drag.widget_id = Some(widget.instance_id.clone());
                let item = PickupItem::CardToken(widget.clone());
                drag.group = if self.alt_down { self.pickup_group(&item) } else { vec![item] };
                if drag.group.len() > 1 {
                    drag.group_ids = drag.group.iter().map(|item| item.instance_id().to_string()).collect();
                }
                self.drag = Some(drag);
            }
            DragSourceKind::Widget(widget) | DragSourceKind::Token(widget) => {
                drag.widget_id = Some(widget.instance_id.clone());
                self.drag = Some(drag);
            }
            DragSourceKind::RevealedBagToken { widget, .. } => {
                drag.widget_id = Some(widget.instance_id.clone());
                self.drag = Some(drag);
            }
            _ => self.drag = Some(drag),
        }
    }

    /// A single dragged card eases to the size it would have where it's held (hand, table or
    /// base size); the grab offset scales with it.
    pub(super) fn update_drag_scale(&mut self, frame: &mut Frame) {
        let Some(drag) = &self.drag else { return };
        if !matches!(drag.source.kind, DragSourceKind::Card(_) | DragSourceKind::Pile(_)) || drag.group.len() > 1 {
            return;
        }
        let (mouse_x, mouse_y) = frame.ui.mouse();
        let (x, y) = (mouse_x + drag.grab.0, mouse_y + drag.grab.1);
        let target = if self.docked_zone_id_at(x, y).is_some() {
            1.0
        } else if self.is_over_local_hand(x, y) {
            TableScreen::hand_parameters(frame).scale
        } else if self.is_over_table(x, y) {
            self.zoom
        } else {
            1.0
        };
        let easing = 1.0 - (-18.0 * self.last_dt).exp();
        let Some(drag) = &mut self.drag else { return };
        drag.scale += (target - drag.scale) * easing;
        let factor = drag.scale / drag.scale_at_start;
        drag.grab = (drag.grab_at_start.0 * factor, drag.grab_at_start.1 * factor);
    }

    fn end_drag(&mut self, frame: &mut Frame) {
        let Some(drag) = self.drag.take() else { return };
        let (mouse_x, mouse_y) = frame.ui.mouse();
        let (x, y) = (mouse_x + drag.grab.0, mouse_y + drag.grab.1);
        let primary = drag.primary_id.clone().unwrap_or_default();
        match &drag.source.kind {
            DragSourceKind::Card(_) => self.handle_card_drop(frame, &primary, x, y, Some(drag.group.clone())),
            DragSourceKind::Pile(source) => {
                if drag.alt_down {
                    let root_id = source.root_id.clone().unwrap_or_default();
                    self.handle_pile_drop(frame, &root_id, &source.cards, x, y);
                } else {
                    self.handle_card_drop(frame, &primary, x, y, Some(drag.group.clone()));
                }
            }
            DragSourceKind::Widget(widget) => self.handle_widget_drop(&widget.instance_id, widget.kind, x, y),
            DragSourceKind::Token(widget) => self.handle_token_drop(frame, &widget.instance_id, widget.kind, x, y),
            DragSourceKind::CardToken(widget) => {
                self.handle_card_token_drop(frame, &widget.instance_id, x, y, &drag.group)
            }
            DragSourceKind::Library { definition_id } => self.handle_library_drop(definition_id, x, y),
            DragSourceKind::ImageToken { image_file } => {
                self.handle_new_widget_drop(BoardWidgetKind::ImageToken, Some(image_file.clone()), x, y)
            }
            DragSourceKind::NewToken => self.handle_new_widget_drop(BoardWidgetKind::Token, None, x, y),
            DragSourceKind::NewCardToken => self.handle_new_widget_drop(BoardWidgetKind::CardToken, None, x, y),
            DragSourceKind::RevealedBagToken { widget, slot_id, image_path } => {
                self.handle_revealed_bag_token_drop(frame, &widget.instance_id, slot_id, image_path, x, y)
            }
            DragSourceKind::Selection { .. } => self.handle_selection_drop(frame, &drag, x, y),
        }
    }

    /// Right-click during a drag cancels it (nothing was committed yet).
    fn cancel_drag(&mut self) {
        if let Some(drag) = self.drag.take()
            && drag.primary_id.is_some()
        {
            self.perform(TableAction::EndCardDragPreview);
        }
    }

    // -----------------------------------------------------------------------
    // Presses
    // -----------------------------------------------------------------------

    fn top_item_at(&self, x: f32, y: f32) -> Option<HitItem> {
        self.previous_items.iter().rev().find(|item| item.hit(x, y)).cloned()
    }

    /// `T:sourceFor(item)`: the drag source for a press on an item.
    fn source_for(&self, item: &HitItem) -> Option<DragSource> {
        match &item.kind {
            ItemKind::TableCard { card, root_id, owned_by_opponent }
            | ItemKind::Pile { card, root_id, owned_by_opponent, .. } => {
                if !item.interactable {
                    return None;
                }
                let cards = match &item.kind {
                    ItemKind::Pile { cards, .. } => cards.clone(),
                    _ => Vec::new(),
                };
                let face_away = self.card_faces_away_from_me(card.owner_id.as_deref());
                let source = CardSource {
                    card: card.clone(),
                    root_id: Some(root_id.clone()),
                    cards,
                    allow_group: !owned_by_opponent,
                    angle: self.card_turns(card, face_away) * 2.0 * std::f32::consts::PI,
                    border: self.owner_border_color(card.owner_id.as_deref()),
                    force_face_up: false,
                };
                let kind = if matches!(item.kind, ItemKind::Pile { .. }) {
                    DragSourceKind::Pile(source)
                } else {
                    DragSourceKind::Card(source)
                };
                Some(DragSource::new(kind, self.to_screen(card.x, card.y), self.zoom))
            }
            ItemKind::HandCard { card, scale, card_center } => {
                let center = match item.mirror {
                    Some((mirror_x, mirror_y)) => (2.0 * mirror_x - card_center.0, 2.0 * mirror_y - card_center.1),
                    None => *card_center,
                };
                let source = CardSource {
                    border: self.owner_border_color(card.owner_id.as_deref()),
                    ..CardSource::plain(card.clone())
                };
                Some(DragSource::new(DragSourceKind::Card(source), center, *scale))
            }
            ItemKind::Zone { top: Some(top), .. } if item.interactable => {
                let shape = &item.shape;
                let center = (shape.x + shape.width / 2.0, shape.y + shape.height / 2.0);
                Some(DragSource::new(DragSourceKind::Card(CardSource::plain(top.clone())), center, 1.0))
            }
            ItemKind::Counter { widget } | ItemKind::PackGenerator { widget } if item.interactable => {
                let shape = &item.shape;
                let center = (shape.x + shape.width / 2.0, shape.y + shape.height / 2.0);
                Some(DragSource::new(DragSourceKind::Widget(widget.clone()), center, self.zoom))
            }
            ItemKind::Token { widget } if item.interactable => {
                // A card token's own drag never attaches it or lands it in a hand or zone.
                let kind = if widget.kind == BoardWidgetKind::CardToken {
                    DragSourceKind::CardToken(widget.clone())
                } else {
                    DragSourceKind::Token(widget.clone())
                };
                Some(DragSource::new(kind, (item.shape.center_x, item.shape.center_y), self.zoom))
            }
            ItemKind::DeckHeader { widget } => Some(DragSource::new(
                DragSourceKind::Widget(widget.clone()),
                self.to_screen(widget.x, widget.y),
                self.zoom,
            )),
            ItemKind::RevealedBagToken { widget, slot_id, image_path } if item.interactable => {
                let kind = DragSourceKind::RevealedBagToken {
                    widget: widget.clone(),
                    slot_id: slot_id.clone(),
                    image_path: image_path.clone(),
                };
                Some(DragSource::new(kind, (item.shape.center_x, item.shape.center_y), self.zoom))
            }
            _ => None,
        }
    }

    /// `T:applySelectionToPress`: Ctrl+click toggles, a selected item drags the selection, an
    /// unselected one clears it first.
    fn apply_selection_to_press(&mut self, frame: &Frame, item: &HitItem, mut source: DragSource) -> DragSource {
        let (is_token, selectable, selected) = match &item.kind {
            ItemKind::Token { widget } => {
                (true, self.is_selectable_token(Some(widget)), self.is_token_selected(&widget.instance_id))
            }
            ItemKind::TableCard { card, root_id, .. } | ItemKind::Pile { card, root_id, .. } => (
                false,
                card.owner_id.as_deref().is_none_or(|owner| owner == self.acting_player_id),
                self.selected_roots.contains(root_id),
            ),
            _ => return source,
        };
        if selectable && frame.keyboard.ctrl() {
            source.on_click = Some(ClickAction::ToggleSelected(Box::new(item.clone())));
            return source;
        }
        if selected && self.selected_count >= 2 {
            let (anchor_root_id, anchor_token_id) = match &item.kind {
                ItemKind::Token { widget } => (None, Some(widget.instance_id.clone())),
                ItemKind::TableCard { root_id, .. } | ItemKind::Pile { root_id, .. } => (Some(root_id.clone()), None),
                _ => (None, None),
            };
            let _ = is_token;
            return DragSource {
                kind: DragSourceKind::Selection { anchor_root_id, anchor_token_id },
                center: source.center,
                scale: self.zoom,
                on_click: source.on_click,
            };
        }
        if !selected && self.selected_count > 0 {
            self.clear_selection();
        }
        source
    }

    fn run_click(&mut self, frame: &mut Frame, action: ClickAction) {
        match action {
            ClickAction::DrawCard { pile_instance_id } => {
                self.perform(TableAction::DrawCard { pile_instance_id, count: 1 })
            }
            ClickAction::ReturnBagToken { instance_id, slot_id } => {
                self.perform(TableAction::ReturnBagToken { instance_id, slot_id })
            }
            ClickAction::ToggleSelected(item) => self.toggle_selected(&item),
        }
        let _ = frame;
    }

    fn send_arrow_preview(&mut self) {
        let Some(arrow) = self.arrow_drag else { return };
        let (fraction_x, fraction_y) = local_pixel_to_canonical(
            f64::from(arrow.start.0),
            f64::from(arrow.start.1),
            WORLD_WIDTH,
            WORLD_HEIGHT,
            self.is_mirrored,
        );
        let (fraction_x2, fraction_y2) = local_pixel_to_canonical(
            f64::from(arrow.current.0),
            f64::from(arrow.current.1),
            WORLD_WIDTH,
            WORLD_HEIGHT,
            self.is_mirrored,
        );
        self.perform(TableAction::PreviewArrowDrag { fraction_x, fraction_y, fraction_x2, fraction_y2 });
    }

    /// `T:processInput()`.
    pub(super) fn process_input(&mut self, frame: &mut Frame) {
        let (mouse_x, mouse_y) = frame.ui.mouse();
        let free = frame.ui.base_has_mouse();
        let item = if free { self.top_item_at(mouse_x, mouse_y) } else { None };

        // Hover drives the Space preview and the hotkeys.
        let dragging = self.drag.as_ref().is_some_and(|drag| drag.started);
        if !dragging {
            self.hover_item = item.clone();
            if let Some(overlay_id) = self.overlay_hover_id.clone() {
                self.hovered_id = Some(overlay_id);
                self.hovered_force_face_up = true;
            } else {
                self.hovered_id = item.as_ref().and_then(|item| item.hover_id.clone());
                self.hovered_force_face_up = false;
            }
        }
        self.overlay_hover_id = None;
        if let Some(item) = &item
            && let Some(tooltip) = &item.tooltip
            && self.drag.is_none()
        {
            let key = match &item.kind {
                ItemKind::Zone { zone_id, .. } => zone_id.clone(),
                _ => tooltip.clone(),
            };
            frame.ui.tooltip(&format!("table:{}:{key}", item.kind.name()), tooltip);
        }
        if let Some(item) = &item {
            let clickable = is_draggable(&item.kind)
                || matches!(
                    item.kind,
                    ItemKind::Shuffle { .. }
                        | ItemKind::Avatar { .. }
                        | ItemKind::DeckSave { .. }
                        | ItemKind::TokenBagIdle { .. }
                );
            let empty_zone = matches!(item.kind, ItemKind::Zone { top: None, .. });
            if clickable && !empty_zone && item.interactable {
                frame.ui.set_cursor("hand");
            }
        }

        // Wheel zoom over the table, around the pointer.
        let (_, wheel_y) = frame.ui.wheel();
        if free
            && wheel_y != 0.0
            && self.is_over_table(mouse_x, mouse_y)
            && !self.library_open
            && !self.token_picker_open
        {
            let old = self.zoom;
            let new = (old * (wheel_y * 0.1).exp()).clamp(MIN_ZOOM, MAX_ZOOM);
            if new != old {
                let rect = self.table_rect();
                let (anchor_x, anchor_y) =
                    (mouse_x - (rect.x + rect.width / 2.0), mouse_y - (rect.y + rect.height / 2.0));
                self.zoom = new;
                self.camera_x += anchor_x * (1.0 / new - 1.0 / old);
                self.camera_y += anchor_y * (1.0 / new - 1.0 / old);
            }
            frame.ui.consume_wheel();
        }

        // Middle-button drag pans the table.
        if let Some(pan) = self.middle_pan {
            if frame.ui.mouse_down(3) {
                let (max_x, max_y) = self.max_pan();
                self.camera_x = (pan.camera.0 + (mouse_x - pan.press.0) / self.zoom).clamp(-max_x, max_x);
                self.camera_y = (pan.camera.1 + (mouse_y - pan.press.1) / self.zoom).clamp(-max_y, max_y);
                frame.ui.set_cursor("sizeall");
            } else {
                self.middle_pan = None;
            }
        } else if free
            && frame.ui.mouse_pressed(3)
            && self.is_over_table(mouse_x, mouse_y)
            && !self.library_open
            && !self.token_picker_open
        {
            self.middle_pan = Some(MiddlePan { press: (mouse_x, mouse_y), camera: self.clamped_camera() });
        }

        // Arrow drawing (Tab + drag on the table).
        if let Some(mut arrow) = self.arrow_drag {
            arrow.current = self.screen_to_world(mouse_x, mouse_y);
            self.arrow_drag = Some(arrow);
            if frame.time - self.last_preview_at >= PREVIEW_THROTTLE {
                self.last_preview_at = frame.time;
                self.send_arrow_preview();
            }
            if frame.ui.mouse_released(1) {
                self.perform(TableAction::EndArrowDragPreview);
                self.arrow_drag = None;
                let (dx, dy) = (arrow.current.0 - arrow.start.0, arrow.current.1 - arrow.start.1);
                if (dx * dx + dy * dy).sqrt() >= 4.0 {
                    let to_canonical = |point: (f32, f32)| {
                        local_pixel_to_canonical(
                            f64::from(point.0),
                            f64::from(point.1),
                            WORLD_WIDTH,
                            WORLD_HEIGHT,
                            self.is_mirrored,
                        )
                    };
                    let (x, y) = to_canonical(arrow.start);
                    let (x2, y2) = to_canonical(arrow.current);
                    self.perform(TableAction::CreateArrow { instance_id: new_v4_uuid(), x, y, x2, y2 });
                }
            }
            return;
        }

        // A selection box being dragged out.
        if let Some(mut selection) = self.box_select {
            selection.current = Some(self.screen_to_world(mouse_x, mouse_y));
            self.box_select = Some(selection);
            if frame.ui.mouse_released(1) || !frame.ui.mouse_down(1) {
                self.box_select = None;
                self.finish_box_select(selection);
            }
            return;
        }

        // An active drag.
        if self.drag.is_some() {
            if frame.ui.mouse_pressed(2) {
                // Right click cancels instead of opening a menu.
                self.cancel_drag();
                return;
            }
            if self.drag.as_ref().is_some_and(|drag| drag.primary_id.is_some()) {
                self.send_card_preview(frame, false);
            }
            if frame.ui.mouse_released(1) {
                self.end_drag(frame);
            }
            return;
        }
        if let Some(pending) = self.pending.clone() {
            if frame.ui.mouse_down(1) && !frame.ui.mouse_released(1) {
                let moved_x = (mouse_x - pending.press.0).abs() > DRAG_THRESHOLD;
                let moved_y = (mouse_y - pending.press.1).abs() > DRAG_THRESHOLD;
                if moved_x || moved_y {
                    self.start_drag(frame);
                }
                return;
            }
            // Released without dragging: a click. A new press this same frame is still handled.
            self.pending = None;
            if let Some(action) = pending.source.on_click {
                self.run_click(frame, action);
            }
            if !frame.ui.mouse_pressed(1) {
                return;
            }
        }

        let Some(item) = item.filter(|_| free) else { return };

        if frame.ui.mouse_pressed(1) {
            if self.tab_down && is_table_kind(&item.kind) && self.is_over_table(mouse_x, mouse_y) {
                let start = self.screen_to_world(mouse_x, mouse_y);
                self.arrow_drag = Some(ArrowDrag { start, current: start });
                self.last_preview_at = 0.0;
                self.send_arrow_preview();
                return;
            }
            match &item.kind {
                ItemKind::Table if !self.spectator => {
                    // Empty felt: drag out a selection box (Shift adds to the selection).
                    let start = self.screen_to_world(mouse_x, mouse_y);
                    self.box_select = Some(BoxSelect { start, current: None, additive: frame.keyboard.shift() });
                    return;
                }
                ItemKind::Shuffle { .. }
                | ItemKind::DeckSave { .. }
                | ItemKind::Avatar { .. }
                | ItemKind::TokenBagIdle { .. } => {
                    self.click_item = Some(item.clone());
                }
                ItemKind::Counter { widget } | ItemKind::DockedCounter { widget }
                    if frame.ui.click_count(1) >= 2 && item.interactable =>
                {
                    let shape = &item.shape;
                    let right_side = match item.mirror {
                        Some((mirror_x, _)) if matches!(item.kind, ItemKind::DockedCounter { .. }) => {
                            2.0 * mirror_x - mouse_x >= shape.x + shape.width / 2.0
                        }
                        _ => mouse_x >= shape.x + shape.width / 2.0,
                    };
                    let value = widget.value + if right_side { 1.0 } else { -1.0 };
                    self.perform(TableAction::SetWidgetValue { instance_id: widget.instance_id.clone(), value });
                }
                kind if is_draggable(kind) => {
                    if let Some(mut source) = self.source_for(&item) {
                        match &item.kind {
                            ItemKind::Pile { root_id, .. } => {
                                source.on_click = Some(ClickAction::DrawCard { pile_instance_id: root_id.clone() });
                            }
                            ItemKind::RevealedBagToken { widget, slot_id, .. } => {
                                source.on_click = Some(ClickAction::ReturnBagToken {
                                    instance_id: widget.instance_id.clone(),
                                    slot_id: slot_id.clone(),
                                });
                            }
                            _ => {}
                        }
                        let source = self.apply_selection_to_press(frame, &item, source);
                        self.press_source(source, mouse_x, mouse_y);
                    }
                }
                _ => {}
            }
        }
        if frame.ui.mouse_released(1)
            && let Some(clicked) = self.click_item.take()
            && (clicked == item || (clicked.kind.name() == item.kind.name() && clicked.hit(mouse_x, mouse_y)))
        {
            match &clicked.kind {
                ItemKind::Shuffle { key, target } => match target {
                    ShuffleTarget::Pile { root_id } => self.shuffle_pile(frame, root_id),
                    ShuffleTarget::Zone { zone_id } => self.shuffle_zone(frame, zone_id, Some(key.clone())),
                },
                ItemKind::DeckSave { widget } => self.save_deck_widget(frame, widget),
                ItemKind::Avatar { player_id } => {
                    let player_id = player_id.clone();
                    self.backend.session_mut().set_active_seat(Some(&player_id));
                }
                ItemKind::TokenBagIdle { widget } => {
                    self.perform(TableAction::DrawBagToken { instance_id: widget.instance_id.clone() })
                }
                _ => {}
            }
        }
        if frame.ui.mouse_pressed(2) {
            self.right_press_item = Some(item.clone());
        }
        if frame.ui.mouse_released(2)
            && let Some(pressed) = self.right_press_item.take()
            && !self.open_item_menu(frame, &pressed, mouse_x, mouse_y)
            && self.is_over_table(mouse_x, mouse_y)
            && is_table_kind(&pressed.kind)
        {
            self.open_board_menu(frame, mouse_x, mouse_y);
        }
    }

    /// Mouse released after dragging out a box from empty table.
    fn finish_box_select(&mut self, selection: BoxSelect) {
        if !selection.additive {
            self.clear_selection();
        }
        let (x1, y1) = self.world_to_screen(selection.start.0, selection.start.1);
        let current = selection.current.unwrap_or(selection.start);
        let (x2, y2) = self.world_to_screen(current.0, current.1);
        if (x2 - x1).abs() <= DRAG_THRESHOLD && (y2 - y1).abs() <= DRAG_THRESHOLD {
            return;
        }
        let rect = rect_from_points(x1, y1, x2, y2);
        let acting = self.acting_player_id.clone();
        let items = self.previous_items.clone();
        for item in &items {
            match &item.kind {
                ItemKind::TableCard { card, root_id, .. } | ItemKind::Pile { card, root_id, .. } => {
                    if card.owner_id.as_deref().is_none_or(|owner| owner == acting) && box_hits(&rect, &item.shape) {
                        for card in self.derived.pile_groups.get(root_id).into_iter().flatten() {
                            self.selection.cards.insert(card.instance_id.clone());
                        }
                    }
                }
                ItemKind::Token { widget }
                    if self.is_selectable_token(Some(widget)) && box_hits(&rect, &item.shape) =>
                {
                    self.selection.tokens.insert(widget.instance_id.clone());
                }
                _ => {}
            }
        }
        self.refresh_selection();
    }
}
