//! The Set / tag-group filter buttons, Sort menu and search box shared by every filterable card
//! list (the deck editor's pool and the table's card library), so they all filter the same way
//! (see `gametable_core::card_filters`). Port of `src/ui/kit/card_filter_bar.lua`.

use std::collections::{HashMap, HashSet};

use gametable_core::card_filters::{CardFilterSelection, CardSortOrder, card_matches_filters, sort_card_definitions};
use gametable_core::models::{CardDefinition, GameDefinition};

use crate::app::Frame;
use crate::graphics::draw_list::Color;
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::filter_menu::{FilterOption, trigger_width};
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::with_alpha;
use crate::ui::widgets::{ButtonOptions, ButtonVariant, IconButtonOptions, ScrollbarStyle};

/// The filter bar's choices for one card list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CardFilterState {
    /// Tag group id -> tags a card must have one of.
    pub selected: HashMap<String, HashSet<String>>,
    /// Tag group id -> tags that hide a card.
    pub excluded: HashMap<String, HashSet<String>>,
    pub sets: HashSet<String>,
    pub query: String,
    /// `"set"`, `"name"`, a tag group id, or `None` for the game's order.
    pub sort_key: Option<String>,
}

impl CardFilterState {
    /// `newState(game)`: nothing selected, nothing excluded, no search.
    pub fn new(game: &GameDefinition) -> CardFilterState {
        let mut state = CardFilterState::default();
        state.clear(game);
        state
    }

    /// Resets every Set/Type filter; leaves the sort key and search alone.
    pub fn clear(&mut self, game: &GameDefinition) {
        self.sets.clear();
        self.selected = game.tag_groups.iter().map(|group| (group.id.clone(), HashSet::new())).collect();
        self.excluded = self.selected.clone();
    }

    /// True when any Set/Type filter is on (search and sort don't count).
    pub fn is_filtered(&self) -> bool {
        !self.sets.is_empty()
            || self.selected.values().any(|tags| !tags.is_empty())
            || self.excluded.values().any(|tags| !tags.is_empty())
    }

    /// The game's cards that pass the filters, in the chosen order.
    pub fn visible<'game>(&self, game: &'game GameDefinition) -> Vec<&'game CardDefinition> {
        let selection = CardFilterSelection {
            included_tags_by_group_id: self.selected.clone(),
            excluded_tags_by_group_id: self.excluded.clone(),
            selected_set_ids: self.sets.clone(),
            search_query: self.query.clone(),
        };
        let matching: Vec<&CardDefinition> =
            game.cards().iter().filter(|card| card_matches_filters(card, &game.tag_groups, &selection)).collect();
        match &self.sort_key {
            None => matching,
            Some(key) => sort_card_definitions(&matching, &CardSortOrder::from_key(key), &game.sets, &game.tag_groups),
        }
    }
}

/// A button placed before the filters in compact mode (`opts.leading`).
#[derive(Debug, Clone, Copy)]
pub struct LeadingButton<'a> {
    pub label: &'a str,
    pub icon: Option<&'a str>,
    pub id: &'a str,
    pub variant: ButtonVariant,
}

/// Every distinct extra field name across the game's cards (first-seen casing kept, compared
/// ignoring case), alphabetical.
pub fn extra_field_names(game: &GameDefinition) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for card in game.cards() {
        for key in card.extra_fields.keys() {
            if seen.insert(key.to_ascii_lowercase()) {
                names.push(key.clone());
            }
        }
    }
    names.sort_by_key(|name| name.to_ascii_lowercase());
    names
}

const SUGGESTION_HEIGHT: f32 = 36.0;
const SUGGESTIONS_VISIBLE: usize = 6;

#[derive(Debug, Default)]
struct SuggestionState {
    /// The highlighted row (0-based).
    highlighted: usize,
    last_typed: Option<String>,
    /// Scroll the highlighted row into view (after a keyboard move).
    follow: bool,
}

