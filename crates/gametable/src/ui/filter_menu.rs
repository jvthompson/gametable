//! Port of `src/ui/kit/filter_menu.lua` (`multi_select_filter_menu.dart` and the `CardSortMenu`
//! of `card_sort_menu.dart`): outlined trigger buttons whose popup stays open while toggling
//! (filters) or closes on a pick (sort).

use std::collections::HashSet;

use crate::app::Frame;
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::theme::{Palette, Weight, fixed, with_alpha};
use crate::ui::widgets::{IconButtonOptions, ToggleOptions};

/// Option rows per column before wrapping to a new column.
const MAX_PER_COLUMN: usize = 15;
/// Columns visible at once; beyond this the options are paged.
const MAX_COLUMNS: usize = 3;
const ROW_HEIGHT: f32 = 36.0;
const PAGE_ROW_HEIGHT: f32 = 36.0;

/// One option of a filter or sort menu.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterOption {
    pub id: String,
    pub name: String,
}

impl FilterOption {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> FilterOption {
        FilterOption { id: id.into(), name: name.into() }
    }
}

#[derive(Debug, Default)]
struct MenuState {
    open: bool,
    page: usize,
    /// A sort pick, applied when the trigger next draws: `Some(None)` is "Default order".
    pick: Option<Option<String>>,
}

/// `filterMenu.triggerWidth(label)`.
pub fn trigger_width(frame: &mut Frame, label: &str) -> f32 {
    frame.text_width(label, 14.0, Weight::Medium) + 48.0
}

/// `filterMenu.close(id)`.
pub fn close(frame: &mut Frame, id: &str) {
    frame.ui.state::<MenuState>(id).open = false;
}

