//! Hides other players' private cards before a state is sent to one recipient.
//! Port of `src/core/state_filter.lua`.

use std::collections::HashSet;

use crate::models::{CardZone, HIDDEN_DEFINITION_ID, TableState};

/// A copy of `true_state` as `recipient_id` may see it: cards another player owns that are in a
/// hand, or in a zone not marked visible to all, are turned face down with a hidden identity.
///
/// Note: unowned cards and face-down table cards keep their real `definition_id`, as in the
/// original.
pub fn filter_state_for_recipient(
    true_state: &TableState,
    recipient_id: &str,
    zone_ids_visible_to_all: &HashSet<String>,
) -> TableState {
    let mut filtered = true_state.clone();
    for card in filtered.cards_mut() {
        let is_in_private_zone =
            card.zone == CardZone::Zone && !zone_ids_visible_to_all.contains(card.zone_id.as_deref().unwrap_or(""));
        let is_owned_by_someone_else = card.owner_id.as_deref().is_some_and(|owner_id| owner_id != recipient_id);
        let is_opponent_private = is_owned_by_someone_else && (card.zone == CardZone::Hand || is_in_private_zone);
        if is_opponent_private {
            card.face_up = false;
            card.definition_id = HIDDEN_DEFINITION_ID.to_string();
        }
    }
    filtered
}
