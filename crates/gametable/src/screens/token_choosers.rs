//! Token pickers and small editor widgets shared by the table's Change Tokens dialog and the game
//! definition editor. Ports of `tokenFolderImages`, `imagePreviewBox`, `chooseTokenImage`,
//! `chooseToken`, `chooseImage` and `stepper` from `src/ui/screens/gamedef/common.lua`.

use gametable_core::models::GameDefinition;

use crate::app::Frame;
use crate::data::file_system::{is_same_path, join_path, lowercase_extension, parent_folder, strip_extension};
use crate::data::game_files;
use crate::data::image_resolver::clear_existence_cache;
use crate::graphics::image_cache::ImageStatus;
use crate::screens::file_picker::{FilePicker, FilePickerResponse, IMAGE_FILTERS, PickerMode};
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::dropdown::DropdownOptions;
use crate::ui::overlays::{DialogAction, DialogResponse, DialogSpec};
use crate::ui::widgets::{ButtonOptions, ButtonVariant, Choice, IconButtonOptions, ScrollbarStyle};

/// An image in a game's `_tokens` (or `_playmats`) folder.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderImage {
    pub name: String,
    /// Root-relative path to store (`sub/name.png`, `/` on every OS).
    pub file: String,
    /// Resolved for display.
    pub path: String,
}

/// A subfolder: its name and root-relative path.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderLink {
    pub name: String,
    pub relative: String,
}

/// `C.tokenFolderImages(folderPath, subPath, rootName)`: images and subfolders, each sorted by
/// name ignoring case.
pub fn token_folder_images(
    folder_path: Option<&str>,
    sub_path: &str,
    root_name: &str,
) -> (Vec<FolderImage>, Vec<FolderLink>) {
    let Some(folder_path) = folder_path else { return (Vec::new(), Vec::new()) };
    let folder = join_path(&[folder_path, root_name, sub_path]);
    let relative_of = |name: &str| if sub_path.is_empty() { name.to_string() } else { format!("{sub_path}/{name}") };
    let (mut images, mut links) = (Vec::new(), Vec::new());
    for entry in game_files::list_folder(&folder).unwrap_or_default() {
        if entry.is_folder {
            links.push(FolderLink { name: entry.name.clone(), relative: relative_of(&entry.name) });
        } else if matches!(lowercase_extension(&entry.name).as_str(), ".png" | ".jpg" | ".jpeg" | ".webp") {
            images.push(FolderImage {
                name: strip_extension(&entry.name).to_string(),
                file: relative_of(&entry.name),
                path: join_path(&[&folder, &entry.name]),
            });
        }
    }
    images.sort_by_key(|image| image.name.to_lowercase());
    links.sort_by_key(|link| link.name.to_lowercase());
    (images, links)
}

/// The game's defined tokens resolved for display: (image, token type). Empty when the game
/// doesn't use tokens.
pub fn game_token_entries(game: &GameDefinition) -> Vec<(FolderImage, Option<String>)> {
    let Some(folder) = game.folder_path.as_deref() else { return Vec::new() };
    if !game.uses_tokens {
        return Vec::new();
    }
    game.tokens
        .iter()
        .map(|token| {
            let file = token.image_path.clone().unwrap_or_default();
            let path = join_path(&[folder, "_tokens", &file]);
            (FolderImage { name: token.display_name().to_string(), file, path }, token.token_type().map(str::to_string))
        })
        .collect()
}

/// Width of [`Frame::quantity_stepper`].
pub const STEPPER_WIDTH: f32 = 40.0 + 24.0 + 40.0 + 40.0;

