//! The in-app file and folder picker. Port of `src/ui/screens/file_picker_dialog.lua` (and the
//! `src/platform/file_dialog.lua` entry points that route to it).
//!
//! The original replaced the native dialog with this one because the native Windows dialog has a
//! z-order bug with a maximized borderless window; the port keeps the in-app picker so it looks
//! and behaves the same. The screen that opened it shows it each frame until it reports a result.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use crate::app::Frame;
use crate::data::file_system::{FolderEntry, join_path, list_folder, lowercase_extension, parent_folder};
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::overlays::{DialogAction, DialogResponse, DialogSpec};
use crate::ui::text_field::{TextFieldOptions, text_field_height};
use crate::ui::theme::with_alpha;
use crate::ui::widgets::{IconButtonOptions, ScrollbarStyle, ToggleOptions};

const ROW_HEIGHT: f32 = 40.0;
const LIST_HEIGHT: f32 = 360.0;
const DIALOG_WIDTH: f32 = 560.0;

/// One entry of a file-type filter (`{ name =, extensions = {...} }`).
#[derive(Debug, Clone, PartialEq)]
pub struct FileFilter {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
}

/// `D.imageFilters`.
pub const IMAGE_FILTERS: &[FileFilter] = &[FileFilter { name: "Image", extensions: &["jpg", "jpeg", "png", "webp"] }];
/// `D.jsonFilters`.
pub const JSON_FILTERS: &[FileFilter] = &[FileFilter { name: "JSON", extensions: &["json"] }];

/// What the picker chooses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerMode {
    File,
    /// Files with checkboxes and a "Select N Files" button.
    MultipleFiles,
    Folder,
}

/// What the picker reports each frame.
#[derive(Debug, Clone, PartialEq)]
pub enum FilePickerResponse {
    Open,
    /// One absolute path, or (multiple mode) several sorted paths, never none.
    Picked(Vec<String>),
    Cancelled,
}

pub struct FilePicker {
    mode: PickerMode,
    title: String,
    /// Lowercase extensions with their dot; `None` shows every file.
    extensions: Option<HashSet<String>>,
    folder: String,
    path_text: String,
    error: Option<String>,
    entries: Vec<FolderEntry>,
    selected: BTreeSet<String>,
}

/// Folders first (always shown, so the user can keep navigating), then files that pass the
/// filter; no files at all when picking a folder.
fn list_entries(folder: &str, extensions: Option<&HashSet<String>>, folders_only: bool) -> Option<Vec<FolderEntry>> {
    let items = list_folder(Path::new(folder))?;
    let (mut folders, files): (Vec<FolderEntry>, Vec<FolderEntry>) = items.into_iter().partition(|item| item.is_folder);
    let files = files.into_iter().filter(|file| {
        !folders_only && extensions.is_none_or(|extensions| extensions.contains(&lowercase_extension(&file.name)))
    });
    folders.extend(files);
    Some(folders)
}

