//! The game library grid with its folder path, Refresh and Change Folder. Port of
//! `src/ui/screens/game_picker.lua` (from `game_picker.dart` + `game_grid.dart`). A component
//! that screens embed, not a screen itself.

use std::path::Path;

use gametable_core::models::GameDefinition;

use crate::app::Frame;
use crate::data::file_system::{folder_has_entries, is_same_path, move_folder_contents};
use crate::data::game_loader::{load_games_from_folder, load_standard_deck};
use crate::data::image_resolver::{clear_existence_cache, resolve_game_logo_image_path};
use crate::data::paths::set_games_folder;
use crate::graphics::draw_list::Color;
use crate::graphics::image_cache::ImageStatus;
use crate::screens::file_picker::{FilePicker, FilePickerResponse, PickerMode};
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::overlays::{DialogAction, DialogResponse, DialogSpec};
use crate::ui::theme::{fixed, with_alpha};
use crate::ui::widgets::{IconButtonOptions, ScrollbarStyle};

pub const TILE_WIDTH: f32 = 268.0;
pub const MAX_COLUMNS: usize = 7;
pub const SPACING: f32 = 16.0;
pub const TEXT_BLOCK_HEIGHT: f32 = 64.0;

/// One tile of [`Frame::game_grid`].
#[derive(Debug, Clone, Default)]
pub struct GameTile<'a> {
    pub id: &'a str,
    pub name: &'a str,
    /// The game folder; its `gamelogo.*` is the tile's picture unless `image_path` is set.
    pub folder_path: Option<&'a str>,
    pub image_path: Option<&'a str>,
    /// The line under the name; defaults to "<n> card(s)".
    pub caption: Option<String>,
    pub card_count: usize,
    /// Dimmed and not clickable (e.g. can't afford it yet).
    pub disabled: bool,
    pub tooltip: Option<&'a str>,
}

impl<'a> GameTile<'a> {
    pub fn for_game(game: &'a GameDefinition) -> GameTile<'a> {
        GameTile {
            id: &game.id,
            name: &game.name,
            folder_path: game.folder_path.as_deref(),
            card_count: game.cards().len(),
            ..GameTile::default()
        }
    }
}

fn column_count(width: f32) -> usize {
    (((width + SPACING) / (TILE_WIDTH + SPACING)).floor() as usize).clamp(1, MAX_COLUMNS)
}

/// `P.gridHeight(w, count)`: the height of a grid of `count` tiles at this width.
pub fn grid_height(width: f32, count: usize) -> f32 {
    let columns = column_count(width);
    let tile_width = if columns == 1 && width < TILE_WIDTH { width } else { TILE_WIDTH };
    let tile_height = tile_width + TEXT_BLOCK_HEIGHT;
    let rows = count.div_ceil(columns).max(1) as f32;
    rows * tile_height + (rows - 1.0) * SPACING + 8.0
}

impl Frame<'_> {
    fn game_tile(&mut self, tile: &GameTile, x: f32, y: f32, width: f32, height: f32) -> bool {
        let palette = self.palette();
        let region = if tile.disabled {
            None
        } else {
            let id = format!("game:{}:{}", tile.folder_path.unwrap_or(""), tile.id);
            Some(self.ui.region(&id, x, y, width, height, RegionOptions::hand().with_tooltip(tile.tooltip)))
        };
        let hovered = region.as_ref().is_some_and(|region| region.hovered);
        self.fill_rect(x + 1.0, y + 1.0, width, height, 8.0, Color::rgba(0.0, 0.0, 0.0, 0.38));
        self.fill_rect(x, y, width, height, 8.0, palette.surface);
        self.outline_rect(x + 0.5, y + 0.5, width - 1.0, height - 1.0, 8.0, palette.outline_variant, 1.0);
        if hovered {
            self.fill_rect(x, y, width, height, 8.0, with_alpha(palette.on_surface, 0.06));
        }
        let padding = 12.0;
        let logo_size = (width - 2.0 * padding).min(height - 2.0 * padding - 8.0 - 44.0);
        let (logo_x, logo_y) = (x + (width - logo_size) / 2.0, y + padding);
        let logo_path = tile.image_path.map(str::to_string).or_else(|| resolve_game_logo_image_path(tile.folder_path));
        let texture = logo_path.as_deref().and_then(|path| self.images.get(path));
        if let Some(texture) = texture {
            let scale = (logo_size / texture.width).min(logo_size / texture.height);
            let image_x = logo_x + (logo_size - texture.width * scale) / 2.0;
            let image_y = logo_y + (logo_size - texture.height * scale) / 2.0;
            self.draw_image(texture, image_x, image_y, scale, if tile.disabled { 0.5 } else { 1.0 });
        } else if logo_path.as_deref().is_none_or(|path| self.images.status(path) == Some(ImageStatus::Error)) {
            self.fill_rect(logo_x, logo_y, logo_size, logo_size, 6.0, palette.surface_container_highest);
            self.icon_centered("style", logo_x, logo_y, logo_size, logo_size, 32.0, palette.on_surface_variant);
        }
        let text_y = logo_y + logo_size + 8.0;
        let name_color = if tile.disabled { palette.disabled } else { palette.on_surface };
        let name_options =
            TextOptions::new().bold().width(width - 2.0 * padding).centered().ellipsis().color(name_color);
        self.text(tile.name, x + padding, text_y, name_options);
        let caption = tile.caption.clone().unwrap_or_else(|| format!("{} card(s)", tile.card_count));
        let caption_color = if tile.disabled { palette.disabled } else { palette.on_surface_variant };
        let caption_options =
            TextOptions::new().size(12.0).width(width - 2.0 * padding).centered().color(caption_color);
        self.text(&caption, x + padding, text_y + 22.0, caption_options);
        region.is_some_and(|region| region.clicked)
    }

