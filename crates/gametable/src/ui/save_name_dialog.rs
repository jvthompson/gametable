//! A name prompt with an "existing items — tap to reuse a name" list and an overwrite
//! confirmation step. Port of `src/ui/kit/save_name_dialog.lua` (shared by the save-game and
//! save-deck dialogs).

use crate::app::Frame;
use crate::data::deck_library::validate_name;
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::overlays::{DialogAction, DialogResponse, DialogSpec};
use crate::ui::text_field::{TextFieldOptions, text_field_height};
use crate::ui::theme::{Weight, with_alpha};
use crate::ui::widgets::ScrollbarStyle;

/// One existing name in the list, with optional right-aligned detail (a date, a count).
#[derive(Debug, Clone, PartialEq)]
pub struct ExistingName {
    pub name: String,
    pub trailing: Option<String>,
}

/// What the dialog reports.
#[derive(Debug, Clone, PartialEq)]
pub enum SaveNameResponse {
    Open,
    /// Save under this (trimmed, validated, overwrite-confirmed) name.
    Save(String),
    Cancelled,
}

/// The dialog's texts and data (`spec`) plus its state.
pub struct SaveNameDialog {
    pub id: String,
    pub title: String,
    pub field_label: String,
    pub empty_name_message: Option<String>,
    /// Default 408.
    pub width: f32,
    pub existing: Vec<ExistingName>,
    pub existing_hint: String,
    pub empty_list_text: String,
    pub overwrite_title: String,
    /// `overwriteMessage(name)`.
    pub overwrite_message: Box<dyn Fn(&str) -> String>,
    /// `exists(name)`: true asks for overwrite confirmation.
    pub exists: Box<dyn Fn(&str) -> bool>,
    /// Height of an extra row the caller draws under the list (`footerH`), 0 for none.
    pub footer_height: f32,
    name: String,
    error: Option<String>,
    /// The name waiting for overwrite confirmation.
    confirming: Option<String>,
}

impl SaveNameDialog {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        field_label: impl Into<String>,
        existing: Vec<ExistingName>,
        existing_hint: impl Into<String>,
        empty_list_text: impl Into<String>,
        overwrite_title: impl Into<String>,
        overwrite_message: Box<dyn Fn(&str) -> String>,
        exists: Box<dyn Fn(&str) -> bool>,
    ) -> SaveNameDialog {
        SaveNameDialog {
            id: id.into(),
            title: title.into(),
            field_label: field_label.into(),
            empty_name_message: None,
            width: 408.0,
            existing,
            existing_hint: existing_hint.into(),
            empty_list_text: empty_list_text.into(),
            overwrite_title: overwrite_title.into(),
            overwrite_message,
            exists,
            footer_height: 0.0,
            name: String::new(),
            error: None,
            confirming: None,
        }
    }

    /// Validates the typed name: an error, a switch to the overwrite step, or the name to save.
    fn submit(&mut self, frame: &mut Frame) -> Option<String> {
        let name = self.name.trim().to_string();
        if let Some(error) = validate_name(&name, self.empty_name_message.as_deref()) {
            self.error = Some(error);
            return None;
        }
        if (self.exists)(&name) {
            self.confirming = Some(name);
            frame.ui.blur();
            return None;
        }
        Some(name)
    }

    /// Shows the dialog. `footer(frame, x, y, width)` draws the optional row under the list.
    pub fn show(
        &mut self,
        frame: &mut Frame,
        footer: Option<&mut dyn FnMut(&mut Frame, f32, f32, f32)>,
    ) -> SaveNameResponse {
        let confirming = self.confirming.clone();
        let primary = if confirming.is_some() { "Overwrite" } else { "Save" };
        let actions = [DialogAction::text("Cancel"), DialogAction::filled(primary)];
        let title = if confirming.is_some() { self.overwrite_title.clone() } else { self.title.clone() };
        let spec = DialogSpec { title: Some(&title), width: self.width, actions: &actions, ..DialogSpec::default() };
        let mut submitted = false;
        let dialog_id = self.id.clone();
        let response = frame.dialog(&dialog_id, &spec, |frame, x, y, width| {
            let palette = frame.palette();
            if let Some(pending) = &confirming {
                let message = (self.overwrite_message)(pending);
                frame.text(&message, x, y, TextOptions::new().width(width).color(palette.on_surface_variant));
                return frame.text_height(&message, width, 14.0, Weight::Regular);
            }
            let mut row_y = y + 8.0;
            let field_options = TextFieldOptions {
                autofocus: true,
                error: self.error.as_deref(),
                label_background: Some(palette.surface_container_high),
                ..TextFieldOptions::label(&self.field_label)
            };
            let field = frame.text_field("savename:field", x, row_y, width, &mut self.name, &field_options);
            if field.changed {
                self.error = None;
            }
            submitted = field.submitted;
            row_y += text_field_height(None, self.error.as_deref()) + 16.0;
            let hint_options = TextOptions::new().size(12.0).color(palette.on_surface_variant);
            frame.text(&self.existing_hint, x, row_y, hint_options);
            row_y += 20.0;
            frame.divider(x, row_y, width, None);
            row_y += 4.0;
            let list_height = 400.0 - self.footer_height - (row_y - y);
            if let Some(footer) = footer {
                footer(frame, x, row_y + list_height + 8.0, width);
            }
            if self.existing.is_empty() {
                let options =
                    TextOptions::new().width(width).middle_of(list_height).centered().color(palette.on_surface_variant);
                frame.text(&self.empty_list_text, x, row_y, options);
            } else {
                let offset = frame.begin_scroll("savename:list", x, row_y, width, list_height);
                let mut item_y = row_y - offset;
                for (index, existing) in self.existing.iter().enumerate() {
                    let item_id = format!("savename:item{}", index + 1);
                    let region = frame.ui.region(&item_id, x, item_y, width, 40.0, RegionOptions::hand());
                    if region.hovered {
                        frame.fill_rect(x, item_y, width, 40.0, 0.0, with_alpha(palette.on_surface, 0.08));
                    }
                    let name_options = TextOptions::new().middle_of(40.0).width(width - 140.0).ellipsis();
                    frame.text(&existing.name, x + 16.0, item_y, name_options);
                    if let Some(trailing) = &existing.trailing {
                        let trailing_options = TextOptions::new()
                            .width(width - 16.0)
                            .middle_of(40.0)
                            .right()
                            .size(12.0)
                            .color(palette.on_surface_variant);
                        frame.text(trailing, x, item_y, trailing_options);
                    }
                    if region.clicked {
                        self.name = existing.name.clone();
                        self.error = None;
                    }
                    item_y += 40.0;
                }
                frame.end_scroll("savename:list", self.existing.len() as f32 * 40.0, &ScrollbarStyle::default());
            }
            400.0
        });
        if submitted && let Some(name) = self.submit(frame) {
            return SaveNameResponse::Save(name);
        }
        match response {
            DialogResponse::Open | DialogResponse::Enter => SaveNameResponse::Open,
            DialogResponse::Action(0) => {
                if self.confirming.take().is_some() {
                    SaveNameResponse::Open
                } else {
                    SaveNameResponse::Cancelled
                }
            }
            DialogResponse::Action(_) => match self.confirming.clone() {
                Some(pending) => SaveNameResponse::Save(pending),
                None => match self.submit(frame) {
                    Some(name) => SaveNameResponse::Save(name),
                    None => SaveNameResponse::Open,
                },
            },
            DialogResponse::Dismissed => SaveNameResponse::Cancelled,
        }
    }
}
