//! The app Settings dialog, opened from Home and from the in-game menu. Port of
//! `src/ui/screens/settings_dialog.lua`. New settings go here as sections: a heading, then rows.

use crate::app::{
    Frame, TEXT_SCALE_RANGE, TEXT_SCALE_STEP, UI_SCALE_RANGE, UI_SCALE_STEP, set_text_scale_setting,
    set_ui_scale_setting, text_scale_setting, ui_scale_setting,
};
use crate::data::display_settings::{
    HAND_SETTINGS, TableColor, felt_texture_enabled, is_selection_color_default, reset_selection_color,
    selection_color, set_felt_texture_enabled, set_selection_color,
};
use crate::data::profile;
use crate::data::settings::keys;
use crate::data::title_suggestion::claude_vision::ClaudeModel;
use crate::ui::draw::TextOptions;
use crate::ui::overlays::{DialogAction, DialogResponse, DialogSpec};
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::Weight;
use crate::ui::widgets::{BUTTON_HEIGHT, ButtonOptions, Choice, SliderOptions, ToggleOptions};

/// `pct(v)`: a scale as a whole percentage.
fn percent(value: f32) -> String {
    format!("{}%", (value * 100.0 + 0.5).floor())
}

/// The current game's own colours, when opened over a game.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GameColors {
    pub table_color: Option<String>,
    pub playmat_border_color: Option<String>,
    pub current_turn_color: Option<String>,
}

/// The dialog's options (`opts`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingsDialog {
    /// Opened over the game table (which always draws light).
    pub in_game: bool,
    pub game_colors: GameColors,
    /// The UI scale while its slider is dragged: applied on release, because rescaling the
    /// whole app under the mouse mid-drag would move the slider away from it.
    ui_scale_being_dragged: Option<f32>,
}

const SUBTITLE_SIZE: f32 = 12.0;

/// A slider value as a percentage, marking 100% as the default.
fn percent_with_default(value: f32) -> String {
    if (value - 1.0).abs() < 0.001 { "100% (Default)".to_string() } else { percent(value) }
}

impl SettingsDialog {
    pub fn new() -> SettingsDialog {
        SettingsDialog::default()
    }

    fn heading(frame: &mut Frame, text: &str, x: f32, y: f32) -> f32 {
        let color = frame.palette().primary;
        frame.text(text, x, y, TextOptions::new().size(14.0).bold().color(color));
        28.0
    }

    /// A text button 12 units left of the content edge; true when clicked.
    fn reset_button(frame: &mut Frame, label: &str, id: &str, x: f32, y: f32, disabled: bool) -> bool {
        let options = ButtonOptions::text().id(id).disabled(disabled);
        let width = frame.button_width(label, &options);
        frame.button(label, x - 12.0, y, Some(width), None, &options)
    }

    /// One table colour with its reset button; returns the height used.
    #[allow(clippy::too_many_arguments)]
    fn table_color_row(
        frame: &mut Frame,
        color: TableColor,
        id: &str,
        label: &str,
        reset_label: &str,
        game_hex: Option<&str>,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let label_background = Some(frame.palette().surface_container_high);
        let current = color.resolve(frame.settings, game_hex);
        let (picked, field_height) = frame.color_field(id, x, y, width, current, Some(label), label_background);
        color.set(frame.settings, picked, game_hex);
        let mut row_y = y + field_height + 4.0;
        let is_default = color.is_default(frame.settings);
        if Self::reset_button(frame, reset_label, &format!("{id}:reset"), x, row_y, is_default) {
            color.reset(frame.settings);
        }
        row_y += BUTTON_HEIGHT + 16.0;
        row_y - y
    }

