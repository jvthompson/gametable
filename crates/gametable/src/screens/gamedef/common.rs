//! Helpers shared by the game definition editor's tabs. Port of
//! `src/ui/screens/gamedef/common.lua`.
//!
//! The editor works on a game holding BARE image file names; anything that draws an image builds
//! a display-only copy with resolved paths, never stored back.

use std::sync::atomic::{AtomicU64, Ordering};

use gametable_core::models::{CardBackDefinition, CardDefinition, CardOrientation};

use crate::app::Frame;
use crate::data::file_system::join_path;
use crate::data::gamedef_ops::copy_picked_file;
use crate::data::image_resolver::{clear_existence_cache, resolve_bare_image_path};
use crate::graphics::draw_list::{FragmentProgram, Quad};
use crate::graphics::image_cache::ImageStatus;
use crate::screens::file_picker::{FileFilter, FilePicker, FilePickerResponse, IMAGE_FILTERS, PickerMode};
use crate::table::card_render::radius_for;
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::theme::{Weight, with_alpha};
use crate::ui::widgets::{ButtonOptions, ButtonVariant, Choice, ToggleOptions};

/// Page gutter.
pub const PAD: f32 = 24.0;
/// Between fields and rows.
pub const GAP: f32 = 12.0;
/// Between sections.
pub const SECTION_GAP: f32 = 32.0;
pub const FIELD_HEIGHT: f32 = 48.0;
/// Single-column pages are capped and centred.
pub const MAX_CONTENT: f32 = 760.0;
pub const TOGGLE_HEIGHT: f32 = 44.0;

/// `C.documentFilters`.
pub const DOCUMENT_FILTERS: &[FileFilter] = &[FileFilter { name: "Document", extensions: &["pdf", "md"] }];

/// `C.orientationOptions`.
pub fn orientation_choices() -> [Choice<CardOrientation>; 3] {
    [
        Choice::new(CardOrientation::Portrait, "portrait"),
        Choice::new(CardOrientation::Left, "left"),
        Choice::new(CardOrientation::Right, "right"),
    ]
}

/// `C.column(x, w)`: the centred content column of a page.
pub fn column(x: f32, width: f32) -> (f32, f32) {
    let column_width = MAX_CONTENT.min(width - 2.0 * PAD);
    (x + ((width - column_width) / 2.0).floor(), column_width)
}

// ---------------------------------------------------------------------------
// Stable keys
// ---------------------------------------------------------------------------

static NEXT_KEY: AtomicU64 = AtomicU64::new(1);

/// `C.keyOf(obj)` for items kept in a `Vec`: one key per item, following the item through adds,
/// removes and reorders, so widget ids (and their text field state) stay with their item.
#[derive(Debug, Default, Clone)]
pub struct StableKeys {
    keys: Vec<u64>,
}

impl StableKeys {
    fn new_key() -> u64 {
        NEXT_KEY.fetch_add(1, Ordering::Relaxed)
    }

    /// Matches the list's length: new items at the end get new keys.
    pub fn sync(&mut self, length: usize) {
        self.keys.truncate(length);
        while self.keys.len() < length {
            self.keys.push(StableKeys::new_key());
        }
    }

    pub fn key(&self, index: usize) -> u64 {
        self.keys[index]
    }

    pub fn index_of(&self, key: u64) -> Option<usize> {
        self.keys.iter().position(|&known| known == key)
    }

    /// After `move_item(list, from, to)`.
    pub fn moved(&mut self, from: usize, to: usize) {
        crate::ui::components::move_item(&mut self.keys, from, to);
    }

    pub fn removed(&mut self, index: usize) {
        if index < self.keys.len() {
            self.keys.remove(index);
        }
    }

    /// After the list kept only the items at the indexes `keep` accepts.
    pub fn retain(&mut self, mut keep: impl FnMut(usize) -> bool) {
        let mut index = 0;
        self.keys.retain(|_| {
            let kept = keep(index);
            index += 1;
            kept
        });
    }
}