    /// `P.drawGrid`: up to seven 268-unit tiles per row, each row centred on its own tiles, in a
    /// scroll view that extends `padding` past each side. Returns the clicked tile's index.
    #[allow(clippy::too_many_arguments)]
    pub fn game_grid(
        &mut self,
        scroll_id: &str,
        tiles: &[GameTile],
        x: f32,
        y: f32,
        width: f32,
        view_height: f32,
        padding: f32,
    ) -> Option<usize> {
        let columns = column_count(width);
        let tile_width = if columns == 1 && width < TILE_WIDTH { width } else { TILE_WIDTH };
        let tile_height = tile_width + TEXT_BLOCK_HEIGHT;
        let offset = self.begin_scroll(scroll_id, x - padding, y, width + 2.0 * padding, view_height);
        let rows = tiles.len().div_ceil(columns);
        let mut clicked = None;
        for (index, tile) in tiles.iter().enumerate() {
            let (column, row) = (index % columns, index / columns);
            let tiles_in_row = columns.min(tiles.len() - row * columns);
            let row_width = tiles_in_row as f32 * (tile_width + SPACING) - SPACING;
            let tile_x = x + (width - row_width) / 2.0 + column as f32 * (tile_width + SPACING);
            let tile_y = y + 4.0 + row as f32 * (tile_height + SPACING) - offset;
            let is_visible = tile_y + tile_height > y - 10.0 && tile_y < y + view_height + 10.0;
            if is_visible && self.game_tile(tile, tile_x, tile_y, tile_width, tile_height) {
                clicked = Some(index);
            }
        }
        let rows = rows as f32;
        self.end_scroll(scroll_id, rows * tile_height + (rows - 1.0) * SPACING + 8.0, &ScrollbarStyle::default());
        clicked
    }
}

/// The "Move Existing Files?" step after choosing a new library folder
/// (`P.maybeMoveLibraryFolder`, port of `move_library_prompt.dart`).
#[derive(Debug, Clone, PartialEq)]
pub struct MoveLibraryPrompt {
    pub old_root: String,
    pub new_root: String,
    /// "game" or "deck".
    pub what_label: &'static str,
}

/// What the move prompt reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovePromptResponse {
    Open,
    /// The choice was made (and the move done, if chosen): switch to the new folder now.
    Finished,
}

impl MoveLibraryPrompt {
    /// `None` when there's nothing to ask (same folder, or nothing in the old one).
    pub fn needed(old_root: &str, new_root: &str, what_label: &'static str) -> Option<MoveLibraryPrompt> {
        if is_same_path(old_root, new_root) || !folder_has_entries(Path::new(old_root)) {
            return None;
        }
        Some(MoveLibraryPrompt { old_root: old_root.to_string(), new_root: new_root.to_string(), what_label })
    }

    pub fn show(&self, frame: &mut Frame) -> MovePromptResponse {
        let message = format!(
            "Move your existing {} library files from\n\"{}\"\nto\n\"{}\"?",
            self.what_label, self.old_root, self.new_root
        );
        let actions = [DialogAction::text("Don't Move"), DialogAction::filled("Move")];
        let spec = DialogSpec {
            title: Some("Move Existing Files?"),
            message: Some(&message),
            width: 480.0,
            dismissible: false,
            actions: &actions,
            ..DialogSpec::default()
        };
        match frame.dialog("movelibrary", &spec, |_, _, _, _| 0.0) {
            DialogResponse::Action(1) => {
                if let Err(error) = move_folder_contents(Path::new(&self.old_root), Path::new(&self.new_root)) {
                    frame.ui.toast(&format!("Could not move the files: {error}"), None);
                }
                MovePromptResponse::Finished
            }
            DialogResponse::Action(_) => MovePromptResponse::Finished,
            _ => MovePromptResponse::Open,
        }
    }
}

/// The embedded game picker.
pub struct GamePicker {
    pub exclude_standard_deck: bool,
    directory_path: String,
    standard_deck: GameDefinition,
    games: Vec<GameDefinition>,
    busy: bool,
    error: Option<String>,
    /// Frames until the load runs (so the progress bar gets drawn first).
    pending_load: Option<u32>,
    folder_picker: Option<FilePicker>,
    move_prompt: Option<MoveLibraryPrompt>,
}

