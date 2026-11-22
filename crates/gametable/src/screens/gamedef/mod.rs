//! The game definition editor. Port of `src/ui/screens/gamedef_editor.lua`
//! (`game_definition_editor_entry_screen.dart`, `game_definition_editor_screen.dart`): pick (or
//! create) a game definition, edit it across the General / Game Boards / Zones / Sets / Cards tabs
//! (plus Tokens and Scenarios while the game uses them), and save it back to its folder's
//! `gamedef.json`.
//!
//! The editor's game is loaded WITHOUT resolving image paths, so saving writes the bare file names
//! back unchanged. The game's id stays whatever its folder or file gave it. Unlike the original,
//! leaving (Back, quitting, New, Open) with unsaved changes asks first.

pub mod board_zone_placement;
pub mod card_art_job;
pub mod card_view_tab;
pub mod common;
pub mod convert_to_webp_dialog;
pub mod dialogs;
pub mod extra_data_suggestion_dialogs;
pub mod game_board_tab;
pub mod list_picker_dialog;
pub mod save_to_zip_dialog;
pub mod scenario_tab;
pub mod sets_tab;
pub mod settings_tab;
pub mod shrink_card_art_dialog;
pub mod suggestion_review;
pub mod table_board_placement;
pub mod title_area_picker;
pub mod title_suggestion_dialogs;
pub mod tokens_tab;
pub mod widget_scale_dialog;
pub mod zones_tab;

use gametable_core::models::GameDefinition;
use gametable_core::util::trim_lua_whitespace;

use crate::app::{Frame, Screen, ScreenCommand};
use crate::data::file_system::{base_name, join_path, parent_folder};
use crate::data::game_extract::{GameExtractJob, extraction_folder};
use crate::data::game_files::is_packaged_game_folder;
use crate::data::game_package::game_package_path;
use crate::data::gamedef_ops::{
    cards_outside_sets, create_game_definition, find_game_definition_file, load_game_definition_file,
    scan_game_library, validate_game_id, write_game_definition,
};
use crate::data::image_resolver::clear_existence_cache;
use crate::data::paths::set_games_folder;
use crate::screens::file_picker::{FileFilter, FilePicker, FilePickerResponse, PickerMode};
use crate::screens::game_picker::{GameTile, MoveLibraryPrompt, MovePromptResponse};
use crate::screens::rulebook_viewer::RulebookViewer;
use crate::ui::components::{EDITOR_TOOLBAR_HEIGHT, TAB_BAR_HEIGHT, ToolbarAction};
use crate::ui::draw::TextOptions;
use crate::ui::overlays::{
    ConfirmResponse, ContextMenu, DialogAction, DialogResponse, DialogSpec, MenuItem, MenuResponse,
};
use crate::ui::text_field::{TextFieldOptions, text_field_height};
use crate::ui::widgets::{ButtonOptions, ButtonVariant, IconButtonOptions};

use self::card_view_tab::CardViewTab;
use self::common::StableKeys;
use self::convert_to_webp_dialog::apply_webp_conversion;
use self::dialogs::ToolDialog;
use self::game_board_tab::GameBoardTab;
use self::save_to_zip_dialog::SaveToZipFlow;
use self::scenario_tab::ScenarioTab;
use self::sets_tab::SetsTab;
use self::settings_tab::SettingsTab;
use self::tokens_tab::TokensTab;
use self::zones_tab::ZonesTab;

const GAME_DEFINITION_FILTERS: &[FileFilter] = &[FileFilter { name: "Game Definition", extensions: &["json"] }];

/// The game being edited, shared by every tab.
pub struct EditorModel {
    pub folder_path: String,
    pub game: GameDefinition,
    /// One key per card, following it through edits on any tab.
    pub card_keys: StableKeys,
}

// ---------------------------------------------------------------------------
// New / open flows
// ---------------------------------------------------------------------------

/// `M.newGameDefinition`: asks for an id and writes a stub `gamedef.json` under
/// `<games library>/<id>/`.
pub struct NewGameDialog {
    root: String,
    id: String,
    error: Option<String>,
}

/// What the New Game Definition prompt reports.
pub enum NewGameResponse {
    Open,
    Created(Box<GameDefinition>),
    Cancelled,
}

impl NewGameDialog {
    pub fn new(frame: &Frame) -> NewGameDialog {
        let root = frame.paths.games_folder(frame.settings).to_string_lossy().into_owned();
        NewGameDialog { root, id: String::new(), error: None }
    }

