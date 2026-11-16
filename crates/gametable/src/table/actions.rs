//! What the table asks for. Port of `src/game/controller.lua`.
//!
//! The table UI never changes the session itself: every player action becomes a [`TableAction`]
//! handed to its [`TableBackend`]. The host/practice backend applies it to the session directly
//! (with the original Host controller's ownership checks, [`apply_as_host`]); the network client
//! (Phase 7) turns the same actions into request messages.

use gametable_core::deck_widget_zones::parse_deck_widget_zone_id;
use gametable_core::models::{BoardWidgetKind, DeckConfig, WidgetSizeOverride};
use gametable_core::session::Session;
use gametable_core::stack_utils::stack_of;

/// One request from the table, named after the original controller methods.
#[derive(Debug, Clone, PartialEq)]
pub enum TableAction {
    MoveCard {
        instance_id: String,
        x: f64,
        y: f64,
    },
    MoveGroup {
        primary_instance_id: String,
        passenger_root_ids: Vec<String>,
        x: f64,
        y: f64,
    },
    MoveStack {
        root_instance_id: String,
        x: f64,
        y: f64,
    },
    RotateStack {
        root_instance_id: String,
        clockwise: bool,
        step: f64,
    },
    RotateWidget {
        instance_id: String,
        clockwise: bool,
        step: f64,
    },
    FlipWidget {
        instance_id: String,
    },
    SendWidgetToBack {
        instance_id: String,
    },
    SendToBack {
        root_instance_id: String,
    },
    FlipCard {
        instance_id: String,
    },
    GiveCard {
        instance_id: String,
        new_owner_id: Option<String>,
    },
    StackCard {
        instance_id: String,
        onto_instance_id: String,
    },
    MoveToHand {
        instance_id: String,
    },
    ReorderHand {
        instance_id: String,
        target_index: i64,
    },
    DrawCard {
        pile_instance_id: String,
        count: i64,
    },
    ShufflePile {
        root_instance_id: String,
    },
    DrawFromZone {
        zone_id: String,
        count: i64,
    },
    ReturnToZone {
        instance_id: String,
        zone_id: String,
        to_bottom: bool,
    },
    ShuffleZone {
        zone_id: String,
    },
    LoadDeckIntoZone {
        zone_id: String,
        deck: DeckConfig,
    },
    LoadScenario {
        scenario_id: String,
    },
    SetSharedPlaymat {
        on: bool,
        index: i64,
    },
    SetCurrentPlayer {
        player_id: String,
    },
    SetIndividualPlaymat {
        on: bool,
        index: i64,
    },
    StartSearchZone {
        zone_id: String,
    },
    StartSearchPile {
        root_instance_id: String,
    },
    StopSearch,
    CreateWidget {
        instance_id: String,
        kind: BoardWidgetKind,
        x: f64,
        y: f64,
        image_path: Option<String>,
    },
    CreateArrow {
        instance_id: String,
        x: f64,
        y: f64,
        x2: f64,
        y2: f64,
    },
    MoveWidget {
        instance_id: String,
        x: f64,
        y: f64,
    },
    SetWidgetValue {
        instance_id: String,
        value: f64,
    },
    DeleteWidget {
        instance_id: String,
    },
    SetWidgetColors {
        instance_id: String,
        background_color: u32,
        text_color: u32,
    },
    SetWidgetText {
        instance_id: String,
        text: String,
        text_color: u32,
    },
    SetWidgetSize {
        instance_id: String,
        size_override: Option<WidgetSizeOverride>,
    },
    DuplicateWidget {
        source_instance_id: String,
        new_instance_id: String,
        x: f64,
        y: f64,
    },
    AttachWidgetToCard {
        instance_id: String,
        card_id: String,
        x: f64,
        y: f64,
    },
    DrawBagToken {
        instance_id: String,
    },
    ReturnBagToken {
        instance_id: String,
        slot_id: String,
    },
    PlaceBagTokenOnTable {
        instance_id: String,
        slot_id: String,
        new_instance_id: String,
        x: f64,
        y: f64,
    },
    ResetBag {
        instance_id: String,
    },
    SetBagRemaining {
        instance_id: String,
        bag_remaining: Vec<String>,
    },
    ReturnImageTokenToBag {
        bag_instance_id: String,
        token_instance_id: String,
    },
    GeneratePack {
        set_id: String,
    },
    CreateCardFromLibrary {
        definition_id: String,
        x: f64,
        y: f64,
    },
    /// Cosmetic: other players see a ghost of the cards being dragged.
    PreviewCardDrag {
        primary_instance_id: String,
        passenger_root_ids: Vec<String>,
        fraction_x: f64,
        fraction_y: f64,
    },
    EndCardDragPreview,
    PreviewArrowDrag {
        fraction_x: f64,
        fraction_y: f64,
        fraction_x2: f64,
        fraction_y2: f64,
    },
    EndArrowDragPreview,
}

