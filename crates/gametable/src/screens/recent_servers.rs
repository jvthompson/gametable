//! The recently joined servers dropdown at the end of an address field, shared by Home's Join
//! panel and the Join screen (`drawRecentServersPopup` / `Join:drawRecentServers`).

use crate::app::Frame;
use crate::data::profile::{self, RecentServer};
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::theme::with_alpha;
use crate::ui::widgets::IconButtonOptions;

/// Draws the arrow button (and the popup while `open`) for a field at (x, y, width). `prefix`
/// keys the widget ids (`home:join:recent`, `join:recent`). Returns the server picked this frame.
pub fn recent_servers_dropdown(
    frame: &mut Frame,
    prefix: &str,
    recent: &[RecentServer],
    x: f32,
    y: f32,
    width: f32,
    open: &mut bool,
) -> Option<RecentServer> {
    let options = IconButtonOptions::tooltip("Recent servers").id(prefix);
    if frame.icon_button("arrow_drop_down", x + width - 40.0, y + 4.0, &options) {
        *open = !*open;
    }
    if !*open {
        return None;
    }
    let item_height = 40.0;
    let (popup_x, popup_y, popup_width) = (x, y + 50.0, width);
    let popup_height = recent.len() as f32 * item_height + 16.0;
    let mut picked = None;
    frame.overlay(|frame| {
        let palette = frame.palette();
        // Swallow clicks outside so they only close the popup.
        frame.ui.cover(-1e9, -1e9, 2e9, 2e9);
        frame.shadow(popup_x, popup_y, popup_width, popup_height, 4.0, 2.0);
        frame.fill_rect(popup_x, popup_y, popup_width, popup_height, 4.0, palette.surface_container);
        let mut item_y = popup_y + 8.0;
        for (index, server) in recent.iter().enumerate() {
            let number = index + 1;
            let region = frame.ui.region(
                &format!("{prefix}:{number}"),
                popup_x,
                item_y,
                popup_width - 40.0,
                item_height,
                RegionOptions::hand(),
            );
            if region.hovered {
                frame.fill_rect(popup_x, item_y, popup_width, item_height, 0.0, with_alpha(palette.on_surface, 0.08));
            }
            let label = format!("{} : {}", server.host, server.port);
            let label_options = TextOptions::new()
                .width(popup_width - 56.0)
                .middle_of(item_height)
                .ellipsis()
                .color(palette.on_surface);
            frame.text(&label, popup_x + 12.0, item_y, label_options);
            let remove_id = format!("{prefix}:rm:{number}");
            let remove_options = IconButtonOptions::tooltip("Remove").id(&remove_id).size(32.0, 32.0 * 0.6);
            if frame.icon_button("close", popup_x + popup_width - 40.0, item_y + 4.0, &remove_options) {
                profile::remove_recent_server(frame.settings, &server.host, server.port);
                if recent.len() == 1 {
                    *open = false;
                }
            } else if region.clicked {
                picked = Some(server.clone());
                *open = false;
            }
            item_y += item_height;
        }
        let clicked_outside =
            frame.ui.mouse_pressed(1) && !frame.ui.hover_ignoring_layer(popup_x, popup_y, popup_width, popup_height);
        if clicked_outside || frame.ui.mouse_pressed(2) {
            *open = false;
        }
    });
    picked
}
