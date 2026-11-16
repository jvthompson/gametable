//! `ui.dropdown`: an outlined select field whose option list opens in a popup layer.

use crate::app::Frame;
use crate::graphics::draw_list::Color;
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::theme::{self, Weight, with_alpha};
use crate::ui::widgets::{Choice, ScrollbarStyle};

/// Options for [`Frame::dropdown`].
#[derive(Debug, Clone, Copy, Default)]
pub struct DropdownOptions<'a> {
    /// Floating label on the border.
    pub label: Option<&'a str>,
    pub disabled: bool,
    /// Field height (default 48).
    pub height: Option<f32>,
    /// Popup height limit (default 360).
    pub max_popup_height: Option<f32>,
    /// Minimum popup width (default: the field's).
    pub popup_width: Option<f32>,
    /// Shown when no option matches the selection.
    pub placeholder: Option<&'a str>,
    /// Text size (default 16).
    pub size: Option<f32>,
    pub label_background: Option<Color>,
}

struct DropdownState<T> {
    open: bool,
    /// A pick made in the popup, applied when the field next draws.
    pick: Option<T>,
}

impl<T> Default for DropdownState<T> {
    fn default() -> Self {
        DropdownState { open: false, pick: None }
    }
}

const ITEM_HEIGHT: f32 = 40.0;

impl Frame<'_> {
    /// `ui.dropdown(id, x, y, w, options, selected, opts)`: returns the selected value.
    #[allow(clippy::too_many_arguments)]
    pub fn dropdown<T: PartialEq + Clone + 'static>(
        &mut self,
        id: &str,
        x: f32,
        y: f32,
        width: f32,
        choices: &[Choice<T>],
        selected: T,
        options: &DropdownOptions,
    ) -> T {
        let height = options.height.unwrap_or(48.0);
        let palette = self.palette();
        let mut selected = selected;
        if let Some(pick) = self.ui.state::<DropdownState<T>>(id).pick.take() {
            selected = pick;
        }
        let current = choices.iter().find(|choice| choice.value == selected).map(|choice| choice.label.clone());
        let region = if options.disabled {
            Default::default()
        } else {
            self.ui.region(id, x, y, width, height, RegionOptions::hand())
        };
        let open = self.ui.state::<DropdownState<T>>(id).open;
        let border = if options.disabled {
            with_alpha(palette.on_surface, 0.12)
        } else if open {
            palette.primary
        } else {
            palette.outline
        };
        self.outline_rect(x + 0.5, y + 0.5, width - 1.0, height - 1.0, 4.0, border, if open { 2.0 } else { 1.0 });
        if let Some(label) = options.label {
            let label_font = theme::font(12.0, Weight::Regular);
            let label_width = self.fonts.width(label_font, label) + 8.0;
            let label_background = options.label_background.unwrap_or(palette.surface);
            self.fill_rect(x + 8.0, y - 2.0, label_width, 4.0, 0.0, label_background);
            let color = if open { palette.primary } else { palette.on_surface_variant };
            let label_y = y - self.fonts.height(label_font) / 2.0;
            self.text(label, x + 12.0, label_y, TextOptions::new().font(label_font).color(color));
        }
        let value_color = if options.disabled {
            palette.disabled
        } else if current.is_some() {
            palette.on_surface
        } else {
            palette.on_surface_variant
        };
        let shown = current.unwrap_or_else(|| options.placeholder.unwrap_or_default().to_string());
        let value_options = TextOptions::new()
            .width(width - 48.0)
            .middle_of(height)
            .ellipsis()
            .size(options.size.unwrap_or(16.0))
            .color(value_color);
        self.text(&shown, x + 12.0, y, value_options);
        let arrow_color = if options.disabled { palette.disabled } else { palette.on_surface_variant };
        self.icon_centered("arrow_drop_down", x + width - 40.0, y, 36.0, height, 24.0, arrow_color);
        if region.clicked {
            let state = self.ui.state::<DropdownState<T>>(id);
            state.open = !state.open;
        }
        if self.ui.state::<DropdownState<T>>(id).open {
            self.overlay(|frame| frame.dropdown_popup(id, x, y, width, height, choices, &selected, options));
        }
        selected
    }

    #[allow(clippy::too_many_arguments)]
    fn dropdown_popup<T: PartialEq + Clone + 'static>(
        &mut self,
        id: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        choices: &[Choice<T>],
        selected: &T,
        options: &DropdownOptions,
    ) {
        let palette = self.palette();
        let (screen_width, screen_height) = self.ui.screen_size();
        let popup_height = (choices.len() as f32 * ITEM_HEIGHT + 16.0)
            .min(options.max_popup_height.unwrap_or(360.0))
            .min(screen_height - 16.0);
        let mut popup_y = y + height + 2.0;
        if popup_y + popup_height > screen_height - 8.0 {
            popup_y = (y - popup_height - 2.0).max(8.0);
        }
        let popup_width = width.max(options.popup_width.unwrap_or(0.0));
        let mut popup_x = x;
        if popup_x + popup_width > screen_width - 8.0 {
            popup_x = screen_width - 8.0 - popup_width;
        }
        // Swallow clicks outside so they only close the popup.
        self.ui.cover(-1e9, -1e9, 2e9, 2e9);
        self.shadow(popup_x, popup_y, popup_width, popup_height, 4.0, 2.0);
        self.fill_rect(popup_x, popup_y, popup_width, popup_height, 4.0, palette.surface_container);
        let scroll_id = format!("{id}:popup");
        let offset = self.begin_scroll(&scroll_id, popup_x, popup_y + 8.0, popup_width, popup_height - 16.0);
        let mut item_y = popup_y + 8.0 - offset;
        let mut picked = None;
        for (index, choice) in choices.iter().enumerate() {
            let item_id = format!("{id}:opt{}", index + 1);
            let region = self.ui.region(&item_id, popup_x, item_y, popup_width, ITEM_HEIGHT, RegionOptions::hand());
            if choice.value == *selected {
                self.fill_rect(popup_x, item_y, popup_width, ITEM_HEIGHT, 0.0, with_alpha(palette.primary, 0.12));
            }
            if region.hovered {
                self.fill_rect(popup_x, item_y, popup_width, ITEM_HEIGHT, 0.0, with_alpha(palette.on_surface, 0.08));
            }
            let label_options = TextOptions::new()
                .width(popup_width - 24.0)
                .middle_of(ITEM_HEIGHT)
                .ellipsis()
                .color(palette.on_surface);
            self.text(&choice.label, popup_x + 12.0, item_y, label_options);
            if region.clicked {
                picked = Some(choice.value.clone());
            }
            item_y += ITEM_HEIGHT;
        }
        self.end_scroll(&scroll_id, choices.len() as f32 * ITEM_HEIGHT, &ScrollbarStyle::default());
        let pressed_outside = self.ui.mouse_pressed(1)
            && !self.ui.hover(popup_x, popup_y, popup_width, popup_height)
            && !self.ui.hover_ignoring_layer(x, y, width, height);
        let right_pressed = self.ui.mouse_pressed(2);
        let state = self.ui.state::<DropdownState<T>>(id);
        if let Some(value) = picked {
            state.pick = Some(value);
            state.open = false;
        }
        if pressed_outside || right_pressed {
            state.open = false;
        }
    }
}