    /// "Suggest Title with Claude": the user's Anthropic API key and which model reads the
    /// cards. Returns the height used (including the gap below).
    fn claude_section(frame: &mut Frame, x: f32, y: f32, width: f32) -> f32 {
        let palette = frame.palette();
        let mut row_y = y;
        row_y += Self::heading(frame, "Suggest Title with Claude", x, row_y);
        let mut api_key = frame.settings.text(keys::ANTHROPIC_API_KEY).unwrap_or("").to_string();
        let field_options = TextFieldOptions {
            height: Some(48.0),
            password: true,
            label_background: Some(palette.surface_container_high),
            hint: Some("sk-ant-..."),
            ..TextFieldOptions::label("Claude API key")
        };
        frame.text_field("settings:anthropicKey", x, row_y + 6.0, width, &mut api_key, &field_options);
        let api_key = api_key.trim();
        if api_key != frame.settings.text(keys::ANTHROPIC_API_KEY).unwrap_or("") {
            if api_key.is_empty() {
                frame.settings.remove(keys::ANTHROPIC_API_KEY);
            } else {
                frame.settings.set(keys::ANTHROPIC_API_KEY, api_key);
            }
        }
        row_y += 6.0 + 48.0 + 12.0;
        let current = ClaudeModel::from_setting_value(frame.settings.text(keys::CLAUDE_TITLE_MODEL));
        let choices = [
            Choice::new(ClaudeModel::Haiku, "Haiku 5.5 (cheaper)"),
            Choice::new(ClaudeModel::Opus, "Opus 5.5 (most accurate)"),
        ];
        let picked = frame.segmented("settings:claudeModel", x, row_y, width, &choices, current, None, false);
        if picked != current {
            frame.settings.set(keys::CLAUDE_TITLE_MODEL, picked.setting_value());
        }
        row_y += 40.0 + 8.0;
        let note = "Used by the game definition editor's Suggest Title to read titles from card images. Create a key \
                    at console.anthropic.com; API use is billed to that account, separately from a Claude \
                    subscription. Stored in this computer's settings file.";
        let note_options = TextOptions::new().width(width).size(SUBTITLE_SIZE).color(palette.on_surface_variant);
        frame.text(note, x, row_y, note_options);
        row_y += frame.text_height(note, width, SUBTITLE_SIZE, Weight::Regular) + 24.0;
        row_y - y
    }

