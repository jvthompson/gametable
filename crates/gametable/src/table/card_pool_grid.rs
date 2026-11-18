//! The "card pool" grid shared by the deck editor, the game definition editor's card view and the
//! server collection and trade screens. Port of `src/ui/table/card_pool_grid.lua`: tiles a third
//! larger than a table card, columns that exactly fill the width, a name below each tile. Callers
//! draw each tile's content; only the layout and scrolling are shared.
//!
//! GameTable addition: a zoom (1 = the original size) enlarges or shrinks each tile's card and
//! name text; [`CardZoom`] is the slider that sets it, remembered per screen in settings.

use crate::app::Frame;
use crate::data::settings::Settings;
use crate::table::card_render::{CARD_HEIGHT, CARD_WIDTH};
use crate::ui::draw::TextOptions;
use crate::ui::theme::{self, Weight};
use crate::ui::widgets::{IconButtonOptions, ScrollbarStyle, SliderOptions};

pub const NAME_LINES: f32 = 3.0;
pub const TILE_SCALE: f32 = 4.0 / 3.0;
/// The tile's card size at zoom 1.
pub const TILE_CARD_WIDTH: f32 = CARD_WIDTH * TILE_SCALE;
pub const TILE_CARD_HEIGHT: f32 = CARD_HEIGHT * TILE_SCALE;
/// The card names' text size at zoom 1.
pub const NAME_TEXT_SIZE: f32 = 11.0;
/// Names never shrink below this, however far the grid is zoomed out.
const SMALLEST_NAME_TEXT_SIZE: f32 = 9.0;

/// Where one tile goes, and how big its card and name are at the grid's zoom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoolTile {
    pub index: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    /// The tile's horizontal centre: draw the card at (`center_x`, `y + card_height / 2`)
    /// and the name at (`x`, `name_y`, `width`).
    pub center_x: f32,
    /// The card's size and its scale for `CardDraw::scale`.
    pub card_width: f32,
    pub card_height: f32,
    pub card_scale: f32,
    /// The name's text size, its top, and the room for its three lines.
    pub name_size: f32,
    pub name_y: f32,
    pub name_height: f32,
}

impl PoolTile {
    /// The vertical centre of the tile's card.
    pub fn card_center_y(&self) -> f32 {
        self.y + self.card_height / 2.0
    }

    /// Text options for the tile's name: centred, at most three lines, at the zoomed size.
    pub fn name_options(&self) -> TextOptions {
        TextOptions::new().size(self.name_size).width(self.width).centered().max_lines(NAME_LINES as usize)
    }
}

/// The name text size at a zoom.
fn name_text_size(zoom: f32) -> f32 {
    (NAME_TEXT_SIZE * zoom).max(SMALLEST_NAME_TEXT_SIZE)
}

impl Frame<'_> {
    /// `M.nameH()`: room for three lines of 11-point names.
    pub fn card_pool_name_height(&mut self) -> f32 {
        self.card_pool_name_height_at(1.0)
    }

    /// Room for three lines of names at a zoom.
    pub fn card_pool_name_height_at(&mut self, zoom: f32) -> f32 {
        NAME_LINES * self.fonts.height(theme::font(name_text_size(zoom), Weight::Regular))
    }

    /// `M.draw`: a scrollable grid of `count` tiles; `draw_tile` is called for each visible one.
    /// `padding` insets the tiles on both sides; the top inset is always 12. `zoom` scales each
    /// tile's card and name (1 = the original size).
    #[allow(clippy::too_many_arguments)]
    pub fn card_pool_grid(
        &mut self,
        scroll_id: &str,
        count: usize,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        padding: f32,
        zoom: f32,
        mut draw_tile: impl FnMut(&mut Frame, PoolTile),
    ) {
        let (card_width, card_height) = (TILE_CARD_WIDTH * zoom, TILE_CARD_HEIGHT * zoom);
        let name_height = self.card_pool_name_height_at(zoom);
        let (max_extent, row_height, gap) = (card_width + 24.0, card_height + 6.0 + name_height + 4.0, 12.0);
        let inner = width - 2.0 * padding;
        let columns = (((inner + gap) / (max_extent + gap)).ceil() as usize).max(1);
        let tile_width = (inner - (columns as f32 - 1.0) * gap) / columns as f32;
        let offset = self.begin_scroll(scroll_id, x, y, width, height);
        for index in 0..count {
            let (column, row) = (index % columns, index / columns);
            let tile_x = x + padding + column as f32 * (tile_width + gap);
            let tile_y = y + 12.0 + row as f32 * (row_height + gap) - offset;
            if tile_y + row_height > y && tile_y < y + height {
                let tile = PoolTile {
                    index,
                    x: tile_x,
                    y: tile_y,
                    width: tile_width,
                    center_x: tile_x + tile_width / 2.0,
                    card_width,
                    card_height,
                    card_scale: TILE_SCALE * zoom,
                    name_size: name_text_size(zoom),
                    name_y: tile_y + card_height + 6.0,
                    name_height,
                };
                draw_tile(self, tile);
            }
        }
        let rows = count.div_ceil(columns) as f32;
        self.end_scroll(scroll_id, 24.0 + rows * (row_height + gap) - gap, &ScrollbarStyle::default());
    }
}

