//! Small composite widgets shared by several screens. Port of `src/ui/kit/widgets.lua`.

use crate::app::Frame;
use crate::graphics::draw_list::{Color, Quad};
use crate::graphics::tessellation::DrawMode;
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::theme::{Weight, fixed, with_alpha};
use crate::ui::widgets::IconButtonOptions;

/// `widgets.editorToolbarHeight`.
pub const EDITOR_TOOLBAR_HEIGHT: f32 = 56.0;
/// `widgets.tabBarHeight`.
pub const TAB_BAR_HEIGHT: f32 = 48.0;
/// `widgets.chipHeight`.
pub const CHIP_HEIGHT: f32 = 32.0;

/// An ARGB colour (`0xAARRGGBB`) as a draw colour.
pub fn argb_color(argb: u32) -> Color {
    Color::from_argb(argb)
}

/// Where an avatar's picture comes from.
#[derive(Debug, Clone, Copy)]
pub enum AvatarSource<'a> {
    /// An image file.
    Path(&'a str),
    /// An in-memory image registered with `ImageCache::put_bytes`.
    Key(&'a str),
}

/// Banner colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerKind {
    Warning,
    Info,
}

/// One action of [`Frame::editor_toolbar`].
#[derive(Debug, Clone, Copy)]
pub struct ToolbarAction<'a> {
    pub icon: &'a str,
    pub tooltip: &'a str,
    pub id: Option<&'a str>,
    pub disabled: bool,
}

/// Options for [`Frame::chip`].
#[derive(Debug, Clone, Copy, Default)]
pub struct ChipOptions<'a> {
    pub id: Option<&'a str>,
    /// Leading icon instead of the check shown while selected.
    pub icon: Option<&'a str>,
    pub tooltip: Option<&'a str>,
}

/// Lays widgets out left to right, wrapping within a width (`widgets.flow`).
#[derive(Debug, Clone, Copy)]
pub struct Flow {
    left: f32,
    width: f32,
    row_height: f32,
    gap: f32,
    x: f32,
    y: f32,
}

impl Flow {
    pub fn new(x: f32, y: f32, width: f32, row_height: f32, gap: f32) -> Flow {
        Flow { left: x, width, row_height, gap, x, y }
    }

    /// The position for the next item of this width.
    pub fn place(&mut self, item_width: f32) -> (f32, f32) {
        if self.x + item_width > self.left + self.width && self.x > self.left {
            self.x = self.left;
            self.y += self.row_height + self.gap;
        }
        let position = (self.x, self.y);
        self.x += item_width + self.gap;
        position
    }

    /// The y below the last row.
    pub fn bottom(&self) -> f32 {
        self.y + self.row_height
    }
}

/// `widgets.moveItem`: moves `list[from]` so it ends up at index `to` (after removal).
pub fn move_item<T>(list: &mut Vec<T>, from: usize, to: usize) {
    let item = list.remove(from);
    list.insert(to.min(list.len()), item);
}

#[derive(Debug, Default)]
struct ReorderState {
    /// (y, height) of each row drawn this frame.
    rows: Vec<(f32, f32)>,
    dragging: Option<usize>,
}

/// Drag-to-reorder for a list drawn immediate-mode (`widgets.reorder`):
/// call [`Reorder::handle`] for each row (it draws the drag handle), then [`Reorder::finish`].
pub struct Reorder {
    id: String,
}

impl Reorder {
    pub fn begin(frame: &mut Frame, id: &str) -> Reorder {
        frame.ui.state::<ReorderState>(id).rows.clear();
        Reorder { id: id.to_string() }
    }

    /// Draws row `index`'s 40×40 handle at (`handle_x`, `handle_y`); the row spans `row_y`..+`row_height`.
    pub fn handle(&self, frame: &mut Frame, index: usize, handle_x: f32, handle_y: f32, row_y: f32, row_height: f32) {
        let state = frame.ui.state::<ReorderState>(&self.id);
        if state.rows.len() <= index {
            state.rows.resize(index + 1, (0.0, 0.0));
        }
        state.rows[index] = (row_y, row_height);
        let dragging = state.dragging;
        let region_id = format!("{}:h{}", self.id, index + 1);
        let options = RegionOptions::cursor("sizeall").with_tooltip(Some("Drag to reorder"));
        let region = frame.ui.region(&region_id, handle_x, handle_y, 40.0, 40.0, options);
        let color = frame.palette().on_surface_variant;
        if region.hovered || dragging == Some(index) {
            frame.fill_rect(handle_x + 4.0, handle_y + 4.0, 32.0, 32.0, 16.0, with_alpha(color, 0.1));
        }
        frame.icon_centered("drag_indicator", handle_x, handle_y, 40.0, 40.0, 24.0, color);
        if region.pressed && frame.ui.mouse_pressed(1) {
            frame.ui.state::<ReorderState>(&self.id).dragging = Some(index);
        }
    }