impl Frame<'_> {
    /// `drawFilters`: "Clear Filters" plus a wrapped row of filter buttons and the Sort menu
    /// from `y`. Draws nothing for a game with no sets, tag groups or leading buttons. With
    /// `compact`, everything shares one wrapped row and Clear Filters comes last, only while a
    /// filter is on. Returns the y below it and the leading button clicked, if any.
    #[allow(clippy::too_many_arguments)]
    pub fn card_filters(
        &mut self,
        prefix: &str,
        state: &mut CardFilterState,
        game: &GameDefinition,
        x: f32,
        y: f32,
        width: f32,
        compact: bool,
        leading: &[LeadingButton],
    ) -> (f32, Option<usize>) {
        let groups: Vec<_> = game.tag_groups.iter().filter(|group| !group.tags.is_empty()).collect();
        if game.sets.is_empty() && groups.is_empty() && leading.is_empty() {
            return (y, None);
        }
        let mut row_y = y + 12.0;
        let clear_id = format!("{prefix}:clear");
        let clear_options = ButtonOptions::text().icon("filter_alt_off").id(&clear_id);
        if !compact {
            if self.button("Clear Filters", x - 8.0, row_y, None, Some(40.0), &clear_options) {
                state.clear(game);
            }
            row_y += 44.0;
        }
        let mut flow_x = x;
        let mut place = |width_needed: f32, row_y: &mut f32| {
            if flow_x + width_needed > x + width && flow_x > x {
                flow_x = x;
                *row_y += 48.0;
            }
            let placed = flow_x;
            flow_x += width_needed + 8.0;
            placed
        };
        let mut leading_clicked = None;
        for (index, button) in leading.iter().enumerate() {
            let options = ButtonOptions {
                variant: button.variant,
                icon: button.icon,
                id: Some(button.id),
                ..ButtonOptions::default()
            };
            let button_width = self.button_width(button.label, &options);
            let button_x = place(button_width, &mut row_y);
            if self.button(button.label, button_x, row_y, Some(button_width), Some(40.0), &options) {
                leading_clicked = Some(index);
            }
        }
        if !leading.is_empty() && (!game.sets.is_empty() || !groups.is_empty()) {
            let separator_x = place(1.0, &mut row_y);
            let color = self.palette().outline_variant;
            self.fill_rect(separator_x, row_y + 8.0, 1.0, 24.0, 0.0, color);
        }
        if game.sets.is_empty() && groups.is_empty() {
            return (row_y + 40.0, leading_clicked);
        }
        if !game.sets.is_empty() {
            let options: Vec<FilterOption> = game
                .sets
                .iter()
                .map(|set| {
                    let count =
                        game.cards().iter().filter(|card| card.set_id.as_deref() == Some(set.id.as_str())).count();
                    FilterOption::new(set.id.clone(), format!("{} ({count})", set.name))
                })
                .collect();
            let label_width = trigger_width(self, &format!("Set ({})", state.sets.len()));
            let set_x = place(label_width, &mut row_y);
            self.multi_select(&format!("{prefix}:sets"), set_x, row_y, "Set", &options, &mut state.sets, None);
        }
        for group in &groups {
            let options: Vec<FilterOption> =
                group.tags.iter().map(|tag| FilterOption::new(tag.clone(), tag.clone())).collect();
            let selected_count = state.selected.get(&group.id).map_or(0, HashSet::len);
            let group_x = place(trigger_width(self, &format!("{} ({selected_count})", group.name)), &mut row_y);
            let mut selected = state.selected.remove(&group.id).unwrap_or_default();
            let mut excluded = state.excluded.remove(&group.id).unwrap_or_default();
            let menu_id = format!("{prefix}:g:{}", group.id);
            self.multi_select(&menu_id, group_x, row_y, &group.name, &options, &mut selected, Some(&mut excluded));
            state.selected.insert(group.id.clone(), selected);
            state.excluded.insert(group.id.clone(), excluded);
        }
        let mut sort_options = Vec::new();
        if !game.sets.is_empty() {
            sort_options.push(FilterOption::new(CardSortOrder::BY_SET_KEY, "Set"));
        }
        sort_options.push(FilterOption::new(CardSortOrder::BY_NAME_KEY, "Name (A-Z)"));
        sort_options.extend(groups.iter().map(|group| FilterOption::new(group.id.clone(), group.name.clone())));
        let sort_label = sort_options
            .iter()
            .find(|option| Some(&option.id) == state.sort_key.as_ref())
            .map_or_else(|| "Sort".to_string(), |option| format!("Sort: {}", option.name));
        let sort_x = place(trigger_width(self, &sort_label), &mut row_y);
        (state.sort_key, _) =
            self.sort_menu(&format!("{prefix}:sort"), sort_x, row_y, &sort_options, state.sort_key.take());
        if compact && state.is_filtered() {
            let button_width = self.button_width("Clear Filters", &clear_options);
            let clear_x = place(button_width, &mut row_y);
            if self.button("Clear Filters", clear_x, row_y, Some(button_width), Some(40.0), &clear_options) {
                state.clear(game);
            }
        }
        (row_y + 40.0, leading_clicked)
    }

    /// `drawSearch`: a 44-tall search field (name or id, or `:field text`) with a clear button;
    /// typing `:` offers the game's extra field names. Returns the y below it.
    #[allow(clippy::too_many_arguments)]
    pub fn card_search(
        &mut self,
        prefix: &str,
        state: &mut CardFilterState,
        game: &GameDefinition,
        x: f32,
        y: f32,
        width: f32,
        label_background: Option<Color>,
        clear_color: Option<Color>,
    ) -> f32 {
        let id = format!("{prefix}:search");
        let field_width = width - if state.query.is_empty() { 0.0 } else { 44.0 };
        let options = TextFieldOptions {
            hint: Some("Search by name or ID, or :field text"),
            height: Some(44.0),
            label_background,
            ..TextFieldOptions::label("Search")
        };
        self.text_field(&id, x, y, field_width, &mut state.query, &options);
        if !state.query.is_empty() {
            let clear_id = format!("{prefix}:clearq");
            let clear_options =
                IconButtonOptions { color: clear_color, ..IconButtonOptions::tooltip("Clear search").id(&clear_id) };
            if self.icon_button("clear", x + width - 40.0, y + 2.0, &clear_options) {
                state.query.clear();
            }
        }
        self.field_suggestions(&id, state, game, x, y + 44.0, width);
        y + 44.0
    }

    /// The `:fieldName` autocomplete popup under the search field while it has focus and the
    /// query is still a bare `:name` token. Up/Down move the highlight (wrapping); Enter or a
    /// click fills in `:name ` to keep typing.
    #[allow(clippy::too_many_arguments)]
    fn field_suggestions(
        &mut self,
        id: &str,
        state: &mut CardFilterState,
        game: &GameDefinition,
        x: f32,
        y: f32,
        width: f32,
    ) {
        if self.ui.focus.as_deref() != Some(id) {
            return;
        }
        let Some(typed) = state.query.strip_prefix(':').filter(|rest| !rest.contains(' ')) else { return };
        let typed = typed.to_ascii_lowercase();
        let names: Vec<String> = extra_field_names(game)
            .into_iter()
            .filter(|name| typed.is_empty() || name.to_ascii_lowercase().contains(&typed))
            .collect();
        if names.is_empty() {
            return;
        }
        let suggestion_id = format!("{id}:suggest");
        let scroll_id = format!("{id}:suggest:scroll");
        let suggestion = self.ui.state::<SuggestionState>(&suggestion_id);
        let restart = suggestion.last_typed.as_deref() != Some(typed.as_str());
        if restart {
            suggestion.highlighted = 0;
            suggestion.last_typed = Some(typed.clone());
        }
        suggestion.highlighted = suggestion.highlighted.min(names.len() - 1);
        if restart {
            self.ui.scroll_to(&scroll_id, 0.0);
        }

        let mut pick: Option<String> = None;
        let keys = self.ui.input.keys.clone();
        let suggestion = self.ui.state::<SuggestionState>(&suggestion_id);
        for key_press in keys {
            match key_press.key {
                "down" => {
                    suggestion.highlighted =
                        if suggestion.highlighted + 1 < names.len() { suggestion.highlighted + 1 } else { 0 };
                    suggestion.follow = true;
                }
                "up" => {
                    suggestion.highlighted =
                        if suggestion.highlighted > 0 { suggestion.highlighted - 1 } else { names.len() - 1 };
                    suggestion.follow = true;
                }
                "return" | "kpenter" => pick = Some(names[suggestion.highlighted].clone()),
                _ => {}
            }
        }

        let list_height = names.len().min(SUGGESTIONS_VISIBLE) as f32 * SUGGESTION_HEIGHT;
        let popup_height = list_height + 8.0;
        let (screen_width, screen_height) = self.ui.screen_size();
        let (mut popup_x, mut popup_y) = (x, y);
        if popup_y + popup_height > screen_height - 8.0 {
            popup_y = (y - popup_height - 48.0).max(8.0);
        }
        if popup_x + width > screen_width - 8.0 {
            popup_x = screen_width - 8.0 - width;
        }
        self.overlay(|frame| {
            frame.ui.cover(-1e9, -1e9, 2e9, 2e9);
            // A click inside the popup keeps the field focused (like a click on the field), so
            // the release can pick a row.
            if frame.ui.mouse_pressed(1) && frame.ui.hover_ignoring_layer(popup_x, popup_y, width, popup_height) {
                frame.ui.focus_hit_this_frame = true;
            }
            let palette = frame.palette();
            frame.shadow(popup_x, popup_y, width, popup_height, 4.0, 2.0);
            frame.fill_rect(popup_x, popup_y, width, popup_height, 4.0, palette.surface_container);
            // Keep the keyboard-highlighted row in view.
            let suggestion = frame.ui.state::<SuggestionState>(&suggestion_id);
            if suggestion.follow {
                suggestion.follow = false;
                let highlighted = suggestion.highlighted;
                let top = highlighted as f32 * SUGGESTION_HEIGHT;
                let bottom = top + SUGGESTION_HEIGHT;
                let current = frame.ui.scroll_offset(&scroll_id);
                if top < current {
                    frame.ui.scroll_to(&scroll_id, top);
                } else if bottom > current + list_height {
                    frame.ui.scroll_to(&scroll_id, bottom - list_height);
                }
            }
            let offset = frame.begin_scroll(&scroll_id, popup_x, popup_y + 4.0, width, list_height);
            let mut row_y = popup_y + 4.0 - offset;
            for (index, name) in names.iter().enumerate() {
                let row_id = format!("{id}:sg{}", index + 1);
                let region = frame.ui.region(&row_id, popup_x, row_y, width, SUGGESTION_HEIGHT, RegionOptions::hand());
                let highlighted = frame.ui.state::<SuggestionState>(&suggestion_id).highlighted;
                if region.hovered {
                    frame.ui.state::<SuggestionState>(&suggestion_id).highlighted = index;
                    frame.fill_rect(
                        popup_x,
                        row_y,
                        width,
                        SUGGESTION_HEIGHT,
                        0.0,
                        with_alpha(palette.on_surface, 0.08),
                    );
                } else if index == highlighted {
                    frame.fill_rect(popup_x, row_y, width, SUGGESTION_HEIGHT, 0.0, with_alpha(palette.primary, 0.12));
                }
                let options = TextOptions::new()
                    .width(width - 24.0)
                    .middle_of(SUGGESTION_HEIGHT)
                    .ellipsis()
                    .color(palette.on_surface);
                frame.text(name, popup_x + 12.0, row_y, options);
                if region.clicked {
                    pick = Some(name.clone());
                }
                row_y += SUGGESTION_HEIGHT;
            }
            frame.end_scroll(&scroll_id, names.len() as f32 * SUGGESTION_HEIGHT, &ScrollbarStyle::default());
        });
        if let Some(name) = pick {
            state.query = format!(":{name} ");
            self.ui.focus_field_at_end(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gametable_core::models::TagGroup;

    fn game() -> GameDefinition {
        let mut game = GameDefinition::new("g", "Game");
        game.tag_groups.push(TagGroup { id: "type".into(), name: "Type".into(), tags: vec!["Hero".into()] });
        let mut card = CardDefinition::new("c1", "Alpha");
        card.extra_fields.insert("Cost", 1.0);
        card.extra_fields.insert("cost", 2.0);
        card.extra_fields.insert("Attack", 3.0);
        card.types.push("Hero".into());
        game.cards_mut().push(card);
        game.cards_mut().push(CardDefinition::new("c2", "Beta"));
        game
    }

    #[test]
    fn extra_field_names_are_deduplicated_ignoring_case() {
        assert_eq!(extra_field_names(&game()), vec!["Attack".to_string(), "Cost".to_string()]);
    }

    #[test]
    fn filtering_and_clearing() {
        let game = game();
        let mut state = CardFilterState::new(&game);
        assert!(!state.is_filtered());
        assert_eq!(state.visible(&game).len(), 2);
        state.selected.get_mut("type").unwrap().insert("Hero".into());
        assert!(state.is_filtered());
        assert_eq!(state.visible(&game).iter().map(|card| card.id.as_str()).collect::<Vec<_>>(), ["c1"]);
        state.query = "zzz".into();
        state.clear(&game);
        assert!(!state.is_filtered());
        assert_eq!(state.query, "zzz", "clear leaves the search alone");
    }
}