/// Where the table's session lives and where its actions go.
pub trait TableBackend {
    fn session(&self) -> &Session;
    /// For local-only changes (timers, the practice seat being acted as).
    fn session_mut(&mut self) -> &mut Session;
    fn perform(&mut self, action: TableAction);
    /// Called once per frame before drawing (network backends pump messages here).
    fn update(&mut self) {
        self.session_mut().update();
    }
}

/// A practice game: the session lives here and actions apply straight to it.
pub struct PracticeBackend {
    pub session: Session,
}

impl TableBackend for PracticeBackend {
    fn session(&self) -> &Session {
        &self.session
    }

    fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    fn perform(&mut self, action: TableAction) {
        apply_as_host(&mut self.session, action);
    }
}

/// `Host:isOwnedOrUnowned(instanceId)`.
fn is_owned_or_unowned(session: &Session, instance_id: &str) -> bool {
    let Some(card) = session.find_card(instance_id) else { return false };
    card.owner_id.as_deref().is_none_or(|owner| owner == session.acting_player_id())
}

/// `Host:isOwnedOrUnownedStack(rootId)`.
fn is_owned_or_unowned_stack(session: &Session, root_instance_id: &str) -> bool {
    let acting = session.acting_player_id();
    stack_of(session.state().cards(), root_instance_id)
        .iter()
        .all(|card| card.owner_id.as_deref().is_none_or(|owner| owner == acting))
}

/// `Host:zoneOwnerId(zoneId)`: a deck widget's owner for its sub-zones, nobody for shared zones,
/// otherwise the acting player.
fn zone_owner_id(session: &Session, zone_id: &str) -> Option<String> {
    if let Some(parsed) = parse_deck_widget_zone_id(zone_id) {
        return session.find_widget(parsed.widget_instance_id).and_then(|widget| widget.owner_id.clone());
    }
    let shared = session.effective_zones().find(|zone| zone.id == zone_id).is_some_and(|zone| zone.shared);
    if shared { None } else { Some(session.acting_player_id().to_string()) }
}