    fn submit(&mut self) -> Option<GameDefinition> {
        let id = trim_lua_whitespace(&self.id).to_string();
        let mut error = validate_game_id(&id).map(str::to_string);
        if error.is_none() && std::path::Path::new(&join_path(&[&self.root, &id])).exists() {
            error = Some("A game with this ID already exists.".to_string());
        }
        if error.is_some() {
            self.error = error;
            return None;
        }
        match create_game_definition(&self.root, &id) {
            Ok(game) => Some(game),
            Err(message) => {
                self.error = Some(format!("Could not create it: {message}"));
                None
            }
        }
    }

    pub fn show(&mut self, frame: &mut Frame) -> NewGameResponse {
        let actions = [DialogAction::text("Cancel"), DialogAction::filled("Create")];
        let spec =
            DialogSpec { title: Some("New Game Definition"), width: 400.0, actions: &actions, ..DialogSpec::default() };
        let label_background = frame.palette().surface_container_high;
        let mut submitted = false;
        let response = frame.dialog("newgame", &spec, |frame, x, y, width| {
            let options = TextFieldOptions {
                autofocus: true,
                error: self.error.as_deref(),
                label_background: Some(label_background),
                ..TextFieldOptions::label("Game ID")
            };
            let field = frame.text_field("newgame:id", x, y + 8.0, width, &mut self.id, &options);
            if field.changed {
                self.error = None;
            }
            submitted = field.submitted;
            8.0 + text_field_height(None, self.error.as_deref())
        });
        if submitted || response == DialogResponse::Action(1) {
            return match self.submit() {
                Some(game) => NewGameResponse::Created(Box::new(game)),
                None => NewGameResponse::Open,
            };
        }
        match response {
            DialogResponse::Open | DialogResponse::Enter => NewGameResponse::Open,
            _ => NewGameResponse::Cancelled,
        }
    }
}

/// `M.openExistingGameDefinition`: picks a `gamedef.json` anywhere on disk.
fn open_existing_picker(frame: &Frame) -> FilePicker {
    FilePicker::new(frame, PickerMode::File, Some("Open Game Definition"), Some(GAME_DEFINITION_FILTERS), None)
}

