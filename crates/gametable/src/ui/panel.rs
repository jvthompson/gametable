//! Full-panel chrome (`src/ui/kit/panel.lua`): a scrim that closes on click, a dark rounded
//! panel and a title row with a close button. Used by the table's Game Menu, Log and Card
//! Library panels and by the rulebook viewer. Also the list/detail split
//! (`src/ui/kit/list_detail.lua`) used by the game definition editor's tabs.

use crate::app::Frame;
use crate::graphics::draw_list::Color;
use crate::ui::context::Rect;
use crate::ui::draw::TextOptions;
use crate::ui::theme::fixed;
use crate::ui::widgets::IconButtonOptions;

/// `M.TITLE_BAR_H`: the title row above a panel's content.
pub const PANEL_TITLE_BAR_HEIGHT: f32 = 65.0;

/// A small icon button before the panel title (e.g. a sidebar toggle).
#[derive(Debug, Clone, Copy)]
pub struct PanelLeading<'a> {
    pub icon: &'a str,
    pub tooltip: &'a str,
}

/// What [`Frame::panel`] reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelResponse {
    /// The content area below the title row.
    pub content: Rect,
    /// The close button or the scrim was clicked.
    pub close_requested: bool,
    pub leading_clicked: bool,
}

/// Which side of a list/detail split to draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pane {
    List(Rect),
    Detail(Rect),
}

/// Keeps a list/detail selection valid: an item no longer in the list is dropped, and nothing
/// selected picks the first item.
pub fn clamp_selection<T: PartialEq + Clone>(selected: &mut Option<T>, list: &[T]) {
    if selected.as_ref().is_some_and(|item| !list.contains(item)) {
        *selected = None;
    }
    if selected.is_none() {
        *selected = list.first().cloned();
    }
}

impl Frame<'_> {
    /// `panel.draw(id, w, h, title, onClose, trailing, leading)`: draw in an overlay layer.
    pub fn panel(
        &mut self,
        id: &str,
        width: f32,
        height: f32,
        title: &str,
        trailing: Option<&str>,
        leading: Option<PanelLeading>,
    ) -> PanelResponse {
        let (screen_width, screen_height) = self.ui.screen_size();
        let top = self.ui.content_top;
        self.ui.cover(-1e9, top, 2e9, 2e9);
        self.fill_rect(0.0, top, screen_width, screen_height - top, 0.0, Color::rgba(0.0, 0.0, 0.0, 0.25));
        let x = ((screen_width - width) / 2.0).floor();
        let y = (top + (screen_height - top - height) / 2.0).floor();
        let mut close_requested = self.ui.mouse_pressed(1) && !self.ui.hover_ignoring_layer(x, y, width, height);
        self.fill_rect(x, y, width, height, 12.0, fixed::OVERLAY_PANEL);
        let mut title_x = x + 16.0;
        let mut leading_clicked = false;
        if let Some(leading) = leading {
            let leading_id = format!("{id}:leading");
            let options = IconButtonOptions::tooltip(leading.tooltip).id(&leading_id).color(fixed::WHITE);
            leading_clicked = self.icon_button(leading.icon, x + 8.0, y + 12.0, &options);
            title_x = x + 56.0;
        }
        self.text(title, title_x, y + 12.0, TextOptions::new().size(18.0).bold().color(fixed::WHITE).middle_of(40.0));
        if let Some(trailing) = trailing {
            let options =
                TextOptions::new().width(width - 56.0).middle_of(40.0).right().color(Color::rgba(1.0, 1.0, 1.0, 0.7));
            self.text(trailing, x, y + 12.0, options);
        }
        let close_id = format!("{id}:close");
        if self.icon_button(
            "close",
            x + width - 48.0,
            y + 12.0,
            &IconButtonOptions::tooltip("Close").id(&close_id).color(fixed::WHITE),
        ) {
            close_requested = true;
        }
        self.divider(x, y + 64.0, width, Some(Color::rgba(1.0, 1.0, 1.0, 0.24)));
        PanelResponse {
            content: Rect::new(x, y + PANEL_TITLE_BAR_HEIGHT, width, height - PANEL_TITLE_BAR_HEIGHT),
            close_requested,
            leading_clicked,
        }
    }

    /// `list_detail.draw`: a list pane `min(list_width, 40% of width)` wide, a divider, and the
    /// detail pane, or `empty_message` centred when nothing is selected. `draw_pane` is called
    /// for the list, then (with a selection) the detail, each clipped to its pane.
    #[allow(clippy::too_many_arguments)]
    pub fn list_detail(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        list_width: f32,
        has_selection: bool,
        empty_message: &str,
        mut draw_pane: impl FnMut(&mut Frame, Pane),
    ) {
        let palette = self.palette();
        let list_width = list_width.min((width * 0.4).floor());
        self.ui.push_clip(x, y, list_width, height);
        draw_pane(self, Pane::List(Rect::new(x, y, list_width, height)));
        self.ui.pop_clip();
        self.fill_rect(x + list_width, y, 1.0, height, 0.0, palette.outline_variant);
        let detail = Rect::new(x + list_width + 1.0, y, width - list_width - 1.0, height);
        self.ui.push_clip(detail.x, detail.y, detail.width, detail.height);
        if has_selection {
            draw_pane(self, Pane::Detail(detail));
        } else {
            let options =
                TextOptions::new().width(detail.width).middle_of(height).centered().color(palette.on_surface_variant);
            self.text(empty_message, detail.x, y, options);
        }
        self.ui.pop_clip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_is_kept_valid() {
        let mut selected = Some(5);
        clamp_selection(&mut selected, &[1, 2]);
        assert_eq!(selected, Some(1));
        clamp_selection(&mut selected, &[3, 1]);
        assert_eq!(selected, Some(1));
        clamp_selection(&mut selected, &[]);
        assert_eq!(selected, None);
    }
}