/// The Host controller: applies an action for the acting player. Errors the original raised as
/// Lua errors (an unknown zone) leave the session as the failed call left it.
pub fn apply_as_host(session: &mut Session, action: TableAction) {
    let acting = session.acting_player_id().to_string();
    let acting = acting.as_str();
    match action {
        TableAction::MoveCard { instance_id, x, y } => {
            if is_owned_or_unowned(session, &instance_id) {
                let _ = session.move_card(&instance_id, x, y, acting);
            }
        }
        TableAction::MoveGroup { primary_instance_id, passenger_root_ids, x, y } => {
            if is_owned_or_unowned(session, &primary_instance_id) {
                let _ = session.move_group(&primary_instance_id, &passenger_root_ids, x, y, acting);
            }
        }
        TableAction::MoveStack { root_instance_id, x, y } => {
            if is_owned_or_unowned(session, &root_instance_id) {
                session.move_stack(&root_instance_id, x, y);
            }
        }
        TableAction::RotateStack { root_instance_id, clockwise, step } => {
            if is_owned_or_unowned(session, &root_instance_id) {
                let _ = session.rotate_stack(&root_instance_id, clockwise, acting, step);
            }
        }
        TableAction::RotateWidget { instance_id, clockwise, step } => {
            session.rotate_widget(&instance_id, clockwise, step)
        }
        TableAction::FlipWidget { instance_id } => session.flip_widget(&instance_id),
        TableAction::SendWidgetToBack { instance_id } => session.send_widget_to_back(&instance_id),
        TableAction::SendToBack { root_instance_id } => {
            if is_owned_or_unowned(session, &root_instance_id) {
                session.send_to_back(&root_instance_id);
            }
        }
        TableAction::FlipCard { instance_id } => {
            if is_owned_or_unowned(session, &instance_id) {
                let _ = session.flip_card(&instance_id, acting);
            }
        }
        TableAction::GiveCard { instance_id, new_owner_id } => {
            if is_owned_or_unowned(session, &instance_id) {
                let _ = session.give_card(&instance_id, new_owner_id.as_deref(), acting);
            }
        }
        TableAction::StackCard { instance_id, onto_instance_id } => {
            if is_owned_or_unowned(session, &instance_id) {
                let _ = session.stack_card(&instance_id, &onto_instance_id, acting);
            }
        }
        TableAction::MoveToHand { instance_id } => {
            let _ = session.move_to_hand(&instance_id, Some(acting));
        }
        TableAction::ReorderHand { instance_id, target_index } => {
            session.reorder_hand(&instance_id, target_index, Some(acting))
        }
        TableAction::DrawCard { pile_instance_id, count } => session.draw_card(&pile_instance_id, Some(acting), count),
        TableAction::ShufflePile { root_instance_id } => session.shuffle_pile(&root_instance_id, acting),
        TableAction::DrawFromZone { zone_id, count } => {
            let owner = zone_owner_id(session, &zone_id);
            let _ = session.draw_from_zone(&zone_id, owner.as_deref(), Some(acting), count);
        }
        TableAction::ReturnToZone { instance_id, zone_id, to_bottom } => {
            let owner = zone_owner_id(session, &zone_id);
            let _ = session.return_to_zone(&instance_id, &zone_id, owner.as_deref(), to_bottom, acting);
        }
        TableAction::ShuffleZone { zone_id } => {
            let owner = zone_owner_id(session, &zone_id);
            let _ = session.shuffle_zone(&zone_id, owner.as_deref(), acting);
        }
        TableAction::LoadDeckIntoZone { zone_id, deck } => {
            let _ = session.load_deck_into_zone(&zone_id, &deck, acting);
        }
        TableAction::LoadScenario { scenario_id } => session.load_scenario(&scenario_id, acting),
        TableAction::SetSharedPlaymat { on, index } => session.set_shared_playmat(on, index),
        TableAction::SetCurrentPlayer { player_id } => session.set_current_player(Some(&player_id)),
        TableAction::SetIndividualPlaymat { on, index } => session.set_individual_playmat(acting, on, index),
        TableAction::StartSearchZone { zone_id } => {
            let owner = zone_owner_id(session, &zone_id);
            let _ = session.start_search_zone(&zone_id, owner.as_deref(), Some(acting));
        }
        TableAction::StartSearchPile { root_instance_id } => {
            if is_owned_or_unowned_stack(session, &root_instance_id) {
                session.start_search_pile(&root_instance_id, Some(acting));
            }
        }
        TableAction::StopSearch => session.stop_search(Some(acting)),
        TableAction::CreateWidget { instance_id, kind, x, y, image_path } => {
            session.create_widget(&instance_id, kind, x, y, acting, image_path.as_deref())
        }
        TableAction::CreateArrow { instance_id, x, y, x2, y2 } => {
            session.create_arrow(&instance_id, x, y, x2, y2, acting)
        }
        TableAction::MoveWidget { instance_id, x, y } => session.move_widget(&instance_id, x, y),
        TableAction::SetWidgetValue { instance_id, value } => {
            let _ = session.set_widget_value(&instance_id, value, acting);
        }
        TableAction::DeleteWidget { instance_id } => session.delete_widget(&instance_id),
        TableAction::SetWidgetColors { instance_id, background_color, text_color } => {
            session.set_widget_colors(&instance_id, background_color, text_color)
        }
        TableAction::SetWidgetText { instance_id, text, text_color } => {
            session.set_widget_text(&instance_id, &text, text_color)
        }
        TableAction::SetWidgetSize { instance_id, size_override } => {
            session.set_widget_size(&instance_id, size_override)
        }
        TableAction::DuplicateWidget { source_instance_id, new_instance_id, x, y } => {
            session.duplicate_widget(&source_instance_id, &new_instance_id, x, y)
        }
        TableAction::AttachWidgetToCard { instance_id, card_id, x, y } => {
            let _ = session.attach_widget_to_card(&instance_id, &card_id, x, y, acting);
        }
        TableAction::DrawBagToken { instance_id } => {
            let _ = session.draw_bag_token(&instance_id, acting);
        }
        TableAction::ReturnBagToken { instance_id, slot_id } => {
            let _ = session.return_bag_token(&instance_id, &slot_id, acting);
        }
        TableAction::PlaceBagTokenOnTable { instance_id, slot_id, new_instance_id, x, y } => {
            session.place_bag_token_on_table(&instance_id, &slot_id, &new_instance_id, x, y, acting)
        }
        TableAction::ResetBag { instance_id } => {
            let _ = session.reset_bag(&instance_id, acting);
        }
        TableAction::SetBagRemaining { instance_id, bag_remaining } => {
            session.set_bag_remaining(&instance_id, &bag_remaining, acting)
        }
        TableAction::ReturnImageTokenToBag { bag_instance_id, token_instance_id } => {
            let _ = session.return_image_token_to_bag(&bag_instance_id, &token_instance_id, acting);
        }
        TableAction::GeneratePack { set_id } => session.generate_pack(&set_id, acting),
        TableAction::CreateCardFromLibrary { definition_id, x, y } => {
            session.create_card_from_library(&definition_id, x, y, acting)
        }
        // A practice game has nobody to show drag previews to.
        TableAction::PreviewCardDrag { .. }
        | TableAction::EndCardDragPreview
        | TableAction::PreviewArrowDrag { .. }
        | TableAction::EndArrowDragPreview => {}
    }
}