impl FilePicker {
    /// Where browsing starts by default: the game library when it exists (most browsing is for
    /// game assets), else the home folder, else the working folder.
    pub fn default_folder(frame: &Frame) -> String {
        let library = frame.paths.games_folder(frame.settings);
        if library.is_dir() {
            return library.to_string_lossy().into_owned();
        }
        let home = std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).ok();
        if let Some(home) = home.filter(|home| Path::new(home).is_dir()) {
            return home;
        }
        std::env::current_dir().map(|folder| folder.to_string_lossy().into_owned()).unwrap_or_else(|_| ".".into())
    }

    /// Opens the picker. `filters` is ignored when picking a folder.
    pub fn new(
        frame: &Frame,
        mode: PickerMode,
        title: Option<&str>,
        filters: Option<&[FileFilter]>,
        initial_folder: Option<&str>,
    ) -> FilePicker {
        let extensions = filters.filter(|_| mode != PickerMode::Folder).map(|filters| {
            filters
                .iter()
                .flat_map(|filter| filter.extensions.iter().map(|extension| format!(".{}", extension.to_lowercase())))
                .collect()
        });
        let default_title = if mode == PickerMode::Folder { "Choose a Folder" } else { "Choose a File" };
        let start = initial_folder
            .filter(|folder| Path::new(folder).is_dir())
            .map(str::to_string)
            .unwrap_or_else(|| FilePicker::default_folder(frame));
        let mut picker = FilePicker {
            mode,
            title: title.unwrap_or(default_title).to_string(),
            extensions,
            folder: start.clone(),
            path_text: start.clone(),
            error: None,
            entries: Vec::new(),
            selected: BTreeSet::new(),
        };
        if let Some(entries) = list_entries(&start, picker.extensions.as_ref(), mode == PickerMode::Folder) {
            picker.entries = entries;
        }
        picker
    }

    /// Lists a folder; a toast when it can't be read.
    fn navigate(&mut self, frame: &mut Frame, folder: String) {
        match list_entries(&folder, self.extensions.as_ref(), self.mode == PickerMode::Folder) {
            Some(entries) => {
                self.entries = entries;
                self.path_text = folder.clone();
                self.folder = folder;
                self.error = None;
                self.selected.clear();
            }
            None => frame.ui.toast("Can't open that folder.", Some(2.0)),
        }
    }

    pub fn show(&mut self, frame: &mut Frame) -> FilePickerResponse {
        let selected_count = self.selected.len();
        let select_label = if selected_count > 0 {
            format!("Select {selected_count} File{}", if selected_count == 1 { "" } else { "s" })
        } else {
            "Select".to_string()
        };
        let actions: Vec<DialogAction> = match self.mode {
            PickerMode::Folder => vec![DialogAction::text("Cancel"), DialogAction::filled("Select This Folder")],
            PickerMode::MultipleFiles => {
                vec![DialogAction::text("Cancel"), DialogAction::filled(&select_label).disabled(selected_count == 0)]
            }
            PickerMode::File => vec![DialogAction::text("Cancel")],
        };
        let title = self.title.clone();
        let spec = DialogSpec { title: Some(&title), width: DIALOG_WIDTH, actions: &actions, ..DialogSpec::default() };
        let mut navigate_to: Option<String> = None;
        let mut picked_file: Option<String> = None;
        let response = frame.dialog("filepicker", &spec, |frame, x, y, width| {
            let palette = frame.palette();
            let mut row_y = y;
            if frame.icon_button(
                "arrow_upward",
                x,
                row_y,
                &IconButtonOptions::tooltip("Up one level").id("filepicker:up"),
            ) {
                navigate_to = Some(parent_folder(&self.folder));
            }
            if frame.icon_button(
                "refresh",
                x + 44.0,
                row_y,
                &IconButtonOptions::tooltip("Refresh").id("filepicker:refresh"),
            ) {
                navigate_to = Some(self.folder.clone());
            }
            let field_options =
                TextFieldOptions { height: Some(40.0), error: self.error.as_deref(), ..TextFieldOptions::default() };
            let field =
                frame.text_field("filepicker:path", x + 92.0, row_y, width - 92.0, &mut self.path_text, &field_options);
            if field.submitted {
                if Path::new(&self.path_text).is_dir() {
                    navigate_to = Some(self.path_text.clone());
                } else {
                    self.error = Some("Not a folder.".to_string());
                }
            }
            row_y += text_field_height(Some(40.0), self.error.as_deref()) + 8.0;
            frame.divider(x, row_y, width, None);
            row_y += 8.0;

            let list_top = row_y;
            let offset = frame.begin_scroll("filepicker:list", x, list_top, width, LIST_HEIGHT);
            let mut item_y = list_top - offset;
            let multiple = self.mode == PickerMode::MultipleFiles;
            for (index, entry) in self.entries.iter().enumerate() {
                let region = frame.ui.region(
                    &format!("filepicker:item{}", index + 1),
                    x,
                    item_y,
                    width,
                    ROW_HEIGHT,
                    RegionOptions::hand(),
                );
                if region.hovered {
                    frame.fill_rect(x, item_y, width, ROW_HEIGHT, 0.0, with_alpha(palette.on_surface, 0.08));
                }
                let mut icon_x = x + 8.0;
                let full_path = join_path(&[&self.folder, &entry.name]);
                if multiple && !entry.is_folder {
                    let was_selected = self.selected.contains(&full_path);
                    let checkbox_id = format!("filepicker:cb{}", index + 1);
                    let checkbox_y = item_y + (ROW_HEIGHT - 18.0) / 2.0;
                    let checked =
                        frame.checkbox(&checkbox_id, x + 8.0, checkbox_y, was_selected, &ToggleOptions::default());
                    if region.clicked || checked != was_selected {
                        if was_selected {
                            self.selected.remove(&full_path);
                        } else {
                            self.selected.insert(full_path.clone());
                        }
                    }
                    icon_x = x + 40.0;
                }
                let icon = if entry.is_folder { "folder" } else { "insert_drive_file" };
                frame.icon(icon, icon_x, item_y + (ROW_HEIGHT - 20.0) / 2.0, 20.0, palette.on_surface_variant);
                let name_options =
                    TextOptions::new().middle_of(ROW_HEIGHT).width(x + width - (icon_x + 32.0) - 16.0).ellipsis();
                frame.text(&entry.name, icon_x + 32.0, item_y, name_options);
                if region.clicked && entry.is_folder {
                    navigate_to = Some(full_path);
                } else if region.clicked && !multiple {
                    picked_file = Some(full_path);
                }
                item_y += ROW_HEIGHT;
            }
            frame.end_scroll("filepicker:list", self.entries.len() as f32 * ROW_HEIGHT, &ScrollbarStyle::default());
            (list_top - y) + LIST_HEIGHT
        });
        if let Some(path) = picked_file {
            return FilePickerResponse::Picked(vec![path]);
        }
        if let Some(folder) = navigate_to {
            self.navigate(frame, folder);
        }
        match (response, self.mode) {
            (DialogResponse::Open | DialogResponse::Enter, _) => FilePickerResponse::Open,
            (DialogResponse::Action(1), PickerMode::Folder) => FilePickerResponse::Picked(vec![self.folder.clone()]),
            (DialogResponse::Action(1), PickerMode::MultipleFiles) => {
                FilePickerResponse::Picked(self.selected.iter().cloned().collect())
            }
            _ => FilePickerResponse::Cancelled,
        }
    }
}