/// A card grid's zoom control: zoom-out button, slider, zoom-in button. The zoom is kept in
/// settings under `settings_key`; while the slider is dragged it is only held here and saved on
/// release, so dragging doesn't rewrite the settings file every frame.
#[derive(Debug, Clone, PartialEq)]
pub struct CardZoom {
    settings_key: &'static str,
    while_dragging: Option<f32>,
}

impl CardZoom {
    pub const MINIMUM: f32 = 0.6;
    pub const MAXIMUM: f32 = 2.5;
    /// The slider's step, and the step of the zoom buttons.
    pub const SLIDER_STEP: f32 = 0.05;
    pub const BUTTON_STEP: f32 = 0.1;
    /// The control's width: two 32-unit buttons around the slider.
    pub const WIDTH: f32 = 220.0;

    pub fn new(settings_key: &'static str) -> CardZoom {
        CardZoom { settings_key, while_dragging: None }
    }

    /// The stored zoom (1 when unset), clamped to the slider's range.
    pub fn saved(&self, settings: &Settings) -> f32 {
        (settings.number(self.settings_key).unwrap_or(1.0) as f32).clamp(Self::MINIMUM, Self::MAXIMUM)
    }

    /// The zoom to draw with this frame.
    pub fn current(&self, settings: &Settings) -> f32 {
        self.while_dragging.unwrap_or_else(|| self.saved(settings))
    }

    fn save(&self, settings: &mut Settings, zoom: f32) {
        // Rounded as f64, so the stored value is exactly the whole percentage (1.2, not 1.2000000476).
        let zoom = (f64::from(zoom.clamp(Self::MINIMUM, Self::MAXIMUM)) * 100.0).round() / 100.0;
        if (zoom - 1.0).abs() < 0.001 {
            settings.remove(self.settings_key);
        } else {
            settings.set(self.settings_key, zoom);
        }
    }

    /// Draws the control in a 44-unit tall row at (`x`, `y`), [`CardZoom::WIDTH`] wide, and
    /// returns the zoom to draw with. Ids are `{id}:out`, `{id}:slider` and `{id}:in`.
    pub fn control(&mut self, frame: &mut Frame, id: &str, x: f32, y: f32) -> f32 {
        let zoom = self.current(frame.settings);
        let (out_id, in_id, slider_id) = (format!("{id}:out"), format!("{id}:in"), format!("{id}:slider"));
        let out_options = IconButtonOptions::tooltip("Smaller cards").id(&out_id).size(32.0, 20.0);
        if frame.icon_button("zoom_out", x, y + 6.0, &out_options) {
            self.save(frame.settings, zoom - Self::BUTTON_STEP);
        }
        let in_options = IconButtonOptions::tooltip("Larger cards").id(&in_id).size(32.0, 20.0);
        if frame.icon_button("zoom_in", x + Self::WIDTH - 32.0, y + 6.0, &in_options) {
            self.save(frame.settings, zoom + Self::BUTTON_STEP);
        }
        let slider_options = SliderOptions { step: Some(Self::SLIDER_STEP), disabled: false };
        let (slider_x, slider_width) = (x + 32.0 + 12.0, Self::WIDTH - 2.0 * (32.0 + 12.0));
        let (new_zoom, held) = frame.slider(
            &slider_id,
            slider_x,
            y + 2.0,
            slider_width,
            zoom,
            Self::MINIMUM,
            Self::MAXIMUM,
            &slider_options,
        );
        if held {
            self.while_dragging = Some(new_zoom);
        } else if let Some(released_at) = self.while_dragging.take() {
            self.save(frame.settings, released_at);
        }
        self.current(frame.settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_is_stored_rounded_and_clamped() {
        let mut settings = Settings::in_memory();
        let zoom = CardZoom::new("test_zoom");
        assert_eq!(zoom.saved(&settings), 1.0);
        zoom.save(&mut settings, 1.4999999);
        assert_eq!(settings.number("test_zoom"), Some(1.5));
        zoom.save(&mut settings, 9.0);
        assert_eq!(zoom.saved(&settings), CardZoom::MAXIMUM);
        zoom.save(&mut settings, 1.0);
        assert!(settings.get("test_zoom").is_none());
    }

    #[test]
    fn names_grow_with_zoom_but_stay_readable() {
        assert_eq!(name_text_size(1.0), NAME_TEXT_SIZE);
        assert_eq!(name_text_size(2.0), 22.0);
        assert_eq!(name_text_size(0.6), SMALLEST_NAME_TEXT_SIZE);
    }
}