    /// Shows the dialog; false once it closed.
    pub fn show(&mut self, frame: &mut Frame) -> bool {
        let actions = [DialogAction::filled("Done")];
        let spec = DialogSpec { title: Some("Settings"), width: 440.0, actions: &actions, ..DialogSpec::default() };
        let response = frame.dialog("settings", &spec, |frame, x, y, width| {
            let palette = frame.palette();
            let label_background = Some(palette.surface_container_high);
            let mut row_y = y;

            row_y += Self::heading(frame, "Display", x, row_y);
            let format_percent = |value: f32| percent_with_default(value);
            let (minimum, maximum) = UI_SCALE_RANGE;
            let shown_ui_scale = self.ui_scale_being_dragged.unwrap_or_else(|| ui_scale_setting(frame.settings));
            let slider_options = SliderOptions { step: Some(UI_SCALE_STEP), disabled: false };
            let (new_ui_scale, held) = frame.slider_row(
                "settings:uiScale",
                "UI scale",
                x,
                row_y,
                width,
                shown_ui_scale,
                minimum,
                maximum,
                &slider_options,
                Some(&format_percent),
            );
            if held {
                self.ui_scale_being_dragged = Some(new_ui_scale);
            } else if let Some(released_at) = self.ui_scale_being_dragged.take() {
                set_ui_scale_setting(frame.settings, released_at);
            }
            row_y += 64.0;

            let (minimum, maximum) = TEXT_SCALE_RANGE;
            let current_text_scale = text_scale_setting(frame.settings);
            let slider_options = SliderOptions { step: Some(TEXT_SCALE_STEP), disabled: false };
            let (new_text_scale, _) = frame.slider_row(
                "settings:textScale",
                "Text size",
                x,
                row_y,
                width,
                current_text_scale,
                minimum,
                maximum,
                &slider_options,
                Some(&format_percent),
            );
            // Applied live (the app picks it up next frame): only text grows, so the slider stays put.
            if (new_text_scale - current_text_scale).abs() > 0.001 {
                set_text_scale_setting(frame.settings, new_text_scale);
            }
            row_y += 64.0;
            let (dpi, effective) = (frame.dpi_scale, frame.scale);
            let mut note =
                format!("Windows display scaling: {}. Effective scale: {}.", percent(dpi), percent(effective));
            if effective + 0.005 < dpi * ui_scale_setting(frame.settings) {
                note.push_str(" Limited so the app still fits on this screen.");
            }
            let note_options = TextOptions::new().width(width).size(12.0).color(palette.on_surface_variant);
            frame.text(&note, x, row_y, note_options);
            row_y += frame.text_height(&note, width, 12.0, Weight::Regular) + 24.0;

            row_y += Self::heading(frame, "Appearance", x, row_y);
            let subtitle = self.in_game.then_some("The game table always uses the light theme.");
            let is_dark = frame.theme.is_dark;
            let options = ToggleOptions { subtitle, ..ToggleOptions::default() };
            if frame.switch_row("settings:dark", "Dark mode", x, row_y, width, is_dark, &options) != is_dark {
                frame.theme.is_dark = !is_dark;
                profile::set_dark_mode(frame.settings, frame.theme.is_dark);
            }
            row_y += 32.0;
            if let Some(subtitle) = subtitle {
                row_y += frame.text_height(subtitle, width - 64.0, SUBTITLE_SIZE, Weight::Regular) + 4.0;
            }
            row_y += 24.0;

            row_y += Self::heading(frame, "Hand", x, row_y);
            for definition in HAND_SETTINGS {
                let setting = definition.setting;
                let value = frame.hand_settings.get(frame.settings, setting) as f32;
                let format = move |value: f32| setting.format_value(f64::from(value));
                let slider_options = SliderOptions { step: Some(definition.step as f32), disabled: false };
                let (new_value, held) = frame.slider_row(
                    &format!("settings:hand:{}", definition.id),
                    definition.label,
                    x,
                    row_y,
                    width,
                    value,
                    definition.min as f32,
                    definition.max as f32,
                    &slider_options,
                    Some(&format),
                );
                // Applied live while dragging; saved once the slider is released.
                frame.hand_settings.set(frame.settings, setting, f64::from(new_value), held);
                row_y += 64.0;
            }
            let hand_is_default = frame.hand_settings.is_default(frame.settings);
            if Self::reset_button(frame, "Reset hand to defaults", "settings:hand:reset", x, row_y, hand_is_default) {
                frame.hand_settings.reset(frame.settings);
            }
            row_y += BUTTON_HEIGHT + 16.0;

            row_y += Self::heading(frame, "Table", x, row_y);
            let felt_texture_on = felt_texture_enabled(frame.settings);
            let felt_subtitle = "Adds a subtle woven-cloth texture to the table.";
            let options = ToggleOptions { subtitle: Some(felt_subtitle), ..ToggleOptions::default() };
            if frame.switch_row("settings:feltTexture", "Felt texture", x, row_y, width, felt_texture_on, &options)
                != felt_texture_on
            {
                set_felt_texture_enabled(frame.settings, !felt_texture_on);
            }
            row_y += 32.0 + frame.text_height(felt_subtitle, width - 64.0, SUBTITLE_SIZE, Weight::Regular) + 4.0 + 24.0;

            let current = selection_color(frame.settings);
            let (picked, field_height) = frame.color_field(
                "settings:selectionColor",
                x,
                row_y,
                width,
                current,
                Some("Selection Color"),
                label_background,
            );
            set_selection_color(frame.settings, picked);
            row_y += field_height + 4.0;
            let selection_is_default = is_selection_color_default(frame.settings);
            let reset_id = "settings:selectionColor:reset";
            if Self::reset_button(frame, "Reset selection color", reset_id, x, row_y, selection_is_default) {
                reset_selection_color(frame.settings);
            }
            row_y += BUTTON_HEIGHT + 16.0;

            let colors = &self.game_colors;
            row_y += Self::table_color_row(
                frame,
                TableColor::Felt,
                "settings:feltColor",
                "Tabletop Color",
                "Reset table color",
                colors.table_color.as_deref(),
                x,
                row_y,
                width,
            );
            row_y += Self::table_color_row(
                frame,
                TableColor::PlaymatBorder,
                "settings:playmatBorderColor",
                "Playmat Border Color",
                "Reset playmat border color",
                colors.playmat_border_color.as_deref(),
                x,
                row_y,
                width,
            );
            row_y += Self::table_color_row(
                frame,
                TableColor::CurrentTurn,
                "settings:currentTurnColor",
                "Current Turn Color",
                "Reset current turn color",
                colors.current_turn_color.as_deref(),
                x,
                row_y,
                width,
            );
            if !self.in_game {
                row_y += 8.0;
                row_y += Self::claude_section(frame, x, row_y, width);
            }
            // The last row has no gap below it.
            row_y - 16.0 - y
        });
        response == DialogResponse::Open
    }
}
