//! Port of `src/ui/kit/colorpicker.lua` (`color_picker_field.dart`): a hex field and a preview
//! button that opens a saturation/value square + hue bar dialog.

use gametable_core::color::{
    ArgbColor, color_from_hex, from_rgba_channels, hex_from_color, hsv_to_rgb, rgb_to_hsv, to_rgba_channels,
};

use crate::app::Frame;
use crate::graphics::draw_list::Color;
use crate::graphics::tessellation::DrawMode;
use crate::ui::components::argb_color;
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::overlays::{DialogAction, DialogResponse, DialogSpec};
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::{fixed, is_light};

/// Hue, saturation and value, each 0..1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsv {
    pub hue: f64,
    pub saturation: f64,
    pub value: f64,
}

impl Hsv {
    pub fn from_argb(argb: ArgbColor) -> Hsv {
        let channels = to_rgba_channels(argb, 1.0);
        let (hue, saturation, value) = rgb_to_hsv(channels.red, channels.green, channels.blue);
        Hsv { hue, saturation, value }
    }

    /// Opaque ARGB.
    pub fn to_argb(self) -> ArgbColor {
        let (red, green, blue) = hsv_to_rgb(self.hue, self.saturation, self.value);
        from_rgba_channels(red, green, blue, 1.0)
    }
}

fn hsv_color(hue: f64, saturation: f64, value: f64) -> Color {
    let (red, green, blue) = hsv_to_rgb(hue, saturation, value);
    Color::rgba(red as f32, green as f32, blue as f32, 1.0)
}

/// What the picker dialog reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PickerResponse {
    Open,
    Picked(ArgbColor),
    Cancelled,
}

#[derive(Debug, Default)]
struct ColorFieldState {
    hex: String,
    last_color: Option<ArgbColor>,
    /// The picker dialog is open with this colour.
    picker: Option<Hsv>,
}

