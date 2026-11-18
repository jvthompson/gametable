//! The "Title area" part of the Suggest Title dialog: one card's picture, on which the user drags
//! a box around the printed title. The box is kept as fractions of the image and read in the same
//! place on every card in the run, so it suits games whose title moves between card layouts
//! (draw it over a card of the layout you are about to run).

use crate::app::Frame;
use crate::data::title_suggestion::TitleArea;
use crate::graphics::draw_list::Color;
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::theme::Weight;
use crate::ui::widgets::{ButtonOptions, ButtonVariant};

/// The tallest the card picture is drawn.
pub const PICKER_MAX_IMAGE_HEIGHT: f32 = 320.0;

#[derive(Debug, Default)]
pub struct TitleAreaPicker {
    pub area: Option<TitleArea>,
    /// Where the current drag started, as fractions of the image.
    drag_start: Option<(f32, f32)>,
}

impl TitleAreaPicker {
    /// Draws the heading, the hint and the card picture (`image_path`, already resolved) with the
    /// area on it; dragging over the picture draws a new area. Returns the height used.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        frame: &mut Frame,
        id: &str,
        image_path: Option<&str>,
        card_name: &str,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let palette = frame.palette();
        let mut row_y = y;
        frame.text("Title area", x, row_y + 6.0, TextOptions::new().medium());
        if self.area.is_some() {
            let clear_id = format!("{id}:clear");
            let options = ButtonOptions::variant(ButtonVariant::Text).icon("clear").id(&clear_id);
            let clear_width = frame.button_width("Clear", &options);
            if frame.button("Clear", x + width - clear_width + 12.0, row_y, Some(clear_width), Some(32.0), &options) {
                self.area = None;
            }
        }
        row_y += 32.0;
        let hint = if self.area.is_some() {
            format!(
                "Only the boxed part is read, in the same place on every card. Drag again to redraw it. ({card_name})"
            )
        } else {
            format!(
                "Optional: drag a box around the title on this card ({card_name}). It is read in the same place on \
                 every card; without a box the whole card is read."
            )
        };
        let hint_options = TextOptions::new().width(width).size(12.0).color(palette.on_surface_variant);
        frame.text(&hint, x, row_y, hint_options);
        row_y += frame.text_height(&hint, width, 12.0, Weight::Regular) + 8.0;

        let texture = image_path.and_then(|path| frame.images.get(path));
        let Some(texture) = texture else {
            let message =
                if image_path.is_some() { "Loading the card's picture..." } else { "This card has no picture." };
            let box_height = 80.0;
            frame.outline_rect(x + 0.5, row_y + 0.5, width - 1.0, box_height - 1.0, 8.0, palette.outline_variant, 1.0);
            let options =
                TextOptions::new().width(width).centered().middle_of(box_height).color(palette.on_surface_variant);
            frame.text(message, x, row_y, options);
            return row_y + box_height - y;
        };
        let scale = (width / texture.width).min(PICKER_MAX_IMAGE_HEIGHT / texture.height);
        let (image_width, image_height) = (texture.width * scale, texture.height * scale);
        let image_x = x + (width - image_width) / 2.0;
        frame.draw_image(texture, image_x, row_y, scale, 1.0);
        frame.outline_rect(
            image_x + 0.5,
            row_y + 0.5,
            image_width - 1.0,
            image_height - 1.0,
            4.0,
            palette.outline_variant,
            1.0,
        );

        let region = frame.ui.region(
            &format!("{id}:image"),
            image_x,
            row_y,
            image_width,
            image_height,
            RegionOptions::cursor("crosshair"),
        );
        let (mouse_x, mouse_y) = frame.ui.mouse();
        let mouse_fraction = ((mouse_x - image_x) / image_width, (mouse_y - row_y) / image_height);
        if region.pressed {
            self.drag_start = Some(mouse_fraction);
        }
        let shown_area = match self.drag_start {
            Some(start) if region.held => TitleArea::between(start, mouse_fraction),
            Some(start) => {
                // Released: a real drag keeps the new box; a plain click leaves the old one.
                self.drag_start = None;
                if let Some(area) = TitleArea::between(start, mouse_fraction) {
                    self.area = Some(area);
                }
                self.area
            }
            None => self.area,
        };
        if let Some(area) = shown_area {
            draw_area(frame, area, image_x, row_y, image_width, image_height);
        }
        row_y + image_height - y
    }
}

/// Dims the picture outside the area and outlines the area.
fn draw_area(frame: &mut Frame, area: TitleArea, image_x: f32, image_y: f32, image_width: f32, image_height: f32) {
    let (left, top) = (image_x + area.x * image_width, image_y + area.y * image_height);
    let (width, height) = (area.width * image_width, area.height * image_height);
    let (right, bottom) = (left + width, top + height);
    let shade = Color::rgba(0.0, 0.0, 0.0, 0.45);
    frame.fill_rect(image_x, image_y, image_width, top - image_y, 0.0, shade);
    frame.fill_rect(image_x, bottom, image_width, image_y + image_height - bottom, 0.0, shade);
    frame.fill_rect(image_x, top, left - image_x, height, 0.0, shade);
    frame.fill_rect(right, top, image_x + image_width - right, height, 0.0, shade);
    // Neon cyan stands out on any card art, in either theme.
    frame.outline_rect(left, top, width, height, 2.0, Color::from_argb(0xFF00_E5FF), 2.0);
}
