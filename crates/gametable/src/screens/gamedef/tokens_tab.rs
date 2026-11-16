//! The Tokens tab: a reorderable list of named token images backed by `game.tokens`. A token's
//! name may carry a `|Type` suffix (e.g. "Daisy Walker|Investigator") that groups it in the Token
//! Widget's type filter at the table. Port of `src/ui/screens/gamedef/tokens_tab.lua`.

use std::collections::HashSet;
use std::path::Path;

use gametable_core::models::TokenDefinition;
use gametable_core::uuid::new_v4_uuid;

use crate::app::Frame;
use crate::data::file_system::{
    base_name, join_path, list_files_recursive, lowercase_extension, parent_folder, strip_extension,
};
use crate::screens::gamedef::EditorModel;
use crate::screens::gamedef::common::{FIELD_HEIGHT, GAP, PAD, StableKeys, column};
use crate::screens::token_choosers::{ChooserResponse, TokenChooser};
use crate::ui::components::{Reorder, move_item};
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::panel::{Pane, clamp_selection};
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::with_alpha;
use crate::ui::widgets::{ButtonOptions, ButtonVariant, ScrollbarStyle};

const LIST_WIDTH: f32 = 300.0;
const ROW_HEIGHT: f32 = 56.0;
const THUMB: f32 = 40.0;
const BUTTON_HEIGHT: f32 = 36.0;

/// `T:autoAddTokens()`: a token for every image under `<folder>/_tokens` not already used. A file
/// one folder down is named `"<file>|<folder>"` so the folder becomes its type (deeper files use
/// their own containing folder). Returns how many were added.
pub fn auto_add_tokens(tokens: &mut Vec<TokenDefinition>, folder: &str) -> usize {
    let mut used: HashSet<String> = tokens.iter().filter_map(|token| token.image_path.clone()).collect();
    let mut added = 0;
    for file in list_files_recursive(Path::new(&join_path(&[folder, "_tokens"]))) {
        let relative = file.relative_path;
        if used.contains(&relative)
            || !matches!(lowercase_extension(&relative).as_str(), ".png" | ".jpg" | ".jpeg" | ".webp")
        {
            continue;
        }
        let directory = parent_folder(&relative);
        let token_type = (directory != ".").then(|| base_name(&directory).to_string());
        let stem = strip_extension(base_name(&relative)).to_string();
        let name = match token_type {
            Some(token_type) => format!("{stem}|{token_type}"),
            None => stem,
        };
        tokens.push(TokenDefinition { id: new_v4_uuid(), name, image_path: Some(relative.clone()) });
        used.insert(relative);
        added += 1;
    }
    added
}

#[derive(Default)]
pub struct TokensTab {
    keys: StableKeys,
    selected: Option<u64>,
    chooser: Option<(TokenChooser, u64)>,
    pending_remove: Option<usize>,
}

impl TokensTab {
    fn image_path(folder: &str, token: &TokenDefinition) -> Option<String> {
        token.image_path.as_deref().map(|file| join_path(&[folder, "_tokens", file]))
    }