/// Shows the Open picker; returns the loaded game once picked (a toast when it isn't valid).
fn show_open_picker(frame: &mut Frame, picker: &mut Option<FilePicker>) -> Option<GameDefinition> {
    let response = picker.as_mut()?.show(frame);
    match response {
        FilePickerResponse::Open => None,
        FilePickerResponse::Cancelled => {
            *picker = None;
            None
        }
        FilePickerResponse::Picked(mut paths) => {
            *picker = None;
            let path = paths.remove(0);
            match load_game_definition_file(&path, &parent_folder(&path)) {
                Ok(game) => Some(game),
                Err(_) => {
                    frame.ui.toast("That file is not a valid game definition.", None);
                    None
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Editor
// ---------------------------------------------------------------------------

const TAB_LABELS: [&str; 5] = ["General", "Game Boards", "Zones", "Sets", "Cards"];

/// What the user was doing when the unsaved-changes prompt appeared; carried out on "Discard".
#[derive(Debug, Clone, Copy, PartialEq)]
enum LeaveAction {
    Back,
    Quit,
    NewGame,
    OpenExisting,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tool {
    DuplicateIds,
    MergeImages,
}

pub struct GameDefinitionEditor {
    model: EditorModel,
    /// 0-based; past the fixed tabs it counts the optional tabs that are on.
    tab: usize,
    settings_tab: SettingsTab,
    game_board_tab: GameBoardTab,
    zones_tab: ZonesTab,
    sets_tab: SetsTab,
    cards_tab: CardViewTab,
    tokens_tab: Option<TokensTab>,
    scenario_tab: Option<ScenarioTab>,
    rulebook: RulebookViewer,
    new_game: Option<NewGameDialog>,
    open_picker: Option<FilePicker>,
    tools_menu: Option<ContextMenu<Tool>>,
    tool_dialog: Option<ToolDialog>,
    /// "Cards Without a Set" warning text while it is shown.
    save_warning: Option<String>,
    /// The game as it was last loaded or saved, to tell whether there are unsaved changes.
    saved_game: GameDefinition,
    /// Set while the "Unsaved Changes" prompt is shown.
    pending_leave: Option<LeaveAction>,
    /// "Save to Zip" while it runs.
    save_to_zip: Option<SaveToZipFlow>,
    /// "Save to Zip" saved first and is waiting on the "Cards Without a Set" warning.
    save_to_zip_after_saving: bool,
}

impl GameDefinitionEditor {
    pub fn new(game: GameDefinition) -> GameDefinitionEditor {
        let folder_path = game.folder_path.clone().unwrap_or_default();
        let mut card_keys = StableKeys::default();
        card_keys.sync(game.cards().len());
        clear_existence_cache();
        GameDefinitionEditor {
            rulebook: RulebookViewer::new(&game),
            cards_tab: CardViewTab::new(&game),
            saved_game: game.clone(),
            model: EditorModel { folder_path, game, card_keys },
            tab: 0,
            settings_tab: SettingsTab::default(),
            game_board_tab: GameBoardTab::new(),
            zones_tab: ZonesTab::default(),
            sets_tab: SetsTab::default(),
            tokens_tab: None,
            scenario_tab: None,
            new_game: None,
            open_picker: None,
            tools_menu: None,
            tool_dialog: None,
            save_warning: None,
            pending_leave: None,
            save_to_zip: None,
            save_to_zip_after_saving: false,
        }
    }

    /// Whether the game differs from what was last loaded or saved.
    fn has_unsaved_changes(&self) -> bool {
        self.model.game != self.saved_game
    }

    /// Carries out `action` now, or asks first when there are unsaved changes.
    fn leave_or_confirm(&mut self, frame: &mut Frame, action: LeaveAction) {
        if self.has_unsaved_changes() {
            self.pending_leave = Some(action);
        } else {
            self.carry_out_leave(frame, action);
        }
    }

    fn carry_out_leave(&mut self, frame: &mut Frame, action: LeaveAction) {
        match action {
            LeaveAction::Back => frame.commands.push(ScreenCommand::Pop),
            LeaveAction::Quit => frame.commands.push(ScreenCommand::Quit),
            LeaveAction::NewGame => self.new_game = Some(NewGameDialog::new(frame)),
            LeaveAction::OpenExisting => self.open_picker = Some(open_existing_picker(frame)),
        }
    }

    /// The "Unsaved Changes" prompt, while one is pending.
    fn draw_unsaved_changes_prompt(&mut self, frame: &mut Frame) {
        let Some(action) = self.pending_leave else { return };
        let name = if self.model.game.name.trim().is_empty() { "this game" } else { self.model.game.name.as_str() };
        let message = format!("You have unsaved changes to {name}. If you leave now, they will be lost.");
        let actions = [DialogAction::text("Cancel"), DialogAction::new("Discard Changes", ButtonVariant::Danger)];
        let spec = DialogSpec {
            title: Some("Unsaved Changes"),
            message: Some(&message),
            width: 440.0,
            actions: &actions,
            ..DialogSpec::default()
        };
        match frame.dialog("unsavedchanges", &spec, |_, _, _, _| 0.0) {
            DialogResponse::Open | DialogResponse::Enter => {}
            DialogResponse::Action(1) => {
                self.pending_leave = None;
                self.carry_out_leave(frame, action);
            }
            _ => self.pending_leave = None,
        }
    }

    /// Development hook (`GAMETABLE_GAMEDEF_TAB=<tab label>[,select][,suggest][,placement|deck]
    /// [,table]`): opens a tab (`boards`, or the old `playmats`, for Game Boards); `select` selects
    /// the first card on the Cards tab and the first board on the Game Boards tab, `suggest` opens
    /// its Suggest Title dialog, `shrinkart` opens Shrink Card Art, `towebp` Convert to WebP (`start` clicks Convert), `addset` opens the Sets tab's import options
    /// for the folder in `GAMETABLE_SET_FOLDER` (`importall` ticks both options and adds it), `savetozip`
    /// clicks Save to Zip, `placement`/`deck` pick the Scenarios tab's sub-view, and `table`
    /// switches the Game Boards tab to its Table Preview.
    pub fn apply_development_setup(&mut self, steps: &str) {
        for step in steps.split(',').map(str::trim) {
            let label = if matches!(step, "boards" | "playmats") { "Game Boards" } else { step };
            if let Some(position) = self.tab_labels().iter().position(|tab| tab.eq_ignore_ascii_case(label)) {
                self.tab = position;
            }
            match step {
                "select" => {
                    self.cards_tab.select_first(&self.model);
                    self.game_board_tab.select_first(&self.model);
                }
                "suggest" => self.cards_tab.open_suggest_title_for_development(),
                "shrinkart" => self.cards_tab.open_shrink_card_art_for_development(&self.model),
                "towebp" => self.cards_tab.open_convert_to_webp_for_development(&self.model),
                "start" => self.cards_tab.start_convert_to_webp_for_development(),
                "addset" => {
                    if let Ok(folder) = std::env::var("GAMETABLE_SET_FOLDER") {
                        self.sets_tab.open_import_options_for_development(folder);
                    }
                }
                "importall" => self.sets_tab.add_set_with_every_option_for_development(&self.model),
                "savetozip" => self.save_to_zip = Some(SaveToZipFlow::open(&self.model.folder_path)),
                "table" => self.game_board_tab.show_table_preview_for_development(),
                "placement" | "deck" => {
                    self.scenario_tab.get_or_insert_with(ScenarioTab::new).set_view_for_development(step)
                }
                _ => {}
            }
        }
    }

    /// `Editor:load`: switches to another game (keeping the tab).
    fn load(&mut self, game: GameDefinition) {
        let tab = self.tab;
        *self = GameDefinitionEditor { tab, ..GameDefinitionEditor::new(game) };
    }

    /// `Editor:tabLabels()`: optional tabs only while the game opts into them.
    fn tab_labels(&self) -> Vec<&'static str> {
        let mut labels = TAB_LABELS.to_vec();
        if self.model.game.uses_tokens {
            labels.push("Tokens");
        }
        if self.model.game.has_scenarios {
            labels.push("Scenarios");
        }
        labels
    }

    /// Which tab is showing; an optional tab that was turned off falls back to Cards.
    fn active_tab_name(&mut self) -> &'static str {
        let labels = self.tab_labels();
        if self.tab >= labels.len() {
            self.tab = TAB_LABELS.len() - 1;
        }
        labels[self.tab]
    }

    /// `Editor:save()`: warns first when cards without a set would be dropped.
    fn save(&mut self, frame: &mut Frame) {
        let lost = cards_outside_sets(&self.model.game);
        if lost.is_empty() {
            self.write(frame);
            return;
        }
        let names: Vec<String> = lost.iter().take(5).map(|card| format!("\"{}\"", card.card_title)).collect();
        let more = if lost.len() > 5 { format!(" and {} more", lost.len() - 5) } else { String::new() };
        let plural = if lost.len() == 1 { "card isn't" } else { "cards aren't" };
        self.save_warning = Some(format!(
            "{} {plural} in any set ({}{more}). This game uses sets, so cards outside one are not saved and will be \
             lost. Assign them a set on the Cards tab to keep them.",
            lost.len(),
            names.join(", ")
        ));
    }

    /// Card View "Convert to WebP": once its files are written, points the cards (here and in the
    /// saved copy) at them, saves that change alone and deletes the originals.
    fn apply_finished_webp_conversion(&mut self, frame: &mut Frame) {
        let Some(converted) = self.cards_tab.take_webp_conversion_to_apply() else { return };
        let folder_path = self.model.folder_path.clone();
        let applied = apply_webp_conversion(&mut self.model.game, &mut self.saved_game, &folder_path, &converted);
        for original in &converted.originals {
            frame.images.invalidate(original);
        }
        self.cards_tab.finish_webp_conversion(converted, applied);
    }

    fn write(&mut self, frame: &mut Frame) {
        let result = write_game_definition(&self.model.folder_path, &self.model.game);
        clear_existence_cache();
        match result {
            Ok(()) => {
                self.saved_game = self.model.game.clone();
                frame.ui.toast(&format!("Saved to {}", self.model.folder_path), None)
            }
            Err(error) => frame.ui.toast(&format!("Could not save: {error}"), None),
        }
    }

    /// Toolbar "Save to Zip": saves any unsaved changes first, so the package holds what is on
    /// screen, then packs the game folder.
    fn save_to_zip(&mut self, frame: &mut Frame) {
        if self.has_unsaved_changes() {
            self.save(frame);
            if self.save_warning.is_some() {
                self.save_to_zip_after_saving = true;
                return;
            }
        }
        self.start_save_to_zip_if_saved();
    }

    /// Packs the folder, unless the save before it failed or was cancelled.
    fn start_save_to_zip_if_saved(&mut self) {
        if !self.has_unsaved_changes() {
            self.save_to_zip = Some(SaveToZipFlow::open(&self.model.folder_path));
        }
    }

    fn toggle_rulebook(&mut self, frame: &mut Frame) {
        self.rulebook.toggle(frame);
    }

    fn draw_dialogs(&mut self, frame: &mut Frame) {
        if let Some(dialog) = &mut self.new_game {
            match dialog.show(frame) {
                NewGameResponse::Open => {}
                NewGameResponse::Created(game) => {
                    self.new_game = None;
                    self.load(*game);
                }
                NewGameResponse::Cancelled => self.new_game = None,
            }
        }
        if let Some(game) = show_open_picker(frame, &mut self.open_picker) {
            self.load(game);
        }
        if let Some(menu) = &mut self.tools_menu {
            match menu.show(frame) {
                MenuResponse::Open => {}
                MenuResponse::Closed => self.tools_menu = None,
                MenuResponse::Selected(tool) => {
                    self.tools_menu = None;
                    self.tool_dialog = match tool {
                        Tool::DuplicateIds => ToolDialog::duplicate_ids(frame, &self.model.game),
                        Tool::MergeImages => ToolDialog::merge_images(frame, &self.model.game),
                    };
                }
            }
        }
        if let Some(dialog) = &mut self.tool_dialog
            && !dialog.show(frame, &mut self.model.game, &mut self.model.card_keys)
        {
            self.tool_dialog = None;
        }
        if let Some(message) = self.save_warning.clone() {
            let actions = [DialogAction::text("Cancel"), DialogAction::new("Save Anyway", ButtonVariant::Danger)];
            let spec = DialogSpec {
                title: Some("Cards Without a Set"),
                message: Some(&message),
                width: 480.0,
                actions: &actions,
                ..DialogSpec::default()
            };
            match frame.dialog("savewarning", &spec, |_, _, _, _| 0.0) {
                DialogResponse::Open | DialogResponse::Enter => {}
                DialogResponse::Action(1) => {
                    self.save_warning = None;
                    self.write(frame);
                }
                _ => self.save_warning = None,
            }
        }
        if self.save_to_zip_after_saving && self.save_warning.is_none() {
            self.save_to_zip_after_saving = false;
            self.start_save_to_zip_if_saved();
        }
        if let Some(flow) = &mut self.save_to_zip
            && !flow.show(frame)
        {
            self.save_to_zip = None;
        }
        self.draw_unsaved_changes_prompt(frame);
    }
}

impl Screen for GameDefinitionEditor {
    fn title(&self) -> String {
        "Game Definition Editor".to_string()
    }

    fn on_back(&mut self, frame: &mut Frame) {
        self.leave_or_confirm(frame, LeaveAction::Back);
    }

    /// Closing the app with unsaved changes asks first (and cancels this quit).
    fn quit(&mut self, _frame: &mut Frame) -> bool {
        if self.has_unsaved_changes() {
            self.pending_leave = Some(LeaveAction::Quit);
            return true;
        }
        false
    }

    fn update(&mut self, frame: &mut Frame, _dt: f32) {
        self.model.card_keys.sync(self.model.game.cards().len());
        self.sets_tab.update(frame, &mut self.model);
    }

    fn key_pressed(&mut self, frame: &mut Frame, key: &'static str, is_repeat: bool) {
        if self.rulebook.is_open() {
            if key == "r" && frame.keyboard.ctrl() && !is_repeat {
                self.toggle_rulebook(frame);
            }
            return;
        }
        if key == "r" && frame.keyboard.ctrl() && !is_repeat {
            self.toggle_rulebook(frame);
            return;
        }
        match self.active_tab_name() {
            "Cards" => {
                self.cards_tab.key_pressed(frame, &self.model, key);
            }
            "Scenarios" => {
                if let Some(tab) = &mut self.scenario_tab {
                    tab.key_pressed(frame, &mut self.model, key);
                }
            }
            _ => {}
        }
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        self.model.card_keys.sync(self.model.game.cards().len());
        let busy = self.sets_tab.busy;
        let can_save_to_zip = !busy && self.save_to_zip.is_none() && !self.model.folder_path.is_empty();
        let name = &self.model.game.name;
        let mut title = if name.is_empty() { "Game Definition Editor".to_string() } else { name.clone() };
        // A package beside the folder is what the game plays from, so say how edits reach it.
        if !self.model.folder_path.is_empty() {
            let package_path = game_package_path(&self.model.folder_path);
            if std::path::Path::new(&package_path).is_file() {
                title.push_str(&format!(
                    "  \u{2014}  plays from {}; Save to Zip to update it",
                    base_name(&package_path)
                ));
            }
        }
        let actions = [
            ToolbarAction { icon: "note_add", tooltip: "New Game Definition...", id: Some("gde:new"), disabled: busy },
            ToolbarAction { icon: "folder_open", tooltip: "Open Existing...", id: Some("gde:open"), disabled: busy },
            ToolbarAction { icon: "menu_book", tooltip: "Rulebook (Ctrl+R)", id: Some("gde:rulebook"), disabled: busy },
            ToolbarAction { icon: "build", tooltip: "Tools", id: Some("gde:tools"), disabled: busy },
            ToolbarAction {
                icon: "folder_zip",
                tooltip: "Save to Zip (a .gamedef file in the game library)",
                id: Some("gde:savetozip"),
                disabled: !can_save_to_zip,
            },
            ToolbarAction { icon: "save", tooltip: "Save", id: Some("gde:save"), disabled: busy },
        ];
        let clicked = frame.editor_toolbar(x, y, width, &title, &actions);
        let tab_y = y + EDITOR_TOOLBAR_HEIGHT + 1.0;
        let labels = self.tab_labels();
        self.active_tab_name();
        let new_tab = frame.tab_bar("gde:tabs", x, tab_y, width, &labels, self.tab);
        if new_tab != self.tab {
            self.tab = new_tab;
            frame.ui.blur();
        }
        let body_y = tab_y + TAB_BAR_HEIGHT;
        let body_height = y + height - body_y;
        frame.ui.push_clip(x, body_y, width, body_height);
        let active = self.active_tab_name();
        let model = &mut self.model;
        match active {
            "General" => self.settings_tab.draw(frame, model, x, body_y, width, body_height),
            "Game Boards" => self.game_board_tab.draw(frame, model, x, body_y, width, body_height),
            "Zones" => self.zones_tab.draw(frame, model, x, body_y, width, body_height),
            "Sets" => self.sets_tab.draw(frame, model, x, body_y, width, body_height),
            "Tokens" => {
                self.tokens_tab.get_or_insert_with(TokensTab::default).draw(frame, model, x, body_y, width, body_height)
            }
            "Scenarios" => {
                self.scenario_tab.get_or_insert_with(ScenarioTab::new).draw(frame, model, x, body_y, width, body_height)
            }
            _ => self.cards_tab.draw(frame, model, x, body_y, width, body_height),
        }
        frame.ui.pop_clip();
        self.apply_finished_webp_conversion(frame);
        if self.rulebook.is_open() {
            let close = frame.overlay(|frame| self.rulebook.draw(frame, None));
            if close {
                self.toggle_rulebook(frame);
            }
        }
        match clicked {
            Some(0) => self.leave_or_confirm(frame, LeaveAction::NewGame),
            Some(1) => self.leave_or_confirm(frame, LeaveAction::OpenExisting),
            Some(2) => self.toggle_rulebook(frame),
            Some(3) => {
                let (mouse_x, mouse_y) = frame.ui.mouse();
                let items = vec![
                    MenuItem::new("Check for Duplicate Card IDs...", Tool::DuplicateIds).icon("copy_all"),
                    MenuItem::new("Merge Duplicate Card Images...", Tool::MergeImages).icon("merge_type"),
                ];
                let mut menu = ContextMenu::new("gde:toolsmenu", mouse_x - 280.0, mouse_y + 16.0, items);
                menu.width = Some(300.0);
                self.tools_menu = Some(menu);
            }
            Some(4) => self.save_to_zip(frame),
            Some(5) => self.save(frame),
            _ => {}
        }
        self.draw_dialogs(frame);
    }
}

// ---------------------------------------------------------------------------
// Entry screen
// ---------------------------------------------------------------------------

/// Pick a game definition to edit, make a new one, or open one from anywhere.
pub struct GameDefinitionEntry {
    root: String,
    busy: bool,
    /// Frames until the scan runs (so the progress bar gets a frame first).
    pending_scan: Option<u32>,
    entries: Option<Vec<GameDefinition>>,
    error: Option<String>,
    folder_picker: Option<FilePicker>,
    move_prompt: Option<MoveLibraryPrompt>,
    new_game: Option<NewGameDialog>,
    open_picker: Option<FilePicker>,
    /// A packaged game the user picked, waiting for "Extract".
    extract_prompt: Option<GameDefinition>,
    /// Unpacking it into a folder to edit.
    extracting: Option<GameExtractJob>,
}

impl GameDefinitionEntry {
    pub fn new(frame: &Frame) -> GameDefinitionEntry {
        GameDefinitionEntry {
            root: frame.paths.games_folder(frame.settings).to_string_lossy().into_owned(),
            busy: true,
            pending_scan: Some(2),
            entries: None,
            error: None,
            folder_picker: None,
            move_prompt: None,
            new_game: None,
            open_picker: None,
            extract_prompt: None,
            extracting: None,
        }
    }

    /// The Extract prompt for a packaged game, then the progress while it unpacks, then the
    /// editor on the new folder.
    fn show_extract_dialogs(&mut self, frame: &mut Frame) {
        if let Some(game) = &self.extract_prompt {
            let package_path = game.folder_path.clone().unwrap_or_default();
            let package_name = base_name(&package_path).to_string();
            let folder = extraction_folder(&package_path).unwrap_or_default();
            let mut message = format!(
                "\"{}\" is installed as {package_name}. Extract it to a folder so it can be edited? The game keeps \
                 playing from {package_name} until you use Save to Zip.",
                game.name
            );
            if std::path::Path::new(&folder).exists() {
                message.push_str(&format!(" The existing \"{}\" folder will be replaced.", base_name(&folder)));
            }
            match frame.confirm_dialog("gdx:extract", "Extract Game", &message, "Extract", false) {
                ConfirmResponse::Open => {}
                ConfirmResponse::Cancelled => self.extract_prompt = None,
                ConfirmResponse::Confirmed => {
                    self.extract_prompt = None;
                    match GameExtractJob::start(&package_path) {
                        Ok(job) => self.extracting = Some(job),
                        Err(error) => frame.ui.alert("Couldn't Extract Game", &error),
                    }
                }
            }
        }
        let Some(job) = &self.extracting else { return };
        match job.poll() {
            None => {
                let spec = DialogSpec {
                    title: Some("Extracting Game"),
                    width: 420.0,
                    dismissible: false,
                    ..DialogSpec::default()
                };
                let progress = job.progress();
                frame.dialog("gdx:extracting", &spec, |frame, x, y, width| {
                    frame.progress_bar(x, y + 4.0, width, 4.0, Some(progress), None);
                    28.0
                });
            }
            Some(result) => {
                let folder = job.destination_folder.clone();
                self.extracting = None;
                clear_existence_cache();
                self.refresh();
                let opened = result.and_then(|()| {
                    let path = find_game_definition_file(&folder)
                        .ok_or_else(|| format!("No gamedef.json found in {folder}."))?;
                    load_game_definition_file(&path, &folder)
                });
                match opened {
                    Ok(game) => GameDefinitionEntry::open_editor(frame, game),
                    Err(error) => frame.ui.alert("Couldn't Extract Game", &error),
                }
            }
        }
    }

    fn refresh(&mut self) {
        self.busy = true;
        self.pending_scan = Some(2);
    }

    fn open_editor(frame: &mut Frame, game: GameDefinition) {
        frame.commands.push(ScreenCommand::Push(Box::new(GameDefinitionEditor::new(game))));
    }

    fn use_folder(&mut self, frame: &mut Frame, folder: String) {
        let _ = set_games_folder(frame.settings, &folder);
        self.root = folder;
        self.refresh();
    }
}

impl Screen for GameDefinitionEntry {
    fn title(&self) -> String {
        "Game Definition Editor".to_string()
    }

    fn resume(&mut self, frame: &mut Frame) {
        self.root = frame.paths.games_folder(frame.settings).to_string_lossy().into_owned();
        self.refresh();
    }

    fn update(&mut self, _frame: &mut Frame, _dt: f32) {
        let Some(frames_left) = self.pending_scan else { return };
        if frames_left > 1 {
            self.pending_scan = Some(frames_left - 1);
            return;
        }
        self.pending_scan = None;
        clear_existence_cache();
        self.error = None;
        match scan_game_library(&self.root) {
            Ok(entries) => {
                if entries.is_empty() {
                    self.error = Some("No games found in this folder.".to_string());
                }
                if let Ok(wanted) = std::env::var("GAMETABLE_GAMEDEF_EXTRACT")
                    && self.extracting.is_none()
                {
                    self.extract_prompt = entries
                        .iter()
                        .find(|game| {
                            game.id == wanted && game.folder_path.as_deref().is_some_and(is_packaged_game_folder)
                        })
                        .cloned();
                }
                self.entries = Some(entries);
            }
            Err(error) => {
                self.entries = Some(Vec::new());
                self.error = Some(format!("Could not read that folder: {error}"));
            }
        }
        self.busy = false;
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let padding = 24.0;
        let (x, y, width, height) = (x + padding, y + padding, width - 2.0 * padding, height - 2.0 * padding);
        let mut row_y = y;
        let options =
            TextOptions::new().size(12.0).color(palette.primary).width(width - 96.0).middle_of(40.0).ellipsis();
        frame.text(&self.root, x, row_y, options);
        let refresh = IconButtonOptions::tooltip("Refresh").id("gdx:refresh").disabled(self.busy);
        if frame.icon_button("refresh", x + width - 88.0, row_y, &refresh) {
            self.refresh();
        }
        let change = IconButtonOptions::tooltip("Change Folder...").id("gdx:folder").disabled(self.busy);
        if frame.icon_button("folder_open", x + width - 44.0, row_y, &change) {
            self.folder_picker = Some(FilePicker::new(
                frame,
                PickerMode::Folder,
                Some("Choose your game library folder"),
                None,
                Some(&self.root),
            ));
        }
        row_y += 40.0;
        if self.busy {
            frame.progress_bar(x, row_y + 4.0, width, 2.0, None, None);
        }
        row_y += 10.0 + 12.0;

        // New (filled), a divider, Open Existing (outlined) at the bottom.
        let button_height = 48.0;
        let bottom_height = 8.0 + button_height + 20.0 + 1.0 + 8.0 + button_height;
        let grid_height = y + height - bottom_height - row_y;
        match &self.entries {
            None => frame.spinner(x + width / 2.0, row_y + grid_height / 2.0, 12.0, None),
            Some(entries) if entries.is_empty() => {
                let message = self.error.as_deref().unwrap_or("No games found in this folder.");
                let options =
                    TextOptions::new().width(width).middle_of(grid_height).centered().color(palette.on_surface_variant);
                frame.text(message, x, row_y, options);
            }
            Some(entries) => {
                let tiles: Vec<GameTile> = entries
                    .iter()
                    .map(|game| {
                        let mut tile = GameTile::for_game(game);
                        if game.folder_path.as_deref().is_some_and(is_packaged_game_folder) {
                            tile.caption = Some(format!("{} card(s) \u{00b7} .gamedef package", game.cards().len()));
                        }
                        tile
                    })
                    .collect();
                if let Some(index) = frame.game_grid("gdx:grid", &tiles, x, row_y, width, grid_height, padding) {
                    let game = entries[index].clone();
                    if game.folder_path.as_deref().is_some_and(is_packaged_game_folder) {
                        self.extract_prompt = Some(game);
                    } else {
                        GameDefinitionEntry::open_editor(frame, game);
                    }
                }
            }
        }
        let mut button_y = y + height - bottom_height + 8.0;
        let new_options = ButtonOptions { id: Some("gdx:new"), ..ButtonOptions::default() };
        if frame.button("New Game Definition...", x, button_y, Some(width), Some(button_height), &new_options) {
            self.new_game = Some(NewGameDialog::new(frame));
        }
        button_y += button_height + 20.0;
        frame.divider(x, button_y, width, None);
        button_y += 1.0 + 8.0;
        let open_options = ButtonOptions::variant(ButtonVariant::Outlined).id("gdx:open");
        if frame.button("Open Existing...", x, button_y, Some(width), Some(button_height), &open_options) {
            self.open_picker = Some(open_existing_picker(frame));
        }

        if let Some(dialog) = &mut self.new_game {
            match dialog.show(frame) {
                NewGameResponse::Open => {}
                NewGameResponse::Created(game) => {
                    self.new_game = None;
                    GameDefinitionEntry::open_editor(frame, *game);
                }
                NewGameResponse::Cancelled => self.new_game = None,
            }
        }
        if let Some(game) = show_open_picker(frame, &mut self.open_picker) {
            GameDefinitionEntry::open_editor(frame, game);
        }
        if let Some(picker) = &mut self.folder_picker {
            match picker.show(frame) {
                FilePickerResponse::Open => {}
                FilePickerResponse::Cancelled => self.folder_picker = None,
                FilePickerResponse::Picked(mut paths) => {
                    self.folder_picker = None;
                    let chosen = paths.remove(0);
                    match MoveLibraryPrompt::needed(&self.root, &chosen, "game") {
                        Some(prompt) => self.move_prompt = Some(prompt),
                        None => self.use_folder(frame, chosen),
                    }
                }
            }
        }
        if let Some(prompt) = &self.move_prompt
            && prompt.show(frame) == MovePromptResponse::Finished
        {
            let chosen = prompt.new_root.clone();
            self.move_prompt = None;
            self.use_folder(frame, chosen);
        }
        self.show_extract_dialogs(frame);
    }
}