impl Frame<'_> {
    /// `C.drawImageInBox(path, x, y, w, h, "cover")`: Some(true) drawn, Some(false) still loading,
    /// None missing or broken.
    fn image_cover_in_box(&mut self, path: &str, x: f32, y: f32, width: f32, height: f32) -> Option<bool> {
        let Some(texture) = self.images.get(path) else {
            return (self.images.status(path) == Some(ImageStatus::Loading)).then_some(false);
        };
        let scale = (width / texture.width).max(height / texture.height);
        self.ui.push_clip(x, y, width, height);
        self.draw_image(
            texture,
            x + width / 2.0 - texture.width * scale / 2.0,
            y + height / 2.0 - texture.height * scale / 2.0,
            scale,
            1.0,
        );
        self.ui.pop_clip();
        Some(true)
    }

    /// `C.imagePreviewBox`: the image cover-fit, or an icon when there's none or it can't load.
    pub fn image_preview_box(&mut self, path: Option<&str>, x: f32, y: f32, width: f32, height: f32) {
        let palette = self.palette();
        let drawn = path.and_then(|path| self.image_cover_in_box(path, x + 1.0, y + 1.0, width - 2.0, height - 2.0));
        if drawn.is_none() {
            let icon = if path.is_some() { "broken_image" } else { "image_not_supported" };
            self.icon_centered(icon, x, y, width, height, 24.0, palette.outline);
        }
        self.outline_rect(x + 0.5, y + 0.5, width - 1.0, height - 1.0, 6.0, palette.outline, 1.0);
    }

    /// `C.stepper`: −, the quantity, + and a remove button ending at `right`. Returns the new
    /// quantity and whether remove was clicked.
    pub fn quantity_stepper(
        &mut self,
        id: &str,
        right: f32,
        y: f32,
        quantity: i64,
        remove_tooltip: Option<&str>,
        disabled: bool,
    ) -> (i64, bool) {
        let palette = self.palette();
        let x = right - 40.0 - 40.0 - 24.0 - 40.0;
        let mut new_quantity = quantity;
        let decrement_id = format!("{id}:dec");
        let decrement =
            IconButtonOptions { id: Some(&decrement_id), disabled: disabled || quantity <= 1, ..Default::default() };
        if self.icon_button("remove", x, y, &decrement) {
            new_quantity = quantity - 1;
        }
        let color = if disabled { palette.disabled } else { palette.on_surface };
        let options = TextOptions::new().width(24.0).middle_of(40.0).centered().color(color);
        self.text(&quantity.to_string(), x + 40.0, y, options);
        let increment_id = format!("{id}:inc");
        if self.icon_button(
            "add",
            x + 64.0,
            y,
            &IconButtonOptions { id: Some(&increment_id), disabled, ..Default::default() },
        ) {
            new_quantity = quantity + 1;
        }
        let remove_id = format!("{id}:rm");
        let remove =
            IconButtonOptions { id: Some(&remove_id), tooltip: remove_tooltip, disabled, ..Default::default() };
        let removed = self.icon_button("close", x + 104.0, y, &remove);
        (new_quantity, removed)
    }
}

/// Copies a picked file into a folder (`ops.copyPickedImage` without orientation correction);
/// returns its bare file name.
pub fn copy_picked_file(source_path: &str, destination_folder: &str) -> Result<String, String> {
    let file_name = crate::data::file_system::base_name(source_path).to_string();
    let destination = join_path(&[destination_folder, &file_name]);
    if is_same_path(source_path, &destination) {
        return Ok(file_name);
    }
    std::fs::create_dir_all(destination_folder).map_err(|error| error.to_string())?;
    std::fs::copy(source_path, &destination).map_err(|_| format!("could not copy {source_path}"))?;
    Ok(file_name)
}

/// What a chooser reports.
#[derive(Debug, Clone, PartialEq)]
pub enum ChooserResponse {
    Open,
    /// A root-relative image path.
    Picked(String),
    Cancelled,
}

const TILE: f32 = 72.0;
const GAP: f32 = 8.0;
const COLUMNS: usize = 4;
const GRID_HEIGHT: f32 = 280.0;

enum ChooserMode {
    /// The game's defined tokens with a type filter (`C.chooseToken`).
    GameTokens { entries: Vec<(FolderImage, Option<String>)>, types: Vec<String>, selected_type: String },
    /// Browsing the folder with subfolders and Add Image... (`C.chooseTokenImage`).
    Folder { root_name: String, sub_path: String, add_picker: Option<FilePicker> },
}

