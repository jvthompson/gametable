//! The Edit Player Settings dialog opened from Home's profile. Port of
//! `Home.openEditPlayerSettings` (`home_screen.dart _promptEditPlayerSettings`).

use crate::app::Frame;
use crate::data::profile;
use crate::graphics::draw_list::Color;
use crate::screens::file_picker::{FilePicker, FilePickerResponse, IMAGE_FILTERS, PickerMode};
use crate::ui::components::AvatarSource;
use crate::ui::context::RegionOptions;
use crate::ui::overlays::{DialogAction, DialogResponse, DialogSpec};
use crate::ui::text_field::TextFieldOptions;
use crate::ui::widgets::ButtonOptions;

pub struct EditPlayerSettings {
    name: String,
    color: u32,
    /// What the dialog shows.
    avatar_path: Option<String>,
    avatar_changed: bool,
    /// The picked image to store on Save; `None` with `avatar_changed` removes the avatar.
    new_source: Option<String>,
    file_picker: Option<FilePicker>,
}

impl EditPlayerSettings {
    pub fn new(frame: &Frame) -> EditPlayerSettings {
        EditPlayerSettings {
            name: frame.profile.name.clone(),
            color: frame.profile.color,
            avatar_path: frame.profile.avatar_path.clone(),
            avatar_changed: false,
            new_source: None,
            file_picker: None,
        }
    }

    /// The Save action: name, colour and avatar are applied and stored.
    fn save(&self, frame: &mut Frame) {
        let name = self.name.trim().to_string();
        profile::set_name(frame.settings, &name);
        profile::set_color(frame.settings, f64::from(self.color));
        frame.profile.name = name;
        frame.profile.color = self.color;
        if !self.avatar_changed {
            return;
        }
        let avatar_folder = frame.paths.avatar_folder();
        match &self.new_source {
            Some(source) => match profile::save_avatar(source, &avatar_folder) {
                Ok(saved) => {
                    frame.images.invalidate(&saved);
                    profile::set_avatar_path(frame.settings, Some(&saved));
                    frame.profile.avatar_path = Some(saved);
                }
                Err(error) => frame.ui.toast(&format!("Could not use that image: {error}"), None),
            },
            None => {
                profile::delete_avatar(&avatar_folder);
                if let Some(old) = frame.profile.avatar_path.take() {
                    frame.images.invalidate(&old);
                }
                profile::set_avatar_path(frame.settings, None);
            }
        }
    }

    /// Shows the dialog; false once it closed.
    pub fn show(&mut self, frame: &mut Frame) -> bool {
        let actions = [DialogAction::text("Cancel"), DialogAction::filled("Save")];
        let spec = DialogSpec {
            title: Some("Edit Player Settings"),
            width: 400.0,
            actions: &actions,
            ..DialogSpec::default()
        };
        let mut open_picker = false;
        let response = frame.dialog("editplayer", &spec, |frame, x, y, width| {
            let palette = frame.palette();
            let mut row_y = y;
            let size = 200.0;
            let avatar_x = x + (width - size) / 2.0;
            let region_options = RegionOptions::hand().with_tooltip(Some("Choose an image"));
            let region = frame.ui.region("eps:avatar", avatar_x, row_y, size, size, region_options);
            let source = self.avatar_path.as_deref().map(AvatarSource::Path);
            frame.avatar(avatar_x, row_y, size, self.color, source, true);
            if region.hovered {
                frame.fill_rect(avatar_x, row_y, size, size, 0.0, Color::rgba(1.0, 1.0, 1.0, 0.08));
            }
            if region.clicked {
                open_picker = true;
            }
            row_y += size + 4.0;
            if self.avatar_path.is_some() {
                let options = ButtonOptions::text().icon("delete_outline");
                let button_width = frame.button_width("Remove Image", &options);
                if frame.button(
                    "Remove Image",
                    x + (width - button_width) / 2.0,
                    row_y,
                    Some(button_width),
                    None,
                    &options,
                ) {
                    self.avatar_path = None;
                    self.avatar_changed = true;
                    self.new_source = None;
                }
                row_y += 44.0;
            }
            row_y += 16.0;
            let name_options = TextFieldOptions {
                autofocus: true,
                label_background: Some(palette.surface_container_high),
                ..TextFieldOptions::label("Your name")
            };
            frame.text_field("eps:name", x, row_y, width, &mut self.name, &name_options);
            row_y += 48.0 + 16.0;
            let (color, field_height) = frame.color_field(
                "eps:color",
                x,
                row_y,
                width,
                self.color,
                Some("Player Color"),
                Some(palette.surface_container_high),
            );
            self.color = color;
            row_y += field_height;
            row_y - y
        });
        if open_picker {
            let title = Some("Choose an avatar image");
            self.file_picker = Some(FilePicker::new(frame, PickerMode::File, title, Some(IMAGE_FILTERS), None));
        }
        if let Some(picker) = &mut self.file_picker {
            match picker.show(frame) {
                FilePickerResponse::Open => {}
                FilePickerResponse::Cancelled => self.file_picker = None,
                FilePickerResponse::Picked(mut paths) => {
                    self.file_picker = None;
                    let path = paths.remove(0);
                    self.avatar_path = Some(path.clone());
                    self.avatar_changed = true;
                    self.new_source = Some(path);
                }
            }
        }
        match response {
            DialogResponse::Action(1) => {
                self.save(frame);
                false
            }
            DialogResponse::Open | DialogResponse::Enter => true,
            _ => false,
        }
    }
}
