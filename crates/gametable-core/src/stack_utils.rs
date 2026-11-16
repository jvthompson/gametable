//! Piles on the table. Port of `src/core/stack_utils.lua`.
//!
//! A pile is a chain of cards linked through `stack_parent_id`; its root is the bottom card.

use std::collections::{HashMap, HashSet};

use crate::models::CardInstance;

/// `instance id -> card`, keeping the first card for a repeated id.
fn index_cards_by_id(cards: &[CardInstance]) -> HashMap<&str, &CardInstance> {
    let mut index = HashMap::with_capacity(cards.len());
    for card in cards {
        index.entry(card.instance_id.as_str()).or_insert(card);
    }
    index
}

fn root_id_with_index<'cards>(index: &HashMap<&str, &'cards CardInstance>, card: &'cards CardInstance) -> &'cards str {
    let mut current = card;
    let mut visited: HashSet<&str> = HashSet::new();
    while let Some(parent_id) = &current.stack_parent_id {
        if !visited.insert(current.instance_id.as_str()) {
            break; // a cycle: stop where we are
        }
        match index.get(parent_id.as_str()) {
            Some(parent) => current = parent,
            None => break, // a dangling parent id
        }
    }
    &current.instance_id
}

/// The instance id of the bottom card of `card`'s pile.
pub fn root_id_of<'cards>(cards: &'cards [CardInstance], card: &'cards CardInstance) -> &'cards str {
    root_id_with_index(&index_cards_by_id(cards), card)
}

/// Every card in the pile whose root is `root_instance_id`, in their original order.
pub fn stack_of<'cards>(cards: &'cards [CardInstance], root_instance_id: &str) -> Vec<&'cards CardInstance> {
    let index = index_cards_by_id(cards);
    cards
        .iter()
        .filter(|card| card.instance_id == root_instance_id || root_id_with_index(&index, card) == root_instance_id)
        .collect()
}

/// Cards grouped by pile root, with the roots in first-seen order.
pub struct StacksByRoot<'cards> {
    pub root_ids_in_order: Vec<&'cards str>,
    pub cards_by_root_id: HashMap<&'cards str, Vec<&'cards CardInstance>>,
}

pub fn group_by_stack(cards: &[CardInstance]) -> StacksByRoot<'_> {
    let index = index_cards_by_id(cards);
    let mut grouped = StacksByRoot { root_ids_in_order: Vec::new(), cards_by_root_id: HashMap::new() };
    for card in cards {
        let root_id = root_id_with_index(&index, card);
        let pile = grouped.cards_by_root_id.entry(root_id).or_insert_with(|| {
            grouped.root_ids_in_order.push(root_id);
            Vec::new()
        });
        pile.push(card);
    }
    grouped
}

/// The card with the highest z-index; ties go to the earlier card. `None` for an empty pile.
pub fn top_of<'cards>(stack: &[&'cards CardInstance]) -> Option<&'cards CardInstance> {
    let mut best = *stack.first()?;
    for card in &stack[1..] {
        if best.z_index < card.z_index {
            best = card;
        }
    }
    Some(best)
}