impl GamePicker {
    /// `P.new(onGameChosen, opts)`; `exclude_standard_deck` hides the Standard 52-Card Deck (it
    /// can't be customized, so the deck editors leave it out).
    pub fn new(frame: &Frame, exclude_standard_deck: bool) -> GamePicker {
        let mut picker = GamePicker {
            exclude_standard_deck,
            directory_path: frame.paths.games_folder(frame.settings).to_string_lossy().into_owned(),
            standard_deck: load_standard_deck(),
            games: Vec::new(),
            busy: false,
            error: None,
            pending_load: None,
            folder_picker: None,
            move_prompt: None,
        };
        picker.refresh();
        picker
    }

    /// Loads on the second update from now so the progress indicator gets a frame first.
    pub fn refresh(&mut self) {
        self.busy = true;
        self.error = None;
        self.pending_load = Some(2);
    }

    pub fn update(&mut self) {
        let Some(frames_left) = self.pending_load else { return };
        if frames_left > 1 {
            self.pending_load = Some(frames_left - 1);
            return;
        }
        self.pending_load = None;
        clear_existence_cache();
        let (mut games, _errors) = load_games_from_folder(&self.directory_path);
        games.sort_by(|first, second| first.name.cmp(&second.name));
        if games.is_empty() {
            self.error = Some("No valid games found in that folder.".to_string());
        }
        self.games = games;
        self.busy = false;
    }

    fn use_folder(&mut self, frame: &mut Frame, folder: String) {
        let _ = set_games_folder(frame.settings, &folder);
        self.directory_path = folder;
        self.refresh();
    }

    /// Draws the picker; returns the game the player chose this frame.
    pub fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) -> Option<GameDefinition> {
        let palette = frame.palette();
        let padding = 24.0;
        let (x, y, width, height) = (x + padding, y + padding, width - 2.0 * padding, height - 2.0 * padding);
        let mut row_y = y;
        frame.text("Games", x, row_y, TextOptions::new().bold());
        row_y += 28.0;
        let path_options = TextOptions::new()
            .size(12.0)
            .color(palette.on_surface_variant)
            .width(width - 96.0)
            .middle_of(40.0)
            .ellipsis();
        frame.text(&self.directory_path, x, row_y, path_options);
        let refresh_options = IconButtonOptions::tooltip("Refresh").id("gp:refresh").disabled(self.busy);
        if frame.icon_button("refresh", x + width - 88.0, row_y, &refresh_options) {
            self.refresh();
        }
        let folder_options = IconButtonOptions::tooltip("Change Folder...").id("gp:folder").disabled(self.busy);
        if frame.icon_button("folder_open", x + width - 44.0, row_y, &folder_options) {
            self.folder_picker = Some(FilePicker::new(
                frame,
                PickerMode::Folder,
                Some("Choose your game library folder"),
                None,
                Some(&self.directory_path),
            ));
        }
        row_y += 44.0;
        if self.busy {
            frame.progress_bar(x, row_y + 4.0, width, 2.0, None, None);
            row_y += 10.0;
        } else if let Some(error) = &self.error {
            frame.text(error, x, row_y + 4.0, TextOptions::new().color(fixed::RED).width(width));
            row_y += 24.0;
        }
        row_y += 12.0;

        let mut games: Vec<&GameDefinition> =
            self.games.iter().filter(|game| game.id != self.standard_deck.id).collect();
        if !self.exclude_standard_deck {
            games.push(&self.standard_deck);
        }
        let tiles: Vec<GameTile> = games.iter().map(|game| GameTile::for_game(game)).collect();
        let chosen = frame
            .game_grid("gp:grid", &tiles, x, row_y, width, y + height - row_y, padding)
            .map(|index| games[index].clone());

        self.show_folder_dialogs(frame);
        chosen
    }

    fn show_folder_dialogs(&mut self, frame: &mut Frame) {
        if let Some(picker) = &mut self.folder_picker {
            match picker.show(frame) {
                FilePickerResponse::Open => {}
                FilePickerResponse::Cancelled => self.folder_picker = None,
                FilePickerResponse::Picked(mut paths) => {
                    self.folder_picker = None;
                    let new_root = paths.remove(0);
                    match MoveLibraryPrompt::needed(&self.directory_path, &new_root, "game") {
                        Some(prompt) => self.move_prompt = Some(prompt),
                        None => self.use_folder(frame, new_root),
                    }
                }
            }
        }
        if let Some(prompt) = &self.move_prompt
            && prompt.show(frame) == MovePromptResponse::Finished
        {
            let new_root = prompt.new_root.clone();
            self.move_prompt = None;
            self.use_folder(frame, new_root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_sizes() {
        assert_eq!(column_count(268.0), 1);
        assert_eq!(column_count(268.0 * 2.0 + 16.0), 2);
        assert_eq!(column_count(10_000.0), MAX_COLUMNS);
        assert_eq!(grid_height(552.0, 3), 2.0 * 332.0 + 16.0 + 8.0);
        assert_eq!(grid_height(200.0, 0), 200.0 + 64.0 + 8.0);
    }
}