    /// Draws the drop line while dragging; on release returns `(from, to)` for [`move_item`].
    pub fn finish(&self, frame: &mut Frame, x: f32, width: f32) -> Option<(usize, usize)> {
        let state = frame.ui.state::<ReorderState>(&self.id);
        let from = state.dragging?;
        let rows = state.rows.clone();
        if from >= rows.len() {
            state.dragging = None;
            return None;
        }
        frame.ui.set_cursor("sizeall");
        let (_, mouse_y) = frame.ui.mouse();
        // Insertion slot: before the first row whose middle is below the mouse.
        let slot = rows.iter().position(|(row_y, row_height)| mouse_y < row_y + row_height / 2.0).unwrap_or(rows.len());
        let primary = frame.palette().primary;
        let (dragged_y, dragged_height) = rows[from];
        frame.fill_rect(x, dragged_y, width, dragged_height, 8.0, with_alpha(primary, 0.08));
        let line_y =
            if slot < rows.len() { rows[slot].0 - 4.0 } else { rows[rows.len() - 1].0 + rows[rows.len() - 1].1 + 2.0 };
        frame.fill_rect(x, line_y, width, 3.0, 1.5, primary);
        if !frame.ui.mouse_down(1) {
            frame.ui.state::<ReorderState>(&self.id).dragging = None;
            let to = if slot > from { slot - 1 } else { slot };
            if to != from {
                return Some((from, to));
            }
        }
        None
    }
}