/// Text typed into a field that maps onto a model value (a number, a deck type), kept between
/// frames so a half-typed value isn't clobbered (`ui.state(k .. ":text")`).
#[derive(Debug, Default)]
pub struct TypedText(pub Option<String>);

// ---------------------------------------------------------------------------
// Display copies
// ---------------------------------------------------------------------------

/// `C.displayCard`: a copy with its image path resolved for drawing.
pub fn display_card(folder_path: &str, card: &CardDefinition) -> CardDefinition {
    let mut copy = card.clone();
    if let Some(bare) = &card.image_path {
        copy.image_path = Some(resolve_bare_image_path(folder_path, bare, card.set_id.as_deref()));
    }
    copy
}

/// `C.displayCardBacks`.
pub fn display_card_backs(folder_path: &str, card_backs: &[CardBackDefinition]) -> Vec<CardBackDefinition> {
    card_backs
        .iter()
        .map(|back| CardBackDefinition {
            image_path: back.image_path.as_deref().map(|bare| resolve_bare_image_path(folder_path, bare, None)),
            ..back.clone()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Picking files
// ---------------------------------------------------------------------------

/// `C.chooseImage` / `C.chooseDocument`: picks a file and copies it into a folder.
pub struct FileImport {
    picker: FilePicker,
    destination: String,
    correct_card_orientation: bool,
    is_document: bool,
}

/// What a [`FileImport`] reports.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportResponse {
    Open,
    /// The copied file's bare name.
    Imported(String),
    /// Cancelled, or the copy failed (with a toast).
    Cancelled,
}

impl FileImport {
    /// An image, optionally turned portrait when it is landscape card art.
    pub fn image(frame: &Frame, destination: String, correct_card_orientation: bool) -> FileImport {
        FileImport {
            picker: FilePicker::new(frame, PickerMode::File, Some("Choose Image"), Some(IMAGE_FILTERS), None),
            destination,
            correct_card_orientation,
            is_document: false,
        }
    }

    /// A PDF or Markdown document.
    pub fn document(frame: &Frame, destination: String) -> FileImport {
        FileImport {
            picker: FilePicker::new(frame, PickerMode::File, Some("Choose Document"), Some(DOCUMENT_FILTERS), None),
            destination,
            correct_card_orientation: false,
            is_document: true,
        }
    }

    pub fn show(&mut self, frame: &mut Frame) -> ImportResponse {
        match self.picker.show(frame) {
            FilePickerResponse::Open => ImportResponse::Open,
            FilePickerResponse::Cancelled => ImportResponse::Cancelled,
            FilePickerResponse::Picked(mut paths) => {
                let source = paths.remove(0);
                match copy_picked_file(&source, &self.destination, self.correct_card_orientation) {
                    Ok(file_name) => {
                        if !self.is_document {
                            // The copy may have replaced a file the caches already hold.
                            frame.images.invalidate(&join_path(&[&self.destination, &file_name]));
                            clear_existence_cache();
                        }
                        ImportResponse::Imported(file_name)
                    }
                    Err(error) => {
                        let what = if self.is_document { "file" } else { "image" };
                        frame.ui.toast(&format!("Could not copy that {what}: {error}"), None);
                        ImportResponse::Cancelled
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Widgets
// ---------------------------------------------------------------------------

/// A section header's right-aligned button.
#[derive(Debug, Clone, Copy)]
pub struct SectionAction<'a> {
    pub label: &'a str,
    pub icon: Option<&'a str>,
    pub id: &'a str,
    pub disabled: bool,
}

/// One on/off row of a [`toggle_grid`].
pub struct ToggleRow<'a> {
    pub id: String,
    pub label: &'a str,
    pub help: Option<&'a str>,
    pub value: &'a mut bool,
    pub disabled: bool,
}

impl Frame<'_> {
    /// `C.infoIcon`: a small (i) whose tooltip holds the help text. Returns its width.
    pub fn info_icon(&mut self, id: &str, x: f32, y: f32, text: &str) -> f32 {
        let size = 18.0;
        let options = RegionOptions { cursor: None, tooltip: Some(text) };
        let region = self.ui.region(id, x - 3.0, y - 3.0, size + 6.0, size + 6.0, options);
        let palette = self.palette();
        let color = if region.hovered { palette.primary } else { palette.outline };
        self.icon("info_outline", x, y, size, color);
        size
    }

    /// `C.sectionHeader`: a 16-medium title with an optional count pill, (i) tooltip and
    /// right-aligned tonal button. Returns (height, button clicked).
    #[allow(clippy::too_many_arguments)]
    pub fn section_header(
        &mut self,
        id: &str,
        title: &str,
        x: f32,
        y: f32,
        width: f32,
        count: Option<usize>,
        info: Option<&str>,
        action: Option<SectionAction>,
    ) -> (f32, bool) {
        let palette = self.palette();
        let height = 40.0;
        let mut text_x = x;
        self.text(title, text_x, y, TextOptions::new().size(16.0).medium().middle_of(height));
        text_x += self.text_width(title, 16.0, Weight::Medium) + 8.0;
        if let Some(count) = count {
            let label = count.to_string();
            let pill_width = self.text_width(&label, 12.0, Weight::Medium) + 14.0;
            self.fill_rect(
                text_x,
                y + (height - 20.0) / 2.0,
                pill_width,
                20.0,
                10.0,
                palette.surface_container_highest,
            );
            let options = TextOptions::new()
                .width(pill_width)
                .middle_of(height)
                .size(12.0)
                .medium()
                .centered()
                .color(palette.on_surface_variant);
            self.text(&label, text_x, y, options);
            text_x += pill_width + 8.0;
        }
        if let Some(info) = info {
            self.info_icon(&format!("{id}:info"), text_x, y + (height - 18.0) / 2.0, info);
        }
        let mut clicked = false;
        if let Some(action) = action {
            let options = ButtonOptions {
                variant: ButtonVariant::Tonal,
                icon: action.icon,
                id: Some(action.id),
                disabled: action.disabled,
                ..ButtonOptions::default()
            };
            let button_width = self.button_width(action.label, &options);
            clicked =
                self.button(action.label, x + width - button_width, y, Some(button_width), Some(height), &options);
        }
        (height, clicked)
    }

    /// `C.groupLabel`: a small primary-coloured label. Returns its height.
    pub fn group_label(&mut self, text: &str, x: f32, y: f32, width: f32) -> f32 {
        let color = self.palette().primary;
        self.text(text, x, y, TextOptions::new().size(12.0).medium().width(width).color(color));
        20.0
    }

    /// The zone tabs' `group`: a group label with a little space above. Returns the height.
    pub fn spaced_group_label(&mut self, text: &str, x: f32, y: f32, width: f32) -> f32 {
        20.0 + self.group_label(text, x, y + 20.0, width) + 2.0
    }

    /// `C.toggleRow`: label, optional (i), switch at the right. Returns (value, height).
    #[allow(clippy::too_many_arguments)]
    pub fn toggle_row(
        &mut self,
        id: &str,
        label: &str,
        help: Option<&str>,
        x: f32,
        y: f32,
        width: f32,
        value: bool,
        disabled: bool,
    ) -> (bool, f32) {
        let palette = self.palette();
        let height = TOGGLE_HEIGHT;
        let help_room = if help.is_some() { 26.0 } else { 0.0 };
        let label_width = self.text_width(label, 14.0, Weight::Regular).min(width - 52.0 - 16.0 - help_room);
        let color = if disabled { palette.disabled } else { palette.on_surface };
        let options = TextOptions::new().width(label_width + 1.0).middle_of(height).ellipsis().color(color);
        self.text(label, x, y, options);
        if let Some(help) = help {
            self.info_icon(&format!("{id}:info"), x + label_width + 8.0, y + (height - 18.0) / 2.0, help);
        }
        let value = self.switch(
            id,
            x + width - 52.0,
            y + (height - 32.0) / 2.0,
            value,
            &ToggleOptions::default().disabled(disabled),
        );
        (value, height)
    }

    /// `C.toggleGrid`: toggle rows in one or two columns (two when the width allows). Returns
    /// the height used.
    pub fn toggle_grid(&mut self, rows: &mut [ToggleRow], x: f32, y: f32, width: f32) -> f32 {
        if rows.is_empty() {
            return 0.0;
        }
        let columns = if width >= 560.0 { 2 } else { 1 };
        let gap = 32.0;
        let column_width = (width - (columns as f32 - 1.0) * gap) / columns as f32;
        let per_column = rows.len().div_ceil(columns);
        for (index, row) in rows.iter_mut().enumerate() {
            let (column, line) = (index / per_column, index % per_column);
            let (value, _) = self.toggle_row(
                &row.id,
                row.label,
                row.help,
                x + column as f32 * (column_width + gap),
                y + line as f32 * TOGGLE_HEIGHT,
                column_width,
                *row.value,
                row.disabled,
            );
            *row.value = value;
        }
        per_column as f32 * TOGGLE_HEIGHT
    }

    /// `C.collapseHeader`: a clickable row with a chevron, a title and optional grey trailing
    /// text. Returns true when clicked.
    #[allow(clippy::too_many_arguments)]
    pub fn collapse_header(
        &mut self,
        id: &str,
        title: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        expanded: bool,
        trailing: Option<&str>,
    ) -> bool {
        let palette = self.palette();
        let region = self.ui.region(id, x, y, width, height, RegionOptions::hand());
        if region.hovered {
            self.fill_rect(x, y, width, height, 6.0, with_alpha(palette.on_surface, 0.05));
        }
        let chevron = if expanded { "expand_more" } else { "chevron_right" };
        self.icon_centered(chevron, x, y, 28.0, height, 20.0, palette.on_surface_variant);
        let title_width = width - 32.0 - if trailing.is_some() { 8.0 } else { 0.0 };
        self.text(title, x + 28.0, y, TextOptions::new().medium().middle_of(height).width(title_width).ellipsis());
        if let Some(trailing) = trailing {
            let name_width = self.text_width(title, 14.0, Weight::Medium);
            let options = TextOptions::new()
                .size(12.0)
                .middle_of(height)
                .width((title_width - name_width - 8.0).max(0.0))
                .ellipsis()
                .color(palette.on_surface_variant);
            self.text(trailing, x + 28.0 + name_width + 8.0, y, options);
        }
        region.clicked
    }

    /// `C.drawImageInBox(path, x, y, w, h, "contain", quarterTurns, rounded)`: the whole image
    /// letterboxed (turned by `quarter_turns`), optionally with card-style rounded corners.
    /// Returns Some(true) drawn, Some(false) still loading, None missing or broken.
    #[allow(clippy::too_many_arguments)]
    pub fn image_contain_in_box(
        &mut self,
        path: Option<&str>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        quarter_turns: i32,
        rounded: bool,
    ) -> Option<bool> {
        let path = path?;
        let Some(texture) = self.images.get(path) else {
            return (self.images.status(path) == Some(ImageStatus::Loading)).then_some(false);
        };
        let turned = quarter_turns.rem_euclid(2) == 1;
        let (box_width, box_height) =
            if turned { (texture.height, texture.width) } else { (texture.width, texture.height) };
        let scale = (width / box_width).min(height / box_height);
        let list = self.ui.list();
        if rounded {
            let (scaled_width, scaled_height) = (texture.width * scale, texture.height * scale);
            list.set_program(FragmentProgram::RoundedImage {
                quad_rect: [0.0, 0.0, 1.0, 1.0],
                size: [scaled_width, scaled_height],
                radius: radius_for(scaled_width.min(scaled_height)),
            });
        }
        list.set_color(crate::graphics::draw_list::Color::rgba(1.0, 1.0, 1.0, 1.0));
        let quad = Quad { x: 0.0, y: 0.0, width: texture.width, height: texture.height };
        list.draw_texture(
            texture,
            Some(quad),
            x + width / 2.0,
            y + height / 2.0,
            quarter_turns as f32 * std::f32::consts::PI / 2.0,
            scale,
            scale,
            texture.width / 2.0,
            texture.height / 2.0,
        );
        if rounded {
            list.set_program(FragmentProgram::Plain);
        }
        Some(true)
    }
}
