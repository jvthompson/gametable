//! The "Set Scale" dialog for one placed widget (a GameTable addition): a counter or token bag on a
//! game board, a scenario's widget set on the table, or a scenario's image token. Each placement
//! keeps its own scale, so two widgets can be different sizes. The slider changes the widget live
//! so the designer sees it on the preview; Cancel puts the old scale back.

use crate::app::Frame;
use crate::ui::overlays::{DialogAction, DialogResponse, DialogSpec};
use crate::ui::widgets::{ButtonVariant, SliderOptions};

/// The smallest and largest scale a widget can be set to.
pub const MINIMUM_WIDGET_SCALE: f64 = 0.25;
pub const MAXIMUM_WIDGET_SCALE: f64 = 4.0;
/// The slider moves in 5% steps (20 to each 100%).
const SCALE_STEP: f32 = 0.05;
const SCALE_STEPS_PER_WHOLE: f64 = 20.0;

/// What the dialog wants done with the widget's scale this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WidgetScaleResponse {
    /// Still open; the widget should be shown at this scale.
    Open(f64),
    /// Closed; the widget keeps this scale (the new one on Done, the old one on Cancel).
    Closed(f64),
}

/// Rounds a scale to the slider's steps and keeps it in range.
pub fn tidy_widget_scale(scale: f64) -> f64 {
    ((scale * SCALE_STEPS_PER_WHOLE).round() / SCALE_STEPS_PER_WHOLE).clamp(MINIMUM_WIDGET_SCALE, MAXIMUM_WIDGET_SCALE)
}

pub fn scale_percent_text(scale: f64) -> String {
    format!("{}%", (scale * 100.0).round() as i64)
}

pub struct WidgetScaleDialog {
    id: String,
    title: String,
    original_scale: f64,
    scale: f64,
}

impl WidgetScaleDialog {
    /// `widget_name` goes in the title; `current_scale` is the widget's scale now.
    pub fn new(id: impl Into<String>, widget_name: &str, current_scale: f64) -> WidgetScaleDialog {
        WidgetScaleDialog {
            id: id.into(),
            title: format!("Set Scale: {widget_name}"),
            original_scale: current_scale,
            scale: current_scale.clamp(MINIMUM_WIDGET_SCALE, MAXIMUM_WIDGET_SCALE),
        }
    }

    pub fn show(&mut self, frame: &mut Frame) -> WidgetScaleResponse {
        let actions = [
            DialogAction::text("Reset to 100%"),
            DialogAction::text("Cancel"),
            DialogAction::new("Done", ButtonVariant::Filled),
        ];
        let spec = DialogSpec {
            title: Some(&self.title),
            width: 420.0,
            actions: &actions,
            handles_enter: true,
            ..DialogSpec::default()
        };
        let slider_id = format!("{}:slider", self.id);
        let mut scale = self.scale;
        let response = frame.dialog(&self.id, &spec, |frame, x, y, width| {
            let format = |value: f32| scale_percent_text(f64::from(value));
            let options = SliderOptions { step: Some(SCALE_STEP), disabled: false };
            let (value, _) = frame.slider_row(
                &slider_id,
                "Scale",
                x,
                y,
                width,
                scale as f32,
                MINIMUM_WIDGET_SCALE as f32,
                MAXIMUM_WIDGET_SCALE as f32,
                &options,
                Some(&format),
            );
            if (f64::from(value) - scale).abs() > 1e-6 {
                scale = tidy_widget_scale(f64::from(value));
            }
            64.0
        });
        self.scale = scale;
        match response {
            DialogResponse::Open => WidgetScaleResponse::Open(self.scale),
            DialogResponse::Action(0) => {
                self.scale = 1.0;
                WidgetScaleResponse::Open(self.scale)
            }
            DialogResponse::Action(2) | DialogResponse::Enter => WidgetScaleResponse::Closed(self.scale),
            DialogResponse::Action(_) | DialogResponse::Dismissed => WidgetScaleResponse::Closed(self.original_scale),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_snap_to_five_percent_steps_within_range() {
        assert_eq!(tidy_widget_scale(1.234), 1.25);
        assert_eq!(tidy_widget_scale(0.01), MINIMUM_WIDGET_SCALE);
        assert_eq!(tidy_widget_scale(9.0), MAXIMUM_WIDGET_SCALE);
        assert_eq!(scale_percent_text(1.5), "150%");
    }
}