impl Frame<'_> {
    /// The full picker dialog (`openPickerDialog`) for `state`; "Select" reports the colour.
    pub fn color_picker_dialog(&mut self, id: &str, state: &mut Hsv) -> PickerResponse {
        let actions = [DialogAction::text("Cancel"), DialogAction::filled("Select")];
        let spec = DialogSpec { title: Some("Pick a Color"), width: 360.0, actions: &actions, ..DialogSpec::default() };
        let response = self.dialog(id, &spec, |frame, x, y, width| {
            let box_height = 200.0;
            // Saturation/value square in 24×24 cells.
            let steps = 24;
            let list = frame.ui.list();
            for column in 0..steps {
                let saturation = column as f64 / steps as f64;
                for row in 0..steps {
                    let value = 1.0 - row as f64 / steps as f64;
                    let half_step = 0.5 / steps as f64;
                    list.set_color(hsv_color(state.hue, saturation + half_step, value - half_step));
                    let cell_width = width / steps as f32;
                    let cell_height = box_height / steps as f32;
                    list.rectangle(
                        DrawMode::Fill,
                        x + column as f32 * cell_width,
                        y + row as f32 * cell_height,
                        cell_width + 1.0,
                        cell_height + 1.0,
                        0.0,
                        0.0,
                    );
                }
            }
            let square =
                frame.ui.region(&format!("{id}:sv"), x, y, width, box_height, RegionOptions::cursor("crosshair"));
            if square.held {
                let (mouse_x, mouse_y) = frame.ui.mouse();
                state.saturation = f64::from(((mouse_x - x) / width).clamp(0.0, 1.0));
                state.value = 1.0 - f64::from(((mouse_y - y) / box_height).clamp(0.0, 1.0));
            }
            let marker_x = x + state.saturation as f32 * width;
            let marker_y = y + (1.0 - state.value as f32) * box_height;
            frame.outline_circle(marker_x, marker_y, 7.0, Color::WHITE, 2.0);

            // Hue bar.
            let hue_y = y + box_height + 16.0;
            let bar_height = 16.0;
            for index in 0..60 {
                let color = hsv_color(index as f64 / 60.0, 1.0, 1.0);
                frame.fill_rect(x + index as f32 * width / 60.0, hue_y, width / 60.0 + 1.0, bar_height, 0.0, color);
            }
            let hue_bar =
                frame.ui.region(&format!("{id}:hue"), x, hue_y - 4.0, width, bar_height + 8.0, RegionOptions::hand());
            if hue_bar.held {
                let (mouse_x, _) = frame.ui.mouse();
                state.hue = f64::from(((mouse_x - x) / width).clamp(0.0, 0.999));
            }
            let hue_x = x + state.hue as f32 * width;
            frame.outline_rect(hue_x - 3.0, hue_y - 3.0, 6.0, bar_height + 6.0, 2.0, Color::WHITE, 2.0);

            // Preview.
            let preview_y = hue_y + bar_height + 16.0;
            frame.fill_rect(x, preview_y, 48.0, 32.0, 4.0, hsv_color(state.hue, state.saturation, state.value));
            let hex = format!("#{}", hex_from_color(state.to_argb()));
            frame.text(&hex, x + 60.0, preview_y, TextOptions::new().middle_of(32.0));
            preview_y + 32.0 - y
        });
        match response {
            DialogResponse::Open | DialogResponse::Enter => PickerResponse::Open,
            DialogResponse::Action(1) => PickerResponse::Picked(state.to_argb()),
            _ => PickerResponse::Cancelled,
        }
    }

    /// `colorpicker.field(id, x, y, w, selected, opts)`: returns the colour and the height used
    /// (48). `label` defaults to "Hex".
    pub fn color_field(
        &mut self,
        id: &str,
        x: f32,
        y: f32,
        width: f32,
        selected: ArgbColor,
        label: Option<&str>,
        label_background: Option<Color>,
    ) -> (ArgbColor, f32) {
        let mut selected = selected;
        let state = self.ui.state::<ColorFieldState>(id);
        // Keep the hex text in sync when the colour changes from elsewhere.
        if state.last_color != Some(selected) {
            state.hex = hex_from_color(selected);
            state.last_color = Some(selected);
        }
        let mut hex = std::mem::take(&mut state.hex);
        let options = TextFieldOptions {
            label: Some(label.unwrap_or("Hex")),
            prefix: Some("#"),
            max_length: Some(7),
            label_background,
            ..TextFieldOptions::default()
        };
        let response = self.text_field(&format!("{id}:hex"), x, y, width - 48.0, &mut hex, &options);
        if response.changed {
            selected = color_from_hex(&hex).unwrap_or(selected);
        }
        let state = self.ui.state::<ColorFieldState>(id);
        state.hex = hex;
        if response.changed {
            state.last_color = Some(selected);
        }

        let (button_x, button_y) = (x + width - 40.0, y + 4.0);
        let options = RegionOptions::hand().with_tooltip(Some("More colors"));
        let region = self.ui.region(&format!("{id}:full"), button_x, button_y, 40.0, 40.0, options);
        let color = argb_color(selected);
        self.fill_rect(button_x, button_y, 40.0, 40.0, 4.0, color);
        let outline = self.palette().outline;
        self.outline_rect(button_x + 0.5, button_y + 0.5, 39.0, 39.0, 4.0, outline, 1.0);
        let icon_color = if is_light(color) { fixed::BLACK } else { fixed::WHITE };
        self.icon_centered("colorize", button_x, button_y, 40.0, 40.0, 18.0, icon_color);
        if region.clicked {
            self.ui.state::<ColorFieldState>(id).picker = Some(Hsv::from_argb(selected));
        }

        if let Some(mut picker) = self.ui.state::<ColorFieldState>(id).picker {
            let response = self.color_picker_dialog(&format!("{id}:picker"), &mut picker);
            let state = self.ui.state::<ColorFieldState>(id);
            match response {
                PickerResponse::Open => state.picker = Some(picker),
                PickerResponse::Picked(argb) => {
                    state.picker = None;
                    selected = argb;
                }
                PickerResponse::Cancelled => state.picker = None,
            }
        }
        (selected, 48.0)
    }

    /// A circle outline of the given line width.
    pub fn outline_circle(&mut self, x: f32, y: f32, radius: f32, color: Color, line_width: f32) {
        let list = self.ui.list();
        list.set_color(color);
        list.set_line_width(line_width);
        list.circle(DrawMode::Line, x, y, radius, None);
        list.set_line_width(1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_round_trip() {
        let hsv = Hsv::from_argb(0xFF3B_6939);
        assert_eq!(hsv.to_argb(), 0xFF3B_6939);
    }
}