impl Frame<'_> {
    fn filter_trigger(&mut self, id: &str, x: f32, y: f32, label: &str, open: bool) -> (f32, bool) {
        let width = trigger_width(self, label);
        let height = 40.0;
        let region = self.ui.region(id, x, y, width, height, RegionOptions::hand());
        let palette = self.palette();
        self.outline_rect(x + 0.5, y + 0.5, width - 1.0, height - 1.0, height / 2.0, palette.outline, 1.0);
        if region.hovered || open {
            self.fill_rect(x, y, width, height, height / 2.0, with_alpha(palette.primary, 0.08));
        }
        self.text(label, x + 16.0, y, TextOptions::new().medium().middle_of(height).color(palette.primary));
        self.icon("arrow_drop_down", x + width - 30.0, y + 8.0, 24.0, palette.primary);
        (width, region.clicked)
    }

    /// The popup panel under a trigger, clamped on screen and closed by an outside click.
    #[allow(clippy::too_many_arguments)]
    fn filter_popup(
        &mut self,
        id: &str,
        x: f32,
        y: f32,
        trigger_width: f32,
        popup_width: f32,
        popup_height: f32,
        draw: impl FnOnce(&mut Frame, f32, f32, &'static Palette),
    ) {
        self.overlay(|frame| {
            let palette = frame.palette();
            let (screen_width, screen_height) = frame.ui.screen_size();
            let (mut popup_x, mut popup_y) = (x, y + 44.0);
            if popup_x + popup_width > screen_width - 8.0 {
                popup_x = (screen_width - 8.0 - popup_width).max(8.0);
            }
            if popup_y + popup_height > screen_height - 8.0 {
                popup_y = (screen_height - 8.0 - popup_height).max(8.0);
            }
            frame.ui.cover(-1e9, -1e9, 2e9, 2e9);
            frame.shadow(popup_x, popup_y, popup_width, popup_height, 4.0, 2.0);
            frame.fill_rect(popup_x, popup_y, popup_width, popup_height, 4.0, palette.surface_container);
            draw(frame, popup_x, popup_y, palette);
            let pressed = frame.ui.mouse_pressed(1) || frame.ui.mouse_pressed(2);
            if pressed
                && !frame.ui.hover_ignoring_layer(popup_x, popup_y, popup_width, popup_height)
                && !frame.ui.hover_ignoring_layer(x, y, trigger_width, 40.0)
            {
                frame.ui.state::<MenuState>(id).open = false;
            }
        });
    }

    /// `filterMenu.multiSelect`: a "Label (n)" trigger whose popup toggles options in
    /// `selected`, with Select All, paging past 45 options and, with `excluded`, an exclude
    /// column. Returns the trigger's width.
    #[allow(clippy::too_many_arguments)]
    pub fn multi_select(
        &mut self,
        id: &str,
        x: f32,
        y: f32,
        label: &str,
        options: &[FilterOption],
        selected: &mut HashSet<String>,
        mut excluded: Option<&mut HashSet<String>>,
    ) -> f32 {
        let count = options.iter().filter(|option| selected.contains(&option.id)).count();
        let open = self.ui.state::<MenuState>(id).open;
        let (width, clicked) = self.filter_trigger(id, x, y, &format!("{label} ({count})"), open);
        if clicked {
            let state = self.ui.state::<MenuState>(id);
            state.open = !state.open;
        }
        if !self.ui.state::<MenuState>(id).open {
            return width;
        }

        let per_page = MAX_PER_COLUMN * MAX_COLUMNS;
        let total_pages = options.len().div_ceil(per_page).max(1);
        let state = self.ui.state::<MenuState>(id);
        state.page = state.page.clamp(1, total_pages);
        let page = state.page;
        let paged = total_pages > 1;

        let has_exclude = excluded.is_some();
        let mut column_width: f32 = 0.0;
        for option in options {
            column_width = column_width.max(self.text_width(&option.name, 14.0, Weight::Regular));
        }
        column_width += 48.0 + if has_exclude { 40.0 } else { 0.0 } + 16.0;
        let (columns, rows) = if paged {
            (MAX_COLUMNS, MAX_PER_COLUMN)
        } else {
            (options.len().div_ceil(MAX_PER_COLUMN).max(1), options.len().min(MAX_PER_COLUMN))
        };
        let popup_width = (columns as f32 * column_width + (columns - 1) as f32 * 16.0 + 16.0).max(220.0);
        let popup_height =
            8.0 + ROW_HEIGHT + 9.0 + if paged { PAGE_ROW_HEIGHT + 4.0 } else { 0.0 } + rows as f32 * ROW_HEIGHT + 8.0;
        self.filter_popup(id, x, y, width, popup_width, popup_height, |frame, popup_x, popup_y, palette| {
            let all_selected = !options.is_empty() && count == options.len();
            let mut row_y = popup_y + 8.0;
            let region =
                frame.ui.region(&format!("{id}:all"), popup_x, row_y, popup_width, ROW_HEIGHT, RegionOptions::hand());
            if region.hovered {
                frame.fill_rect(popup_x, row_y, popup_width, ROW_HEIGHT, 0.0, with_alpha(palette.on_surface, 0.08));
            }
            let all_checkbox = frame.checkbox(
                &format!("{id}:allcb"),
                popup_x + 16.0,
                row_y + 9.0,
                all_selected,
                &ToggleOptions::default(),
            );
            let all_label = if all_selected { "Deselect All" } else { "Select All" };
            let label_options = TextOptions::new().middle_of(ROW_HEIGHT).color(palette.on_surface);
            frame.text(all_label, popup_x + 48.0, row_y, label_options);
            if region.clicked || all_checkbox != all_selected {
                for option in options {
                    if all_selected {
                        selected.remove(&option.id);
                    } else {
                        selected.insert(option.id.clone());
                    }
                }
            }
            row_y += ROW_HEIGHT + 4.0;
            frame.divider(popup_x, row_y, popup_width, Some(palette.outline_variant));
            row_y += 5.0;
            if paged {
                let page_label = format!("Page {page} / {total_pages}");
                let label_width = (frame.text_width(&page_label, 12.0, Weight::Regular) + 12.0).max(56.0);
                let row_x = popup_x + (popup_width - (32.0 + label_width + 32.0)) / 2.0;
                let previous_id = format!("{id}:prevpage");
                let previous =
                    IconButtonOptions::tooltip("Previous Page").id(&previous_id).size(32.0, 18.0).disabled(page <= 1);
                if frame.icon_button("chevron_left", row_x, row_y, &previous) {
                    frame.ui.state::<MenuState>(id).page -= 1;
                }
                let page_options = TextOptions::new()
                    .width(label_width)
                    .middle_of(32.0)
                    .size(12.0)
                    .centered()
                    .color(palette.on_surface_variant);
                frame.text(&page_label, row_x + 32.0, row_y, page_options);
                let next_id = format!("{id}:nextpage");
                let next =
                    IconButtonOptions::tooltip("Next Page").id(&next_id).size(32.0, 18.0).disabled(page >= total_pages);
                if frame.icon_button("chevron_right", row_x + 32.0 + label_width, row_y, &next) {
                    frame.ui.state::<MenuState>(id).page += 1;
                }
                row_y += PAGE_ROW_HEIGHT + 4.0;
            }
            let first = (page - 1) * per_page;
            let last = (page * per_page).min(options.len());
            for (index, option) in options.iter().enumerate().take(last).skip(first) {
                let page_index = index - first;
                let (column, row) = (page_index / MAX_PER_COLUMN, page_index % MAX_PER_COLUMN);
                let option_x = popup_x + 8.0 + column as f32 * (column_width + 16.0);
                let option_y = row_y + row as f32 * ROW_HEIGHT;
                let row_width = column_width - if has_exclude { 40.0 } else { 0.0 };
                let row_region = frame.ui.region(
                    &format!("{id}:o{}", index + 1),
                    option_x,
                    option_y,
                    row_width,
                    ROW_HEIGHT,
                    RegionOptions::hand(),
                );
                if row_region.hovered {
                    frame.fill_rect(
                        option_x,
                        option_y,
                        row_width,
                        ROW_HEIGHT,
                        0.0,
                        with_alpha(palette.on_surface, 0.08),
                    );
                }
                let was_selected = selected.contains(&option.id);
                let checkbox_id = format!("{id}:cb{}", index + 1);
                let checkbox = frame.checkbox(
                    &checkbox_id,
                    option_x + 8.0,
                    option_y + 9.0,
                    was_selected,
                    &ToggleOptions::default(),
                );
                frame.text(
                    &option.name,
                    option_x + 40.0,
                    option_y,
                    TextOptions::new().middle_of(ROW_HEIGHT).color(palette.on_surface),
                );
                if row_region.clicked || checkbox != was_selected {
                    if selected.contains(&option.id) {
                        selected.remove(&option.id);
                    } else {
                        selected.insert(option.id.clone());
                        if let Some(excluded) = excluded.as_deref_mut() {
                            excluded.remove(&option.id);
                        }
                    }
                }
                if let Some(excluded) = excluded.as_deref_mut() {
                    let is_excluded = excluded.contains(&option.id);
                    let exclude_id = format!("{id}:ex{}", index + 1);
                    let exclude_options = IconButtonOptions {
                        tooltip: Some(if is_excluded { "Stop excluding" } else { "Exclude cards with this tag" }),
                        color: Some(if is_excluded { fixed::RED } else { with_alpha(palette.on_surface, 0.38) }),
                        ..IconButtonOptions::default()
                    }
                    .id(&exclude_id)
                    .size(40.0, 18.0);
                    let icon = if is_excluded { "close" } else { "check_box_outline_blank" };
                    if frame.icon_button(icon, option_x + column_width - 40.0, option_y - 2.0, &exclude_options) {
                        if is_excluded {
                            excluded.remove(&option.id);
                        } else {
                            excluded.insert(option.id.clone());
                            selected.remove(&option.id);
                        }
                    }
                }
            }
        });
        width
    }

    /// `filterMenu.sortMenu`: a "Sort: name" trigger whose popup lists "Default order" and the
    /// options. Returns the selected id (`None` = default order) and the trigger's width.
    pub fn sort_menu(
        &mut self,
        id: &str,
        x: f32,
        y: f32,
        options: &[FilterOption],
        selected_id: Option<String>,
    ) -> (Option<String>, f32) {
        let mut selected_id = selected_id;
        if let Some(pick) = self.ui.state::<MenuState>(id).pick.take() {
            selected_id = pick;
        }
        let label = options
            .iter()
            .find(|option| Some(&option.id) == selected_id.as_ref())
            .map_or_else(|| "Sort".to_string(), |option| format!("Sort: {}", option.name));
        let open = self.ui.state::<MenuState>(id).open;
        let (width, clicked) = self.filter_trigger(id, x, y, &label, open);
        if clicked {
            let state = self.ui.state::<MenuState>(id);
            state.open = !state.open;
        }
        if self.ui.state::<MenuState>(id).open {
            let mut all: Vec<(Option<&str>, &str)> = vec![(None, "Default order")];
            all.extend(options.iter().map(|option| (Some(option.id.as_str()), option.name.as_str())));
            let mut popup_width: f32 = 200.0;
            for (_, name) in &all {
                popup_width = popup_width.max(self.text_width(name, 14.0, Weight::Regular) + 64.0);
            }
            let popup_height = 16.0 + all.len() as f32 * ROW_HEIGHT + 9.0;
            let current = selected_id.clone();
            self.filter_popup(id, x, y, width, popup_width, popup_height, |frame, popup_x, popup_y, palette| {
                let mut row_y = popup_y + 8.0;
                for (index, (option_id, name)) in all.iter().enumerate() {
                    let region = frame.ui.region(
                        &format!("{id}:s{}", index + 1),
                        popup_x,
                        row_y,
                        popup_width,
                        ROW_HEIGHT,
                        RegionOptions::hand(),
                    );
                    if region.hovered {
                        frame.fill_rect(
                            popup_x,
                            row_y,
                            popup_width,
                            ROW_HEIGHT,
                            0.0,
                            with_alpha(palette.on_surface, 0.08),
                        );
                    }
                    if *option_id == current.as_deref() {
                        frame.icon("check", popup_x + 12.0, row_y + 9.0, 18.0, palette.on_surface);
                    }
                    frame.text(
                        name,
                        popup_x + 40.0,
                        row_y,
                        TextOptions::new().middle_of(ROW_HEIGHT).color(palette.on_surface),
                    );
                    if region.clicked {
                        let state = frame.ui.state::<MenuState>(id);
                        state.pick = Some(option_id.map(str::to_string));
                        state.open = false;
                    }
                    row_y += ROW_HEIGHT;
                    if index == 0 {
                        frame.divider(popup_x, row_y + 4.0, popup_width, Some(palette.outline_variant));
                        row_y += 9.0;
                    }
                }
            });
        }
        (selected_id, width)
    }
}
