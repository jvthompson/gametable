//! Card sorting and filtering shared by the deck editor, the card editor and the in-game card
//! library. Port of `src/core/card_filters.lua`.

use std::collections::{HashMap, HashSet};

use crate::json::JsonValue;
use crate::models::{CardDefinition, GameSet, TagGroup};
use crate::tag_match::{card_has_tag, parse_tag_entry};
use crate::util::trim_lua_whitespace;

/// How to order a card list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardSortOrder {
    /// By the game's set order (cards without a set last), then title.
    BySet,
    /// By title.
    ByName,
    /// By the tag order of the tag group with this id, then title.
    ByTagGroup(String),
}

impl CardSortOrder {
    pub const BY_SET_KEY: &'static str = "set";
    pub const BY_NAME_KEY: &'static str = "name";

    /// The key the original stores for this order (`"set"`, `"name"`, or a tag group id).
    pub fn key(&self) -> &str {
        match self {
            CardSortOrder::BySet => Self::BY_SET_KEY,
            CardSortOrder::ByName => Self::BY_NAME_KEY,
            CardSortOrder::ByTagGroup(group_id) => group_id,
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            Self::BY_SET_KEY => CardSortOrder::BySet,
            Self::BY_NAME_KEY => CardSortOrder::ByName,
            group_id => CardSortOrder::ByTagGroup(group_id.to_string()),
        }
    }
}

/// Returns the cards sorted by `order`, then by lowercase title, then by original position.
pub fn sort_card_definitions<'cards>(
    cards: &[&'cards CardDefinition],
    order: &CardSortOrder,
    sets: &[GameSet],
    tag_groups: &[TagGroup],
) -> Vec<&'cards CardDefinition> {
    let rank_of: Box<dyn Fn(&CardDefinition) -> usize> = match order {
        CardSortOrder::BySet => {
            let set_positions: HashMap<&str, usize> =
                sets.iter().enumerate().map(|(position, set)| (set.id.as_str(), position)).collect();
            Box::new(move |card| match &card.set_id {
                None => sets.len(),
                Some(set_id) => set_positions.get(set_id.as_str()).copied().unwrap_or(sets.len()),
            })
        }
        CardSortOrder::ByName => Box::new(|_| 0),
        CardSortOrder::ByTagGroup(group_id) => {
            let group = tag_groups.iter().find(|group| group.id == *group_id);
            let tags: &[String] = group.map(|group| group.tags.as_slice()).unwrap_or(&[]);
            let mut tag_positions: HashMap<&str, usize> = HashMap::new();
            for (position, tag) in tags.iter().enumerate() {
                // Lua's `order[t] = i - 1` keeps the last position of a duplicated tag.
                tag_positions.insert(tag.as_str(), position);
            }
            Box::new(move |card| {
                let mut best_rank = tags.len();
                for entry in &card.types {
                    let (entry_group, entry_tag) = parse_tag_entry(entry);
                    let entry_is_in_group = match (entry_group, group) {
                        (None, _) => true,
                        (Some(entry_group), Some(group)) => entry_group == group.name,
                        (Some(_), None) => false,
                    };
                    if entry_is_in_group && let Some(&position) = tag_positions.get(entry_tag) {
                        best_rank = best_rank.min(position);
                    }
                }
                best_rank
            })
        }
    };

    let mut ranked: Vec<(usize, String, usize, &'cards CardDefinition)> = cards
        .iter()
        .enumerate()
        .map(|(position, card)| (rank_of(card), card.card_title.to_ascii_lowercase(), position, *card))
        .collect();
    ranked.sort_by(|a, b| (a.0, &a.1, a.2).cmp(&(b.0, &b.1, b.2)));
    ranked.into_iter().map(|(_, _, _, card)| card).collect()
}

/// The filter choices made in a card filter bar.
#[derive(Debug, Clone, Default)]
pub struct CardFilterSelection {
    /// Tag group id -> tags; a card must have at least one of them.
    pub included_tags_by_group_id: HashMap<String, HashSet<String>>,
    /// Tag group id -> tags; a card with any of them is hidden.
    pub excluded_tags_by_group_id: HashMap<String, HashSet<String>>,
    /// When not empty, cards from other sets are hidden (cards without a set always show).
    pub selected_set_ids: HashSet<String>,
    /// Matches title or id; `":field text"` searches one extra field instead.
    pub search_query: String,
}

/// Whether a card passes the filter bar's tag, set and text filters.
pub fn card_matches_filters(card: &CardDefinition, tag_groups: &[TagGroup], filters: &CardFilterSelection) -> bool {
    for group in tag_groups {
        if let Some(excluded_tags) = filters.excluded_tags_by_group_id.get(&group.id)
            && excluded_tags.iter().any(|tag| card_has_tag(&card.types, &group.name, tag))
        {
            return false;
        }
        if let Some(included_tags) = filters.included_tags_by_group_id.get(&group.id)
            && !included_tags.is_empty()
            && !included_tags.iter().any(|tag| card_has_tag(&card.types, &group.name, tag))
        {
            return false;
        }
    }
    if !filters.selected_set_ids.is_empty()
        && let Some(set_id) = &card.set_id
        && !filters.selected_set_ids.contains(set_id)
    {
        return false;
    }

    let query = trim_lua_whitespace(&filters.search_query).to_ascii_lowercase();
    if query.is_empty() {
        return true;
    }
    if let Some(field_query) = query.strip_prefix(':') {
        return card_extra_field_matches(card, field_query);
    }
    card.card_title.to_ascii_lowercase().contains(&query) || card.id.to_ascii_lowercase().contains(&query)
}

/// Handles `":fieldName text"` (already lowercased, without the colon): the named extra field
/// (compared case-insensitively) must contain the text.
fn card_extra_field_matches(card: &CardDefinition, field_query: &str) -> bool {
    // Lua: query:sub(2):match("^(%S+)%s*(.*)$")
    let is_lua_space = |character: char| matches!(character, ' ' | '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r');
    let field_name_end = field_query.find(is_lua_space).unwrap_or(field_query.len());
    if field_name_end == 0 {
        return false;
    }
    let field_name = &field_query[..field_name_end];
    let wanted_text = field_query[field_name_end..].trim_start_matches(is_lua_space);
    for (key, value) in card.extra_fields.iter() {
        if key.to_ascii_lowercase() == field_name {
            return lua_tostring(value).to_ascii_lowercase().contains(wanted_text);
        }
    }
    false
}

/// Lua's `tostring` for a JSON value stored in a card's extra fields.
fn lua_tostring(value: &JsonValue) -> String {
    match value {
        JsonValue::String(text) => text.clone(),
        JsonValue::Number(number) => crate::json::format_like_c_percent_g(*number, 14),
        JsonValue::Bool(flag) => flag.to_string(),
        JsonValue::Null => "nil".to_string(),
        JsonValue::Array(_) | JsonValue::Object(_) => "table".to_string(),
    }
}