impl Frame<'_> {
    /// `widgets.avatar`: a cover-fit image or a solid colour square with a border in the
    /// player's colour (4 wide from 100 up, else 2).
    pub fn avatar(&mut self, x: f32, y: f32, size: f32, argb: u32, source: Option<AvatarSource>, show_border: bool) {
        let color = argb_color(argb);
        let texture = source.and_then(|source| match source {
            AvatarSource::Path(path) | AvatarSource::Key(path) => self.images.get(path),
        });
        if let Some(texture) = texture {
            let scale = (size / texture.width).max(size / texture.height);
            let side = size / scale;
            let quad =
                Quad { x: (texture.width - side) / 2.0, y: (texture.height - side) / 2.0, width: side, height: side };
            let list = self.ui.list();
            list.set_color(Color::WHITE);
            list.draw_texture(texture, Some(quad), x, y, 0.0, scale, scale, 0.0, 0.0);
        } else {
            self.fill_rect(x, y, size, size, 0.0, color);
        }
        if show_border {
            let border = if size >= 100.0 { 4.0 } else { 2.0 };
            self.outline_rect(x + border / 2.0, y + border / 2.0, size - border, size - border, 0.0, color, border);
        }
    }

    /// `widgets.headerPill`: a right-anchored 48-tall pill with an optional leading glyph drawn
    /// by `draw_icon(frame, x, y)`. Returns the pill's left edge and whether it was clicked.
    #[allow(clippy::too_many_arguments)]
    pub fn header_pill(
        &mut self,
        id: &str,
        right_edge: f32,
        y: f32,
        label: &str,
        tooltip: Option<&str>,
        draw_icon: Option<&mut dyn FnMut(&mut Frame, f32, f32)>,
    ) -> (f32, bool) {
        let palette = self.palette();
        let icon_space = if draw_icon.is_some() { 36.0 } else { 16.0 };
        let pill_width = (self.text_width(label, 14.0, Weight::Medium) + icon_space + 24.0).min(220.0);
        let pill_x = right_edge - pill_width;
        let region = self.ui.region(id, pill_x, y, pill_width, 48.0, RegionOptions::hand().with_tooltip(tooltip));
        if region.hovered {
            self.fill_rect(pill_x, y, pill_width, 48.0, 24.0, with_alpha(palette.on_surface, 0.06));
        }
        let mut text_x = pill_x + 8.0;
        if let Some(draw_icon) = draw_icon {
            draw_icon(self, pill_x + 8.0, y + 8.0);
            text_x = pill_x + 48.0;
        }
        let options = TextOptions::new().middle_of(48.0).medium().width(pill_x + pill_width - 8.0 - text_x).ellipsis();
        self.text(label, text_x, y, options);
        (pill_x, region.clicked)
    }

    /// `widgets.banner`: an orange (warning) or blue (info) message box; returns its height and
    /// whether it was clicked (`clickable` adds hover and the hand cursor).
    pub fn banner(&mut self, text: &str, x: f32, y: f32, width: f32, kind: BannerKind, clickable: bool) -> (f32, bool) {
        let color = if kind == BannerKind::Info { fixed::BLUE } else { fixed::ORANGE };
        let height = self.text_height(text, width - 24.0, 14.0, Weight::Regular) + 24.0;
        self.fill_rect(x, y, width, height, 8.0, with_alpha(color, 0.15));
        self.outline_rect(x + 0.5, y + 0.5, width - 1.0, height - 1.0, 8.0, color, 1.0);
        self.text(text, x + 12.0, y + 12.0, TextOptions::new().width(width - 24.0).color(color));
        let mut clicked = false;
        if clickable {
            let region = self.ui.region(&format!("banner:{text}"), x, y, width, height, RegionOptions::hand());
            if region.hovered {
                self.fill_rect(x, y, width, height, 8.0, with_alpha(color, 0.08));
            }
            clicked = region.clicked;
        }
        (height, clicked)
    }

    /// `widgets.heading`: a bold section heading; returns 20.
    pub fn heading(&mut self, text: &str, x: f32, y: f32, size: Option<f32>) -> f32 {
        let color = self.palette().on_surface;
        self.text(text, x, y, TextOptions::new().bold().size(size.unwrap_or(14.0)).color(color));
        20.0
    }

    /// `widgets.card`: a rounded container (radius 12) with an optional outline.
    #[allow(clippy::too_many_arguments)]
    pub fn card(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        radius: Option<f32>,
        color: Option<Color>,
        border: bool,
    ) {
        let palette = self.palette();
        let radius = radius.unwrap_or(12.0);
        self.fill_rect(x, y, width, height, radius, color.unwrap_or(palette.surface_container));
        if border {
            self.outline_rect(x + 0.5, y + 0.5, width - 1.0, height - 1.0, radius, palette.outline_variant, 1.0);
        }
    }

    /// `widgets.editorToolbar`: a 56-tall in-body toolbar with icon actions on the right. Returns
    /// the index of the action clicked this frame.
    pub fn editor_toolbar(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        title: &str,
        actions: &[ToolbarAction],
    ) -> Option<usize> {
        let palette = self.palette();
        let height = EDITOR_TOOLBAR_HEIGHT;
        self.fill_rect(x, y, width, height, 0.0, palette.surface);
        self.fill_rect(x, y + height, width, 1.0, 0.0, with_alpha(palette.shadow, 0.12));
        let actions_x = x + width - 8.0 - actions.len() as f32 * 48.0;
        let title_options =
            TextOptions::new().size(16.0).medium().middle_of(height).width(actions_x - x - 24.0).ellipsis();
        self.text(title, x + 16.0, y, title_options);
        let mut clicked = None;
        for (index, action) in actions.iter().enumerate() {
            let default_id = format!("tb:{}", action.icon);
            let options = IconButtonOptions {
                tooltip: Some(action.tooltip),
                id: Some(action.id.unwrap_or(&default_id)),
                disabled: action.disabled,
                ..IconButtonOptions::default()
            };
            if self.icon_button(action.icon, actions_x + index as f32 * 48.0 + 4.0, y + 8.0, &options) {
                clicked = Some(index);
            }
        }
        clicked
    }

    /// `widgets.tabBar`: equal-width tabs with an underline under the selected one. Returns the
    /// selected index (0-based).
    pub fn tab_bar(&mut self, id: &str, x: f32, y: f32, width: f32, labels: &[&str], selected: usize) -> usize {
        let palette = self.palette();
        let height = TAB_BAR_HEIGHT;
        let tab_width = width / labels.len().max(1) as f32;
        let mut selected = selected;
        let mut picked = None;
        for (index, label) in labels.iter().enumerate() {
            let tab_x = x + index as f32 * tab_width;
            let region =
                self.ui.region(&format!("{id}:{}", index + 1), tab_x, y, tab_width, height, RegionOptions::hand());
            if region.hovered {
                self.fill_rect(tab_x, y, tab_width, height, 0.0, with_alpha(palette.on_surface, 0.06));
            }
            let is_selected = index == selected;
            let color = if is_selected { palette.primary } else { palette.on_surface_variant };
            let options = TextOptions::new().width(tab_width).middle_of(height).centered().medium().color(color);
            self.text(label, tab_x, y, options);
            if is_selected {
                let label_width = self.text_width(label, 14.0, Weight::Medium);
                self.fill_rect(
                    tab_x + (tab_width - label_width) / 2.0,
                    y + height - 3.0,
                    label_width,
                    3.0,
                    1.5,
                    palette.primary,
                );
            }
            if region.clicked {
                picked = Some(index);
            }
        }
        if let Some(index) = picked {
            selected = index;
        }
        self.fill_rect(x, y + height - 1.0, width, 1.0, 0.0, palette.outline_variant);
        selected
    }

    /// `widgets.chipWidth`.
    pub fn chip_width(&mut self, label: &str, selected: bool, icon: bool) -> f32 {
        let lead = if icon || selected { 26.0 } else { 0.0 };
        self.text_width(label, 14.0, Weight::Medium) + 32.0 + lead
    }

    /// `widgets.chip`: a 32-tall filter chip. Returns (clicked, width).
    pub fn chip(&mut self, label: &str, x: f32, y: f32, selected: bool, options: &ChipOptions) -> (bool, f32) {
        let palette = self.palette();
        let height = CHIP_HEIGHT;
        let width = self.chip_width(label, selected, options.icon.is_some());
        let id = options.id.map_or_else(|| self.ui.default_id(label, x, y), str::to_string);
        let region = self.ui.region(&id, x, y, width, height, RegionOptions::hand().with_tooltip(options.tooltip));
        if selected {
            self.fill_rect(x, y, width, height, 8.0, palette.secondary_container);
        } else {
            self.outline_rect(x + 0.5, y + 0.5, width - 1.0, height - 1.0, 8.0, palette.outline, 1.0);
        }
        if region.hovered {
            self.fill_rect(x, y, width, height, 8.0, with_alpha(palette.on_surface, 0.08));
        }
        let mut text_x = x + 16.0;
        let lead = options.icon.or(selected.then_some("check"));
        if let Some(lead) = lead {
            let color = if selected { palette.on_secondary_container } else { palette.on_surface_variant };
            self.icon(lead, x + 8.0, y + 7.0, 18.0, color);
            text_x = x + 8.0 + 18.0 + 8.0;
        }
        let color = if selected { palette.on_secondary_container } else { palette.on_surface };
        self.text(label, text_x, y, TextOptions::new().middle_of(height).medium().color(color));
        (region.clicked, width)
    }

    /// A filled circle (used by avatars and colour swatches).
    pub fn dot(&mut self, x: f32, y: f32, radius: f32, color: Color) {
        self.circle(DrawMode::Fill, x, y, radius, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_item_matches_table_remove_insert() {
        let mut list = vec!['a', 'b', 'c', 'd'];
        move_item(&mut list, 0, 2);
        assert_eq!(list, ['b', 'c', 'a', 'd']);
        move_item(&mut list, 3, 0);
        assert_eq!(list, ['d', 'b', 'c', 'a']);
    }

    #[test]
    fn flow_wraps_rows() {
        let mut flow = Flow::new(10.0, 0.0, 100.0, 32.0, 8.0);
        assert_eq!(flow.place(60.0), (10.0, 0.0));
        assert_eq!(flow.place(60.0), (10.0, 40.0));
        assert_eq!(flow.place(20.0), (78.0, 40.0));
        assert_eq!(flow.bottom(), 72.0);
        let mut wide = Flow::new(0.0, 0.0, 10.0, 32.0, 8.0);
        assert_eq!(wide.place(50.0), (0.0, 0.0), "a first item wider than the row stays on it");
    }

    #[test]
    fn argb_colors() {
        assert_eq!(argb_color(0xFF00_00FF), Color::rgba(0.0, 0.0, 1.0, 1.0));
    }
}
