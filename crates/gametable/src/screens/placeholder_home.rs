//! A stand-in Home screen for the Phase 3 shell: the logo and a status line. The real Home
//! screen (`src/ui/screens/home.lua`) is ported in Phase 5.

use crate::app::{Frame, Screen, snap};
use crate::graphics::draw_list::Color;
use crate::ui::theme::{self, Weight};

#[derive(Default)]
pub struct PlaceholderHome {
    message: Option<String>,
}

impl Screen for PlaceholderHome {
    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = *frame.theme.palette();
        let logo_name = if frame.theme.is_dark { "logo.png" } else { "logo_light.png" };
        let logo_path = frame.assets_folder.join(logo_name).to_string_lossy().into_owned();
        let mut logo_bottom = y + height / 2.0;
        if let Some(logo) = frame.images.get(&logo_path) {
            let logo_width = (width * 0.4).min(logo.width).min(height * 0.4 * logo.width / logo.height);
            let logo_scale = logo_width / logo.width;
            let logo_x = x + (width - logo_width) / 2.0;
            let logo_y = y + height / 2.0 - logo.height * logo_scale;
            frame.list.set_color(Color::WHITE);
            frame.list.draw_texture(logo, None, logo_x, logo_y, 0.0, logo_scale, logo_scale, 0.0, 0.0);
            logo_bottom = logo_y + logo.height * logo_scale;
        }
        let font = theme::font(16.0, Weight::Regular);
        let line = self
            .message
            .clone()
            .unwrap_or_else(|| format!("GameTable {} \u{2014} app shell (Phase 3)", env!("CARGO_PKG_VERSION")));
        let line_width = frame.fonts.width(font, &line);
        frame.list.set_color(palette.on_surface_variant);
        let text_x = snap(x + (width - line_width) / 2.0, frame.scale);
        frame.fonts.print(frame.list, font, &line, text_x, snap(logo_bottom + 24.0, frame.scale));
    }

    fn set_message(&mut self, message: Option<String>) {
        self.message = message;
    }
}
