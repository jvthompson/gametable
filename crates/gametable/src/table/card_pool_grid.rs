//! The "card pool" grid shared by the deck editor, the game definition editor's card view and the
//! server collection and trade screens. Port of `src/ui/table/card_pool_grid.lua`: tiles a third
//! larger than a table card, columns that exactly fill the width, a name below each tile. Callers
//! draw each tile's content; only the layout and scrolling are shared.

use crate::app::Frame;
use crate::table::card_render::{CARD_HEIGHT, CARD_WIDTH};
use crate::ui::theme::{self, Weight};
use crate::ui::widgets::ScrollbarStyle;

pub const NAME_LINES: f32 = 3.0;
pub const TILE_SCALE: f32 = 4.0 / 3.0;
/// The tile's card size.
pub const TILE_CARD_WIDTH: f32 = CARD_WIDTH * TILE_SCALE;
pub const TILE_CARD_HEIGHT: f32 = CARD_HEIGHT * TILE_SCALE;

/// Where one tile goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoolTile {
    pub index: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    /// The tile's horizontal centre: draw the card at (`center_x`, `y + TILE_CARD_HEIGHT / 2`)
    /// and the name at (`x`, `y + TILE_CARD_HEIGHT + 6`, `width`).
    pub center_x: f32,
}

impl Frame<'_> {
    /// `M.nameH()`: room for three lines of 11-point names.
    pub fn card_pool_name_height(&mut self) -> f32 {
        NAME_LINES * self.fonts.height(theme::font(11.0, Weight::Regular))
    }

    /// `M.draw`: a scrollable grid of `count` tiles; `draw_tile` is called for each visible one.
    /// `padding` insets the tiles on both sides; the top inset is always 12.
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
        mut draw_tile: impl FnMut(&mut Frame, PoolTile),
    ) {
        let name_height = self.card_pool_name_height();
        let (max_extent, row_height, gap) = (TILE_CARD_WIDTH + 24.0, TILE_CARD_HEIGHT + 6.0 + name_height + 4.0, 12.0);
        let inner = width - 2.0 * padding;
        let columns = (((inner + gap) / (max_extent + gap)).ceil() as usize).max(1);
        let tile_width = (inner - (columns as f32 - 1.0) * gap) / columns as f32;
        let offset = self.begin_scroll(scroll_id, x, y, width, height);
        for index in 0..count {
            let (column, row) = (index % columns, index / columns);
            let tile_x = x + padding + column as f32 * (tile_width + gap);
            let tile_y = y + 12.0 + row as f32 * (row_height + gap) - offset;
            if tile_y + row_height > y && tile_y < y + height {
                let tile =
                    PoolTile { index, x: tile_x, y: tile_y, width: tile_width, center_x: tile_x + tile_width / 2.0 };
                draw_tile(self, tile);
            }
        }
        let rows = count.div_ceil(columns) as f32;
        self.end_scroll(scroll_id, 24.0 + rows * (row_height + gap) - gap, &ScrollbarStyle::default());
    }
}