/// `C.chooseToken(game, folderPath, onResult)` / `C.chooseTokenImage`.
pub struct TokenChooser {
    folder_path: Option<String>,
    mode: ChooserMode,
}

impl TokenChooser {
    /// The game's tokens, or the `_tokens` folder when the game defines none.
    pub fn for_game(game: &GameDefinition) -> TokenChooser {
        let entries = game_token_entries(game);
        if entries.is_empty() {
            return TokenChooser::for_folder(game.folder_path.clone(), "_tokens");
        }
        TokenChooser {
            folder_path: game.folder_path.clone(),
            mode: ChooserMode::GameTokens { entries, types: game.token_types(), selected_type: "all".to_string() },
        }
    }

    /// Browse `<folder>/<root name>` (`_tokens` or `_playmats`).
    pub fn for_folder(folder_path: Option<String>, root_name: &str) -> TokenChooser {
        TokenChooser {
            folder_path,
            mode: ChooserMode::Folder { root_name: root_name.to_string(), sub_path: String::new(), add_picker: None },
        }
    }

    pub fn show(&mut self, frame: &mut Frame) -> ChooserResponse {
        let actions = [DialogAction::text("Cancel")];
        let title = match self.mode {
            ChooserMode::GameTokens { .. } => "Choose Token",
            ChooserMode::Folder { .. } => "Choose Token Image",
        };
        let width = COLUMNS as f32 * TILE + (COLUMNS as f32 - 1.0) * GAP + 48.0;
        let spec = DialogSpec { title: Some(title), width, actions: &actions, ..DialogSpec::default() };
        let folder_path = self.folder_path.clone();
        let mut picked: Option<String> = None;
        let mut open_add_picker = false;
        let response = frame.dialog("tokenchooser", &spec, |frame, x, y, width| match &mut self.mode {
            ChooserMode::GameTokens { entries, types, selected_type } => {
                let mut grid_y = y;
                if !types.is_empty() {
                    let mut choices = vec![Choice::new("all".to_string(), "All")];
                    choices.extend(types.iter().map(|token_type| Choice::new(token_type.clone(), token_type.clone())));
                    let options = DropdownOptions { height: Some(40.0), ..DropdownOptions::default() };
                    *selected_type = frame.dropdown(
                        "ctoken:type",
                        x,
                        y,
                        width.min(220.0),
                        &choices,
                        selected_type.clone(),
                        &options,
                    );
                    grid_y = y + 40.0 + 12.0;
                }
                let filtered: Vec<&FolderImage> = entries
                    .iter()
                    .filter(|(_, token_type)| {
                        selected_type == "all" || token_type.as_deref() == Some(selected_type.as_str())
                    })
                    .map(|(image, _)| image)
                    .collect();
                if filtered.is_empty() {
                    let color = frame.palette().on_surface_variant;
                    let options = TextOptions::new().width(width).middle_of(GRID_HEIGHT).centered().color(color);
                    frame.text("No tokens of this type.", x, grid_y, options);
                    return (grid_y - y) + GRID_HEIGHT;
                }
                picked = image_grid(frame, "ctoken", x, grid_y, width, &filtered);
                (grid_y - y) + GRID_HEIGHT
            }
            ChooserMode::Folder { root_name, sub_path, .. } => {
                let palette = frame.palette();
                // A `.gamedef` package can't take new images.
                let is_packaged = folder_path.as_deref().is_some_and(game_files::is_packaged_game_folder);
                let add_options =
                    ButtonOptions::variant(ButtonVariant::Outlined).icon("add").id("ctok:add").disabled(is_packaged);
                let add_width = frame.button_width("Add Image...", &add_options);
                if frame.button("Add Image...", x, y, Some(add_width), Some(40.0), &add_options) {
                    open_add_picker = true;
                }
                let location = if sub_path.is_empty() { root_name.clone() } else { format!("{root_name}/{sub_path}") };
                let options = TextOptions::new()
                    .middle_of(40.0)
                    .color(palette.on_surface_variant)
                    .ellipsis()
                    .width(width - add_width - 12.0);
                frame.text(&location, x + add_width + 12.0, y, options);
                let grid_y = y + 40.0 + 12.0;
                let (images, links) = token_folder_images(folder_path.as_deref(), sub_path, root_name);
                // Up, then subfolders, then images, so navigation always comes first.
                enum Tile<'a> {
                    Up,
                    Folder(&'a FolderLink),
                    Image(&'a FolderImage),
                }
                let mut tiles: Vec<Tile> = Vec::new();
                if !sub_path.is_empty() {
                    tiles.push(Tile::Up);
                }
                tiles.extend(links.iter().map(Tile::Folder));
                tiles.extend(images.iter().map(Tile::Image));
                if tiles.is_empty() {
                    let options = TextOptions::new()
                        .width(width)
                        .middle_of(GRID_HEIGHT)
                        .centered()
                        .color(palette.on_surface_variant);
                    frame.text("No images in this folder yet.", x, grid_y, options);
                    return 40.0 + 12.0 + GRID_HEIGHT;
                }
                let offset = frame.begin_scroll("ctok:grid", x, grid_y, width, GRID_HEIGHT);
                let row_height = TILE + 20.0 + GAP;
                let (mut go_up, mut enter) = (false, None);
                for (index, tile) in tiles.iter().enumerate() {
                    let (column, row) = (index % COLUMNS, index / COLUMNS);
                    let (tile_x, tile_y) =
                        (x + column as f32 * (TILE + GAP), grid_y - offset + row as f32 * row_height);
                    if tile_y + TILE + 20.0 <= grid_y || tile_y >= grid_y + GRID_HEIGHT {
                        continue;
                    }
                    let region = frame.ui.region(
                        &format!("ctok:tile:{}", index + 1),
                        tile_x,
                        tile_y,
                        TILE,
                        TILE,
                        RegionOptions::hand(),
                    );
                    let label_options = TextOptions::new().size(11.0).width(TILE).centered().max_lines(1);
                    match tile {
                        Tile::Up => {
                            frame.fill_rect(tile_x, tile_y, TILE, TILE, 6.0, palette.surface_container_highest);
                            frame.icon_centered(
                                "arrow_upward",
                                tile_x,
                                tile_y,
                                TILE,
                                TILE,
                                28.0,
                                palette.on_surface_variant,
                            );
                            frame.text("Up", tile_x, tile_y + TILE + 2.0, label_options);
                            go_up |= region.clicked;
                        }
                        Tile::Folder(link) => {
                            frame.fill_rect(tile_x, tile_y, TILE, TILE, 6.0, palette.surface_container_highest);
                            frame.icon_centered("folder", tile_x, tile_y, TILE, TILE, 28.0, palette.on_surface_variant);
                            frame.text(&link.name, tile_x, tile_y + TILE + 2.0, label_options);
                            if region.clicked {
                                enter = Some(link.relative.clone());
                            }
                        }
                        Tile::Image(image) => {
                            frame.image_preview_box(Some(&image.path), tile_x, tile_y, TILE, TILE);
                            frame.text(&image.name, tile_x, tile_y + TILE + 2.0, label_options);
                            if region.clicked {
                                picked = Some(image.file.clone());
                            }
                        }
                    }
                }
                frame.end_scroll(
                    "ctok:grid",
                    tiles.len().div_ceil(COLUMNS) as f32 * row_height,
                    &ScrollbarStyle::default(),
                );
                if go_up {
                    let parent = parent_folder(sub_path);
                    *sub_path = if parent == "." { String::new() } else { parent };
                } else if let Some(enter) = enter {
                    *sub_path = enter;
                }
                40.0 + 12.0 + GRID_HEIGHT
            }
        });
        if let Some(file) = picked {
            return ChooserResponse::Picked(file);
        }
        if let ChooserMode::Folder { root_name, sub_path, add_picker } = &mut self.mode {
            if open_add_picker {
                *add_picker =
                    Some(FilePicker::new(frame, PickerMode::File, Some("Choose Image"), Some(IMAGE_FILTERS), None));
            }
            if let Some(picker) = add_picker {
                match picker.show(frame) {
                    FilePickerResponse::Open => {}
                    FilePickerResponse::Cancelled => *add_picker = None,
                    FilePickerResponse::Picked(mut paths) => {
                        *add_picker = None;
                        let source = paths.remove(0);
                        let folder = self.folder_path.as_deref().unwrap_or("");
                        let destination = join_path(&[folder, root_name, sub_path]);
                        match copy_picked_file(&source, &destination) {
                            Ok(file_name) => {
                                // The copy may replace a file the caches already hold.
                                frame.images.invalidate(&join_path(&[&destination, &file_name]));
                                clear_existence_cache();
                                let relative =
                                    if sub_path.is_empty() { file_name } else { format!("{sub_path}/{file_name}") };
                                return ChooserResponse::Picked(relative);
                            }
                            Err(error) => frame.ui.toast(&format!("Could not copy that image: {error}"), None),
                        }
                    }
                }
            }
        }
        match response {
            DialogResponse::Open | DialogResponse::Enter => ChooserResponse::Open,
            _ => ChooserResponse::Cancelled,
        }
    }
}

/// A scrolling 4-column grid of image tiles; returns the clicked one's file.
fn image_grid(frame: &mut Frame, prefix: &str, x: f32, y: f32, width: f32, images: &[&FolderImage]) -> Option<String> {
    let offset = frame.begin_scroll(&format!("{prefix}:grid"), x, y, width, GRID_HEIGHT);
    let row_height = TILE + 20.0 + GAP;
    let mut picked = None;
    for (index, image) in images.iter().enumerate() {
        let (column, row) = (index % COLUMNS, index / COLUMNS);
        let (tile_x, tile_y) = (x + column as f32 * (TILE + GAP), y - offset + row as f32 * row_height);
        if tile_y + TILE + 20.0 <= y || tile_y >= y + GRID_HEIGHT {
            continue;
        }
        let region =
            frame.ui.region(&format!("{prefix}:img:{}", index + 1), tile_x, tile_y, TILE, TILE, RegionOptions::hand());
        frame.image_preview_box(Some(&image.path), tile_x, tile_y, TILE, TILE);
        let options = TextOptions::new().size(11.0).width(TILE).centered().max_lines(1);
        frame.text(&image.name, tile_x, tile_y + TILE + 2.0, options);
        if region.clicked {
            picked = Some(image.file.clone());
        }
    }
    frame.end_scroll(
        &format!("{prefix}:grid"),
        images.len().div_ceil(COLUMNS) as f32 * row_height,
        &ScrollbarStyle::default(),
    );
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_images_and_links() {
        let root = std::env::temp_dir().join(format!("gametable-tokens-{}", gametable_core::uuid::new_v4_uuid()));
        std::fs::create_dir_all(root.join("_tokens").join("Sub")).unwrap();
        std::fs::write(root.join("_tokens").join("b.PNG"), "").unwrap();
        std::fs::write(root.join("_tokens").join("A.webp"), "").unwrap();
        std::fs::write(root.join("_tokens").join("notes.txt"), "").unwrap();
        std::fs::write(root.join("_tokens").join("Sub").join("c.jpg"), "").unwrap();
        let folder = root.to_string_lossy().into_owned();
        let (images, links) = token_folder_images(Some(&folder), "", "_tokens");
        assert_eq!(images.iter().map(|image| image.file.as_str()).collect::<Vec<_>>(), ["A.webp", "b.PNG"]);
        assert_eq!(links, vec![FolderLink { name: "Sub".into(), relative: "Sub".into() }]);
        let (images, _) = token_folder_images(Some(&folder), "Sub", "_tokens");
        assert_eq!(images[0].file, "Sub/c.jpg");
        let copied = copy_picked_file(
            &root.join("_tokens").join("b.PNG").to_string_lossy(),
            &join_path(&[&folder, "_tokens", "Sub"]),
        );
        assert_eq!(copied.as_deref(), Ok("b.PNG"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