    fn draw_list(&mut self, frame: &mut Frame, model: &mut EditorModel, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        frame.fill_rect(x, y, width, height, 0.0, palette.surface_container);
        let (header_height, _) = frame.section_header(
            "tt:list",
            "Tokens",
            x + 16.0,
            y + 12.0,
            width - 32.0,
            Some(model.game.tokens.len()),
            None,
            None,
        );
        let mut button_y = y + 12.0 + header_height + 8.0;
        let button_width = width - 32.0;
        let add_options = ButtonOptions::variant(ButtonVariant::Tonal).icon("add").id("tt:add");
        if frame.button("Add Token", x + 16.0, button_y, Some(button_width), Some(BUTTON_HEIGHT), &add_options) {
            model.game.tokens.push(TokenDefinition {
                id: new_v4_uuid(),
                name: "New Token".to_string(),
                image_path: None,
            });
            self.keys.sync(model.game.tokens.len());
            self.selected = Some(self.keys.key(model.game.tokens.len() - 1));
        }
        button_y += BUTTON_HEIGHT + 8.0;
        let auto_options = ButtonOptions::variant(ButtonVariant::Outlined).icon("upload_file").id("tt:auto");
        if frame.button("Auto Add Tokens", x + 16.0, button_y, Some(button_width), Some(BUTTON_HEIGHT), &auto_options) {
            let added = auto_add_tokens(&mut model.game.tokens, &model.folder_path);
            self.keys.sync(model.game.tokens.len());
            let message = if added > 0 {
                format!("{added} token{} added.", if added == 1 { "" } else { "s" })
            } else {
                "No new images found in the _tokens folder.".to_string()
            };
            frame.ui.toast(&message, None);
        }
        let list_y = button_y + BUTTON_HEIGHT + 8.0;
        let offset = frame.begin_scroll("tt:list", x, list_y, width, y + height - list_y);
        let reorder = Reorder::begin(frame, "tt:tokens");
        let mut row_y = list_y - offset;
        for index in 0..model.game.tokens.len() {
            let key = self.keys.key(index);
            let token = &model.game.tokens[index];
            let (row_x, row_width) = (x + 8.0, width - 16.0);
            let is_selected = self.selected == Some(key);
            let region = frame.ui.region(
                &format!("tt:row:{key}"),
                row_x + 40.0,
                row_y,
                row_width - 40.0,
                ROW_HEIGHT,
                RegionOptions::hand(),
            );
            if is_selected {
                frame.fill_rect(row_x, row_y, row_width, ROW_HEIGHT, 8.0, palette.secondary_container);
            } else if region.hovered {
                frame.fill_rect(row_x, row_y, row_width, ROW_HEIGHT, 8.0, with_alpha(palette.on_surface, 0.05));
            }
            reorder.handle(frame, index, row_x, row_y + (ROW_HEIGHT - 40.0) / 2.0, row_y, ROW_HEIGHT);
            let image_x = row_x + 40.0 + (ROW_HEIGHT - THUMB) / 2.0;
            let path = TokensTab::image_path(&model.folder_path, token);
            frame.image_preview_box(path.as_deref(), image_x, row_y + (ROW_HEIGHT - THUMB) / 2.0, THUMB, THUMB);
            let foreground = if is_selected { palette.on_secondary_container } else { palette.on_surface };
            let text_x = image_x + THUMB + 12.0;
            let name = token.display_name();
            let name = if name.is_empty() { "(unnamed token)" } else { name };
            let mut name_options =
                TextOptions::new().width(row_x + row_width - 8.0 - text_x).ellipsis().color(foreground);
            if is_selected {
                name_options = name_options.medium();
            }
            frame.text(name, text_x, row_y + 9.0, name_options);
            let options = TextOptions::new()
                .width(row_x + row_width - 8.0 - text_x)
                .ellipsis()
                .size(12.0)
                .color(palette.on_surface_variant);
            frame.text(token.token_type().unwrap_or("No type"), text_x, row_y + 30.0, options);
            if region.clicked {
                self.selected = Some(key);
                frame.ui.blur();
            }
            row_y += ROW_HEIGHT + 2.0;
        }
        if let Some((from, to)) = reorder.finish(frame, x + 8.0, width - 16.0) {
            move_item(&mut model.game.tokens, from, to);
            self.keys.moved(from, to);
        }
        frame.end_scroll(
            "tt:list",
            model.game.tokens.len() as f32 * (ROW_HEIGHT + 2.0) + 12.0,
            &ScrollbarStyle::default(),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_detail(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        index: usize,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let palette = frame.palette();
        let key = self.keys.key(index);
        let id = format!("tt:{key}");
        let offset = frame.begin_scroll("tt:detail", x, y, width, height);
        let (column_x, column_width) = column(x, width);
        let mut row_y = y + PAD - offset;
        let folder = model.folder_path.clone();
        let token = &mut model.game.tokens[index];

        let display = token.display_name().to_string();
        let display = if display.is_empty() { "(unnamed token)".to_string() } else { display };
        let options = TextOptions::new().size(20.0).medium().middle_of(40.0).width(column_width - 160.0).ellipsis();
        frame.text(&display, column_x, row_y, options);
        let delete_id = format!("{id}:del");
        let delete_options = ButtonOptions::variant(ButtonVariant::Text).icon("delete_outline").id(&delete_id);
        let delete_width = frame.button_width("Delete Token", &delete_options);
        if frame.button(
            "Delete Token",
            column_x + column_width - delete_width,
            row_y,
            Some(delete_width),
            Some(40.0),
            &delete_options,
        ) {
            self.pending_remove = Some(index);
        }
        row_y += 40.0 + 16.0;

        let name_id = format!("{id}:name");
        let options = TextFieldOptions { trailing_width: 28.0, ..TextFieldOptions::label("Name") };
        frame.text_field(&name_id, column_x, row_y, column_width, &mut token.name, &options);
        frame.info_icon(
            &format!("{name_id}:info"),
            column_x + column_width - 30.0,
            row_y + (FIELD_HEIGHT - 18.0) / 2.0,
            "Add a \"|\" followed by a type to group this token for the Token Widget's type filter at the table, \
             e.g. \"Daisy Walker|Investigator\".",
        );
        row_y += FIELD_HEIGHT + 4.0;
        let shown = token.display_name();
        let shown = if shown.is_empty() { "(unnamed token)" } else { shown };
        let type_text = match token.token_type() {
            Some(token_type) => format!(", type \"{token_type}\""),
            None => ", no type".to_string(),
        };
        let options =
            TextOptions::new().width(column_width).middle_of(20.0).size(12.0).color(palette.on_surface_variant);
        frame.text(&format!("Shown as \"{shown}\"{type_text}"), column_x, row_y, options);
        row_y += 20.0 + GAP;

        let big = 96.0;
        let region = frame.ui.region(
            &format!("{id}:img"),
            column_x,
            row_y,
            big,
            big,
            RegionOptions::hand().with_tooltip(Some("Choose Token Image...")),
        );
        let path = TokensTab::image_path(&folder, token);
        frame.image_preview_box(path.as_deref(), column_x, row_y, big, big);
        if region.clicked {
            self.chooser = Some((TokenChooser::for_folder(Some(folder.clone()), "_tokens"), key));
        }
        let color = if token.image_path.is_some() { palette.on_surface } else { palette.on_surface_variant };
        let options = TextOptions::new().width(column_width - big - 12.0).middle_of(big).ellipsis().color(color);
        frame.text(token.image_path.as_deref().unwrap_or("(no image)"), column_x + big + 12.0, row_y, options);
        row_y += big + PAD;
        frame.end_scroll("tt:detail", row_y + offset - y, &ScrollbarStyle::default());
    }

    pub fn draw(&mut self, frame: &mut Frame, model: &mut EditorModel, x: f32, y: f32, width: f32, height: f32) {
        self.keys.sync(model.game.tokens.len());
        self.pending_remove = None;
        let keys: Vec<u64> = (0..model.game.tokens.len()).map(|index| self.keys.key(index)).collect();
        clamp_selection(&mut self.selected, &keys);
        let selected_index = self.selected.and_then(|key| self.keys.index_of(key));
        frame.list_detail(
            x,
            y,
            width,
            height,
            LIST_WIDTH,
            selected_index.is_some(),
            "Add a token to get started",
            |frame, pane| match pane {
                Pane::List(rect) => self.draw_list(frame, model, rect.x, rect.y, rect.width, rect.height),
                Pane::Detail(rect) => {
                    if let Some(index) = selected_index {
                        self.draw_detail(frame, model, index, rect.x, rect.y, rect.width, rect.height);
                    }
                }
            },
        );
        if let Some(index) = self.pending_remove.take() {
            model.game.tokens.remove(index);
            self.keys.removed(index);
            let count = model.game.tokens.len();
            self.selected = (count > 0).then(|| self.keys.key(index.min(count - 1)));
        }
        if let Some((chooser, key)) = &mut self.chooser {
            match chooser.show(frame) {
                ChooserResponse::Open => {}
                ChooserResponse::Cancelled => self.chooser = None,
                ChooserResponse::Picked(file) => {
                    let key = *key;
                    self.chooser = None;
                    if let Some(index) = self.keys.index_of(key) {
                        model.game.tokens[index].image_path = Some(file);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_add_names_tokens_by_their_folder() {
        let root = std::env::temp_dir().join(format!("gametable_tokens_tab_{}", std::process::id()));
        let tokens_folder = root.join("_tokens");
        std::fs::create_dir_all(tokens_folder.join("Investigator").join("Deep")).unwrap();
        for file in ["Clue.png", "notes.txt", "Investigator/Daisy.jpg", "Investigator/Deep/Roland.webp", "Used.png"] {
            std::fs::write(tokens_folder.join(file), b"x").unwrap();
        }
        let mut tokens =
            vec![TokenDefinition { id: "x".into(), name: "Used".into(), image_path: Some("Used.png".into()) }];
        let added = auto_add_tokens(&mut tokens, root.to_str().unwrap());
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(added, 3);
        let names: Vec<(&str, &str)> =
            tokens[1..].iter().map(|token| (token.name.as_str(), token.image_path.as_deref().unwrap())).collect();
        assert_eq!(
            names,
            [
                ("Clue", "Clue.png"),
                ("Daisy|Investigator", "Investigator/Daisy.jpg"),
                ("Roland|Deep", "Investigator/Deep/Roland.webp"),
            ]
        );
        assert_eq!(auto_add_tokens(&mut tokens, "missing folder"), 0);
    }
}
