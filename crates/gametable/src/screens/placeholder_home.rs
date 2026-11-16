//! A stand-in Home screen until Phase 5: the logo, a status line and a button to the UI kit
//! gallery. The real Home screen (`src/ui/screens/home.lua`) is ported in Phase 5.

use crate::app::{Frame, Screen, ScreenCommand};
use crate::graphics::draw_list::Color;
use crate::graphics::text_layout::Align;
use crate::screens::kit_gallery::KitGallery;
use crate::ui::draw::TextOptions;
use crate::ui::widgets::{ButtonOptions, ButtonVariant};

#[derive(Default)]
pub struct PlaceholderHome {
    message: Option<String>,
}

impl Screen for PlaceholderHome {
    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let logo_name = if frame.theme.is_dark { "logo.png" } else { "logo_light.png" };
        let logo_path = frame.assets_folder.join(logo_name).to_string_lossy().into_owned();
        let mut logo_bottom = y + height / 2.0;
        if let Some(logo) = frame.images.get(&logo_path) {
            let logo_width = (width * 0.4).min(logo.width).min(height * 0.4 * logo.width / logo.height);
            let logo_scale = logo_width / logo.width;
            let logo_x = x + (width - logo_width) / 2.0;
            let logo_y = y + height / 2.0 - logo.height * logo_scale;
            let list = frame.ui.list();
            list.set_color(Color::WHITE);
            list.draw_texture(logo, None, logo_x, logo_y, 0.0, logo_scale, logo_scale, 0.0, 0.0);
            logo_bottom = logo_y + logo.height * logo_scale;
        }
        let line = self
            .message
            .clone()
            .unwrap_or_else(|| format!("GameTable {} \u{2014} UI kit (Phase 4)", env!("CARGO_PKG_VERSION")));
        let line_options =
            TextOptions::new().size(16.0).width(width).align(Align::Center).color(palette.on_surface_variant);
        frame.text(&line, x, logo_bottom + 24.0, line_options);
        let options = ButtonOptions::variant(ButtonVariant::Tonal).icon("widgets");
        let button_width = frame.button_width("UI Kit Gallery", &options);
        let button_x = x + (width - button_width) / 2.0;
        if frame.button("UI Kit Gallery", button_x, logo_bottom + 64.0, Some(button_width), None, &options) {
            frame.commands.push(ScreenCommand::Push(Box::new(KitGallery::default())));
        }
    }

    fn set_message(&mut self, message: Option<String>) {
        self.message = message;
    }
}
